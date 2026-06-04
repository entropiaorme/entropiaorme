import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

// Mirrors trackingStore.test.ts: the same coalescer pattern over the scan
// status read. Singleton module state, so fresh import per test.
const getManualSkillScanStatus = vi.fn();
const listen = vi.fn();

vi.mock('$lib/api', () => ({
	getManualSkillScanStatus: (...args: unknown[]) => getManualSkillScanStatus(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listen(...args),
}));

type Mod = typeof import('./scanStore');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./scanStore');
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (err: unknown) => void } {
	let resolve!: (value: T) => void;
	let reject!: (err: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

const statusIdle = { active: false, phase: 'idle' };
const statusCapturing = { active: true, phase: 'capturing' };

beforeEach(() => {
	getManualSkillScanStatus.mockReset();
	listen.mockReset();
});

describe('hydrate', () => {
	it('publishes the fetched status into the store', async () => {
		getManualSkillScanStatus.mockResolvedValue(statusIdle);
		const { hydrate, scanStatus } = await loadModule();
		expect(get(scanStatus)).toBeNull();

		await hydrate();
		expect(get(scanStatus)).toEqual(statusIdle);
	});

	it('keeps the last good status when a read fails', async () => {
		getManualSkillScanStatus.mockResolvedValueOnce(statusCapturing);
		const { hydrate, scanStatus } = await loadModule();
		await hydrate();

		getManualSkillScanStatus.mockRejectedValueOnce(new Error('backend away'));
		await hydrate();
		expect(get(scanStatus)).toEqual(statusCapturing);
	});

	it('coalesces overlapping calls into exactly one queued follow-up read', async () => {
		const first = deferred<typeof statusIdle>();
		getManualSkillScanStatus.mockReturnValueOnce(first.promise).mockResolvedValue(statusCapturing);
		const { hydrate, scanStatus } = await loadModule();

		const inFlight = hydrate();
		void hydrate();
		void hydrate();
		expect(getManualSkillScanStatus).toHaveBeenCalledTimes(1);

		first.resolve(statusIdle);
		await inFlight;
		expect(getManualSkillScanStatus).toHaveBeenCalledTimes(2);
		expect(get(scanStatus)).toEqual(statusCapturing);
	});

	it('still runs the queued follow-up when the in-flight read fails', async () => {
		const first = deferred<typeof statusIdle>();
		getManualSkillScanStatus.mockReturnValueOnce(first.promise).mockResolvedValue(statusCapturing);
		const { hydrate, scanStatus } = await loadModule();

		const inFlight = hydrate();
		void hydrate();
		first.reject(new Error('mid-read drop'));
		await inFlight;

		expect(getManualSkillScanStatus).toHaveBeenCalledTimes(2);
		expect(get(scanStatus)).toEqual(statusCapturing);
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
		getManualSkillScanStatus.mockResolvedValue(statusCapturing);
		listen.mockResolvedValue(vi.fn());
		const { subscribeScan, scanStatus } = await loadModule();
		await subscribeScan();

		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		// Settle on the store value, not the call count (the set lands a
		// microtask after the read fires).
		onFrame({ payload: {} });
		await vi.waitFor(() => {
			expect(get(scanStatus)).toEqual(statusCapturing);
		});
		expect(getManualSkillScanStatus).toHaveBeenCalledTimes(1);
	});
});
