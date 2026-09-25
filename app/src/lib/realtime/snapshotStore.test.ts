import { beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the one side-effecting seam the factory owns: the Tauri event bus.
// The snapshot read is injected, so each test passes its own mock. Unlike
// the legacy module-singleton stores, the factory holds no module state:
// every test builds a fresh instance, so no vi.resetModules() dance.
const listen = vi.fn();

vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listen(...args),
}));

import { createSnapshotStore } from './snapshotStore.svelte';

/** A manually resolvable snapshot read, for driving the coalescer mid-flight. */
function deferred<T>(): {
	promise: Promise<T>;
	resolve: (value: T) => void;
	reject: (err: unknown) => void;
} {
	let resolve!: (value: T) => void;
	let reject!: (err: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

type Snapshot = { status: string; kill_count: number };

const snapshotA: Snapshot = { status: 'active', kill_count: 1 };
const snapshotB: Snapshot = { status: 'active', kill_count: 2 };

const TOPIC = 'tracking:session:updated';

beforeEach(() => {
	listen.mockReset();
});

describe('hydrate', () => {
	it('publishes the fetched snapshot into current', async () => {
		const read = vi.fn().mockResolvedValue(snapshotA);
		const store = createSnapshotStore(TOPIC, read);
		expect(store.current).toBeNull();

		await store.hydrate();
		expect(store.current).toEqual(snapshotA);
	});

	it('keeps the last good snapshot when a read fails', async () => {
		const read = vi.fn().mockResolvedValueOnce(snapshotA);
		const store = createSnapshotStore(TOPIC, read);
		await store.hydrate();

		read.mockRejectedValueOnce(new Error('backend away'));
		await store.hydrate();
		expect(store.current).toEqual(snapshotA);
		expect(read).toHaveBeenCalledTimes(2);
	});

	it('coalesces overlapping calls into exactly one queued follow-up read', async () => {
		const first = deferred<Snapshot>();
		const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(snapshotB);
		const store = createSnapshotStore(TOPIC, read);

		const inFlight = store.hydrate();
		// Three frames land while the first read is still in flight: they must
		// fold into ONE follow-up read, not three.
		void store.hydrate();
		void store.hydrate();
		void store.hydrate();
		expect(read).toHaveBeenCalledTimes(1);

		first.resolve(snapshotA);
		await inFlight;
		expect(read).toHaveBeenCalledTimes(2);
		expect(store.current).toEqual(snapshotB);
	});

	it('still runs the queued follow-up when the in-flight read fails', async () => {
		const first = deferred<Snapshot>();
		const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(snapshotB);
		const store = createSnapshotStore(TOPIC, read);

		const inFlight = store.hydrate();
		void store.hydrate(); // queued during the failing attempt; may be the last transition
		first.reject(new Error('mid-read drop'));
		await inFlight;

		expect(read).toHaveBeenCalledTimes(2);
		expect(store.current).toEqual(snapshotB);
	});

	it('keeps coalescer state per instance, not per module', async () => {
		const firstA = deferred<Snapshot>();
		const readA = vi.fn().mockReturnValueOnce(firstA.promise);
		const readB = vi.fn().mockResolvedValue(snapshotB);
		const storeA = createSnapshotStore(TOPIC, readA);
		const storeB = createSnapshotStore(TOPIC, readB);

		const inFlightA = storeA.hydrate();
		// A read in flight on one store must not make another store queue.
		await storeB.hydrate();
		expect(readB).toHaveBeenCalledTimes(1);
		expect(storeB.current).toEqual(snapshotB);

		firstA.resolve(snapshotA);
		await inFlightA;
		expect(readA).toHaveBeenCalledTimes(1);
		expect(storeA.current).toEqual(snapshotA);
	});
});

describe('subscribe', () => {
	it('listens on the given topic and returns the detach function', async () => {
		const unlisten = vi.fn();
		listen.mockResolvedValue(unlisten);
		const store = createSnapshotStore(TOPIC, vi.fn());

		const returned = await store.subscribe();
		expect(listen).toHaveBeenCalledTimes(1);
		expect(listen.mock.calls[0][0]).toBe(TOPIC);
		expect(returned).toBe(unlisten);
	});

	it('re-reads the snapshot on every relayed frame, payload or not', async () => {
		const read = vi.fn().mockResolvedValue(snapshotA);
		listen.mockResolvedValue(vi.fn());
		const store = createSnapshotStore(TOPIC, read);
		await store.subscribe();

		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		// A frame is a pure trigger: the callback ignores the payload entirely,
		// so a payload-less frame re-reads instead of blanking.
		// Wait on the settled STORE VALUE, not the call count: the write happens
		// a microtask after the read fires, so settling on the value is
		// race-free.
		onFrame({ payload: {} });
		await vi.waitFor(() => {
			expect(store.current).toEqual(snapshotA);
		});
		expect(read).toHaveBeenCalledTimes(1);
	});
});
