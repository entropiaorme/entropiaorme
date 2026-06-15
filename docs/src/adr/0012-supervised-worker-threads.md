# ADR-0012: Named, owned, supervised worker threads

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The backend runs long-lived background work that observes the outside world: a thread that tails the game's `chat.log`, and OS-level keyboard-hook listeners that watch hotbar-slot presses. This work has to run continuously alongside the request-serving web threadpool, mutate state that readers on other threads aggregate, and terminate cleanly at shutdown so a closing application cannot strand a live OS hook or a file handle.

Two failure modes had to be designed out. The first is the free-floating worker: a coroutine spawned with `asyncio.create_task` or a bare `threading.Thread` with no name and no owner, which nobody cancels and which is invisible in a thread dump. The second is the unsupervised coroutine on the request loop, which a slow reader or a producer's final shutdown event could keep alive past teardown. Both shapes also resist the planned port to a native Rust spine, where the target idiom is an owned worker handle that is joined on shutdown. A convention alone does not hold under change; the constraint had to become a checked artefact so it could not silently regress.

## Decision

Every long-lived backend worker is an OS thread that is named, constructed `daemon=True`, owned by the service that manages it, and stopped on shutdown. There are no `asyncio.create_task` workers in the backend; the one place a long-lived task could live, the server-sent-events fan-out, instead lets each connection's drain be owned by its Starlette response lifecycle, which uvicorn supervises.

The chat.log watcher owns its own thread. `ChatlogWatcher.start` in `backend/services/chatlog_watcher.py` constructs `threading.Thread(target=self._tail_loop, daemon=True, name="chatlog-watcher")`, holds a reference to it, and `stop` flips the running flag and joins with a timeout. The watcher (not the tracker that consumes its events) is the owner; the tracker in `backend/tracking/tracker.py` only subscribes to the bus topics the watcher publishes. The tail loop reads on a fixed `TAIL_INTERVAL = 0.1` second poll only when caught up to end-of-file; it is not a tunable, and consumers that need to know the watcher has drained wait on an idle condition variable rather than on that cadence, so convergence does not depend on the poll interval.

The keyboard-hook listeners follow the same rule by a different construction. In the native input source `frontend/src-tauri/eo-services/src/keystroke_source.rs`, the Windows hook installs a dedicated pump thread named `keystroke-hook` and a single owned worker thread named `keystroke-dispatch`; the hook procedure filters to the recognised key vocabulary and enqueues, the owned worker thread applies the allowlist on dispatch, and `Running::shutdown` posts a quit message and joins both threads on drop. The hotbar listener in `backend/services/hotbar_listener.py` gates its source's lifecycle on a capability toggle and an active session, and dispatches each resolved slot on a short-lived `name="hotbar-resolve"`, `daemon=True` thread.

## Consequences

The rule is enforced, not merely documented, by `backend/tests/test_supervised_workers.py`. A static AST scan over all of `backend/` (excluding the test suite) asserts that no production code calls `create_task`, `ensure_future`, `run_in_executor`, `run_coroutine_threadsafe`, or `asyncio.run`, and that every `threading.Thread` literal is constructed `daemon=True` with an explicit non-empty `name=`. The scanners carry their own teeth tests: each flags a planted violation and passes a compliant construction, so a guideline cannot decay into a gate with no force. Runtime checks complete the pair: one asserts the app lifespan detaches the SSE hub from the bus on shutdown, and one drives the watcher directly and asserts its thread is named `chatlog-watcher`, is a daemon, and is no longer alive after `stop`.

This constrains how new background work is added. Any handler needing long-lived work must route it through an owned, named, cancel-on-shutdown worker rather than a free coroutine or an anonymous thread; the pynput keyboard listeners are named at their own construction because they are not `threading.Thread` literals and so fall outside the literal scan by design. The benefit is operational and forward-looking: every worker is identifiable in a thread dump and joinable at teardown, and the shape maps directly onto an owned Rust worker handle, so the supervised-shutdown invariant is pinned ahead of the native port. By convention the webview routes its timer-driven loops through a single visibility-gated `useVisiblePoll` helper rather than bare `setInterval`s. See [the service map](../architecture/service-map.md) for where these workers sit, [ADR-0002](0002-event-spine.md) for the bus they publish onto, and the [ADR index](README.md).

## Evidence

- `backend/services/chatlog_watcher.py`
- `backend/tracking/tracker.py`
- `backend/services/hotbar_listener.py`
- `backend/tests/test_supervised_workers.py`
- `frontend/src-tauri/eo-services/src/keystroke_source.rs`
