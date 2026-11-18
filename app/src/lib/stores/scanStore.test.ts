import { beforeEach, describe, expect, it, vi } from 'vitest';

// The scan store is a createSnapshotStore instance over the scan topic; the
// factory's coalescing and keep-last-good semantics carry their own suite
// (lib/realtime/snapshotStore.test.ts). Pinned here: the module's own
// surface, i.e. the topic wiring, the injected status read, and the
// singleton semantics its consumers rely on. Module-level state, so fresh
// import per test.
const getSkillScanStatus = vi.fn();
const listen = vi.fn();

vi.mock('$lib/api', () => ({
	getSkillScanStatus: (...args: unknown[]) => getSkillScanStatus(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listen(...args),
}));

type Mod = typeof import('./scanStore.svelte');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./scanStore.svelte');
}

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

const statusIdle = { phase: 'idle', available: true, interrupted: false };
const statusCapturing = {
	phase: 'collecting',
	pagesCollected: 1,
	pageCount: 13,
	missingPages: [2, 3],
	hint: null,
};

beforeEach(() => {
	getSkillScanStatus.mockReset();
	listen.mockReset();
});

describe('hydrate', () => {
	it('publishes the fetched status into the store', async () => {
		getSkillScanStatus.mockResolvedValue(statusIdle);
		const { hydrate, scanStatus } = await loadModule();
		expect(scanStatus.current).toBeNull();

		await hydrate();
		expect(scanStatus.current).toEqual(statusIdle);
	});

	it('keeps the last good status when a read fails', async () => {
		getSkillScanStatus.mockResolvedValueOnce(statusCapturing);
		const { hydrate, scanStatus } = await loadModule();
		await hydrate();

		getSkillScanStatus.mockRejectedValueOnce(new Error('backend away'));
		await hydrate();
		expect(scanStatus.current).toEqual(statusCapturing);
	});

	it('coalesces overlapping calls into exactly one queued follow-up read', async () => {
		const first = deferred<typeof statusIdle>();
		getSkillScanStatus.mockReturnValueOnce(first.promise).mockResolvedValue(statusCapturing);
		const { hydrate, scanStatus } = await loadModule();

		const inFlight = hydrate();
		void hydrate();
		void hydrate();
		expect(getSkillScanStatus).toHaveBeenCalledTimes(1);

		first.resolve(statusIdle);
		await inFlight;
		expect(getSkillScanStatus).toHaveBeenCalledTimes(2);
		expect(scanStatus.current).toEqual(statusCapturing);
	});
});

describe('subscribeScan', () => {
	it('listens on the exported colon-form scan topic', async () => {
		const unlisten = vi.fn();
		listen.mockResolvedValue(unlisten);
		const { subscribeScan, SCAN_TOPIC } = await loadModule();

		const returned = await subscribeScan();
		expect(SCAN_TOPIC).toBe('scan:status:changed');
		expect(listen.mock.calls[0][0]).toBe(SCAN_TOPIC);
		expect(returned).toBe(unlisten);
	});

	it('re-reads the status on every relayed frame, payload or not', async () => {
		getSkillScanStatus.mockResolvedValue(statusCapturing);
		listen.mockResolvedValue(vi.fn());
		const { subscribeScan, scanStatus } = await loadModule();
		await subscribeScan();

		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		// Settle on the store value, not the call count (the write lands a
		// microtask after the read fires).
		onFrame({ payload: {} });
		await vi.waitFor(() => {
			expect(scanStatus.current).toEqual(statusCapturing);
		});
		expect(getSkillScanStatus).toHaveBeenCalledTimes(1);
	});
});
