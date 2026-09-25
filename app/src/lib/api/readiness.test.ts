import { beforeEach, describe, expect, it, vi } from 'vitest';

// The readiness gate and the typed transport it guards. The shell's readiness
// answer and Tauri's `invoke` are both mocked; every test re-imports the
// modules so each starts from a fresh, unsettled gate.
const { awaitSubstrate, tauriInvoke } = vi.hoisted(() => ({
	awaitSubstrate: vi.fn(),
	tauriInvoke: vi.fn(),
}));

vi.mock('./shell', () => ({ awaitSubstrate }));
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => tauriInvoke(...args),
}));

type Readiness = typeof import('./readiness.svelte');
type Transport = typeof import('./invoke');

async function load(): Promise<Readiness & Transport> {
	vi.resetModules();
	const readiness = await import('./readiness.svelte');
	const transport = await import('./invoke');
	return { ...readiness, ...transport };
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	let reject!: (reason: unknown) => void;
	const promise = new Promise<T>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

/** Let every queued microtask run. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
	awaitSubstrate.mockReset();
	tauriInvoke.mockReset();
	tauriInvoke.mockResolvedValue('payload');
	vi.spyOn(console, 'warn').mockImplementation(() => {});
});

describe('delayed startup', () => {
	it('holds commands while the backend starts and sends them once it is ready', async () => {
		const outcome = deferred<{ state: 'ready' }>();
		awaitSubstrate.mockReturnValue(outcome.promise);
		const { invokeCommand, substrate } = await load();

		const first = invokeCommand('tracking_snapshot', {});
		const second = invokeCommand('quests_list', { status: null });
		await flush();
		expect(substrate.phase).toBe('starting');
		expect(tauriInvoke).not.toHaveBeenCalled();

		outcome.resolve({ state: 'ready' });
		await expect(first).resolves.toBe('payload');
		await expect(second).resolves.toBe('payload');
		expect(substrate.phase).toBe('ready');
		expect(tauriInvoke).toHaveBeenCalledTimes(2);
		expect(tauriInvoke).toHaveBeenCalledWith('tracking_snapshot', {});
		expect(tauriInvoke).toHaveBeenCalledWith('quests_list', { status: null });
	});

	it('asks the shell once however many calls arrive while it starts', async () => {
		const outcome = deferred<{ state: 'ready' }>();
		awaitSubstrate.mockReturnValue(outcome.promise);
		const { invokeCommand, whenSubstrateReady } = await load();

		const calls = Array.from({ length: 5 }, () => invokeCommand('tracking_snapshot', {}));
		void whenSubstrateReady();
		outcome.resolve({ state: 'ready' });
		await Promise.all(calls);
		await invokeCommand('tracking_snapshot', {});
		expect(awaitSubstrate).toHaveBeenCalledTimes(1);
	});
});

describe('startup already finished', () => {
	it('dispatches at once when the backend was ready before this window asked', async () => {
		awaitSubstrate.mockResolvedValue({ state: 'ready' });
		const { invokeCommand, substrate } = await load();

		await expect(invokeCommand('tracking_snapshot', {})).resolves.toBe('payload');
		expect(substrate.phase).toBe('ready');
		expect(substrate.failure).toBeNull();
	});
});

describe('failed startup', () => {
	it('rejects every held call as unavailable and records the reason', async () => {
		const outcome = deferred<unknown>();
		awaitSubstrate.mockReturnValue(outcome.promise);
		const { invokeCommand, substrate, SUBSTRATE_FAILED_MESSAGE } = await load();

		const held = invokeCommand('tracking_snapshot', {});
		outcome.resolve({
			state: 'failed',
			reason: 'database_below_baseline',
			detail: 'schema v28',
		});
		await expect(held).rejects.toMatchObject({
			name: 'ApiError',
			kind: 'unavailable',
			message: SUBSTRATE_FAILED_MESSAGE,
		});
		expect(substrate.phase).toBe('failed');
		expect(substrate.failure).toEqual({ reason: 'database_below_baseline', detail: 'schema v28' });
		expect(tauriInvoke).not.toHaveBeenCalled();
	});

	it('rejects later calls immediately without asking again', async () => {
		awaitSubstrate.mockResolvedValue({ state: 'failed', reason: 'unexpected', detail: 'panic' });
		const { invokeCommand } = await load();

		await expect(invokeCommand('tracking_snapshot', {})).rejects.toMatchObject({
			kind: 'unavailable',
		});
		await expect(invokeCommand('quests_list', {})).rejects.toMatchObject({ kind: 'unavailable' });
		expect(awaitSubstrate).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).not.toHaveBeenCalled();
	});
});

describe('readiness question unavailable', () => {
	it('opens the gate rather than holding commands forever', async () => {
		awaitSubstrate.mockRejectedValue(new Error('command substrate_ready not allowed'));
		const { invokeCommand, substrate } = await load();

		await expect(invokeCommand('tracking_snapshot', {})).resolves.toBe('payload');
		expect(substrate.phase).toBe('ready');
		expect(console.warn).toHaveBeenCalled();
	});
});
