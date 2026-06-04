import { get } from 'svelte/store';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the two side-effecting seams: the snapshot read and the Tauri event
// bus. The module under test holds singleton coalescer state (inFlight /
// refetchQueued) plus the writable store, so each test re-imports it fresh via
// vi.resetModules() + dynamic import() so state is order-independent.
const getTrackingSnapshot = vi.fn();
const listen = vi.fn();

vi.mock('$lib/api', () => ({
	getTrackingSnapshot: (...args: unknown[]) => getTrackingSnapshot(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listen(...args),
}));

type Mod = typeof import('./trackingStore');

// Fresh module (and fresh coalescer + store) per call.
async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./trackingStore');
}

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

const snapshotA = { status: 'active', kill_count: 1 };
const snapshotB = { status: 'active', kill_count: 2 };

beforeEach(() => {
	getTrackingSnapshot.mockReset();
	listen.mockReset();
});

describe('hydrate', () => {
	it('publishes the fetched snapshot into the store', async () => {
		getTrackingSnapshot.mockResolvedValue(snapshotA);
		const { hydrate, trackingSnapshot } = await loadModule();
		expect(get(trackingSnapshot)).toBeNull();

		await hydrate();
		expect(get(trackingSnapshot)).toEqual(snapshotA);
	});

	it('keeps the last good snapshot when a read fails', async () => {
		getTrackingSnapshot.mockResolvedValueOnce(snapshotA);
		const { hydrate, trackingSnapshot } = await loadModule();
		await hydrate();

		getTrackingSnapshot.mockRejectedValueOnce(new Error('backend away'));
		await hydrate();
		expect(get(trackingSnapshot)).toEqual(snapshotA);
		expect(getTrackingSnapshot).toHaveBeenCalledTimes(2);
	});

	it('coalesces overlapping calls into exactly one queued follow-up read', async () => {
		const first = deferred<typeof snapshotA>();
		getTrackingSnapshot.mockReturnValueOnce(first.promise).mockResolvedValue(snapshotB);
		const { hydrate, trackingSnapshot } = await loadModule();

		const inFlight = hydrate();
		// Three frames land while the first read is still in flight: they must
		// fold into ONE follow-up read, not three.
		void hydrate();
		void hydrate();
		void hydrate();
		expect(getTrackingSnapshot).toHaveBeenCalledTimes(1);

		first.resolve(snapshotA);
		await inFlight;
		expect(getTrackingSnapshot).toHaveBeenCalledTimes(2);
		expect(get(trackingSnapshot)).toEqual(snapshotB);
	});

	it('still runs the queued follow-up when the in-flight read fails', async () => {
		const first = deferred<typeof snapshotA>();
		getTrackingSnapshot.mockReturnValueOnce(first.promise).mockResolvedValue(snapshotB);
		const { hydrate, trackingSnapshot } = await loadModule();

		const inFlight = hydrate();
		void hydrate(); // queued during the failing attempt; may be the last transition
		first.reject(new Error('mid-read drop'));
		await inFlight;

		expect(getTrackingSnapshot).toHaveBeenCalledTimes(2);
		expect(get(trackingSnapshot)).toEqual(snapshotB);
	});
});

describe('subscribeTracking', () => {
	it('listens on the colon-form tracking topic', async () => {
		const unlisten = vi.fn();
		listen.mockResolvedValue(unlisten);
		const { subscribeTracking } = await loadModule();

		const returned = await subscribeTracking();
		expect(listen).toHaveBeenCalledTimes(1);
		expect(listen.mock.calls[0][0]).toBe('tracking:session:updated');
		expect(returned).toBe(unlisten);
	});

	it('re-reads the snapshot on every relayed frame, payload or not', async () => {
		getTrackingSnapshot.mockResolvedValue(snapshotA);
		listen.mockResolvedValue(vi.fn());
		const { subscribeTracking, trackingSnapshot } = await loadModule();
		await subscribeTracking();

		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		// A frame is a pure trigger: the callback ignores the payload entirely,
		// so a payload-less reconnect nudge re-reads instead of blanking.
		// Wait on the settled STORE VALUE, not the call count: the set happens a
		// microtask after the read fires, so settling on the value is race-free.
		onFrame({ payload: {} });
		await vi.waitFor(() => {
			expect(get(trackingSnapshot)).toEqual(snapshotA);
		});
		expect(getTrackingSnapshot).toHaveBeenCalledTimes(1);
	});
});
