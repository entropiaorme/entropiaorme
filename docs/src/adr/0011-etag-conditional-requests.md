# ADR-0011: Strong-ETag conditional requests

- Status: Accepted
- Context: reflects the landed implementation

## Context and problem statement

The desktop frontend hydrates several read surfaces (the dashboard snapshot, the manual-scan status overlay, the quest and codex tables) and keeps them current by re-reading the relevant endpoint whenever a pushed event signals that state may have moved (see [ADR-0009: push-to-pull invalidation](0009-push-to-pull-invalidation.md)). A push only says that *something* changed, not whether the specific resource a given window reads changed; re-reading on every push therefore risks re-fetching and re-rendering a body identical to the one already held.

The body skip must be reproducible across reads and cheap to verify. Two reads of the same unchanged state must resolve to the same representation, the validator must be computable from the response alone (these are stateless reads with no stored version column), and the mechanism must not silently widen its remit to the rest of the HTTP surface.

## Decision

A single HTTP middleware (`backend/middleware/etag.py`, `etag_dispatch`) wraps eligible responses with a strong validator and conditional-GET handling. Eligibility is deliberately narrow: only `GET` requests whose path falls under one of four hydration prefixes (`/api/tracking`, `/api/scan`, `/api/quests`, `/api/codex`), and only when the inner response status is 2xx. Non-GETs, non-2xx responses, and any path outside the four prefixes pass through untouched.

For an eligible response the middleware drains the body, computes a strong ETag as the quoted SHA-256 hex of the exact response bytes (`compute_strong_etag`), and sets `ETag` plus `Cache-Control: no-cache`. The `no-cache` directive mandates revalidation on every request: the goal is to skip the body, not the network round-trip.

When the request carries an `If-None-Match` that names the freshly-computed representation, the middleware returns `304 Not Modified` with an empty body, dropping `Content-Type` but preserving the validator and the cross-cutting headers an inner layer added (CORS, `Vary`, `Date`), per RFC 7232 section 4.1. Matching follows the RFC's weak-comparison rule (`if_none_match_matches`): the `*` wildcard matches any representation, a comma-separated list matches if any member matches, and a weak `W/"..."` candidate matches a strong `"..."` server tag because the weak/strong prefix is ignored for conditional GETs.

Because the ETag is computed over the bytes the HTTP layer actually emits, it is sensitive to the exact serialisation form. The wire body is compact JSON (`to_wire_json` in `eo-wire`: separators `,` and `:`, no spaces) in the response models' declared insertion order, with floats rendered in Python's shortest round-tripping `repr` form. The native Rust read handlers reproduce this contract byte-for-byte: `frontend/src-tauri/eo-http/src/hydration.rs` carries its own `compute_strong_etag`, `if_none_match_matches`, and `conditional_response`, so a natively-served read returns the same validator the Python middleware would, and the contract applies to any 2xx GET body regardless of media type (the manual-scan capture PNG rides it exactly as the JSON reads do).

## Consequences

The hydrate-and-subscribe loop becomes cheap: a window re-reads on each push, but unchanged state returns a 304 with no body, so no re-parse and no re-render occur. The validator is self-contained, requiring no stored version column.

The cost is a tight coupling to byte-level serialisation. The equivalence goldens canonicalise bodies with sorted keys (`to_python_json`) for diff stability, whereas the ETag hashes the wire form with insertion-order keys; the two forms are intentionally distinct. A change that alters the wire bytes (key order, float rendering, separator spacing) without altering the logical value churns the ETag even while a body golden still passes, so serialisation drift surfaces as an ETag mismatch rather than passing silently.

The narrow remit is enforced, not merely intended. `backend/tests/test_etag.py` asserts the strong-ETag format and `Cache-Control: no-cache` on every round-trippable hydration GET, ETag stability across two reads at unchanged state, the 304-with-empty-body path, the 200 fall-through on a mismatch, and the weak/wildcard/comma-list parsing rules. It enumerates the route table (`covered_get_routes`) to confirm the contract fires on every hydration-prefix GET, and asserts that canonical out-of-scope routes (health, character, equipment, analytics, settings) carry neither header, so widening the prefix set fails the suite; the Rust port pins the same contract hermetically. The network-quiet seam (`backend/tests/test_network_quiet_seam.py`) drives the dashboard, scan, and overlay flows against a real loopback server behind a request recorder, proving each stays current on the snapshot plus the push alone and never re-issues the retired polling endpoints.

## Evidence

- `backend/middleware/etag.py`
- `backend/tests/test_etag.py`
- `backend/tests/test_network_quiet_seam.py`
- `frontend/src-tauri/eo-http/src/hydration.rs`
- `frontend/src-tauri/eo-wire/src/normalizer.rs`
