import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the side-effecting seams. The module holds singleton state (`started`),
// so each test re-imports it fresh via vi.resetModules() + dynamic import() so
// relay state is order-independent.
const emit = vi.fn();
const getCurrentWindow = vi.fn();
const listen = vi.fn();
const handoverUnlisten = vi.fn();
let handoverCallback: (() => void) | undefined;

vi.mock('@tauri-apps/api/event', () => ({
	emit: (...args: unknown[]) => emit(...args),
	listen: (...args: unknown[]) => listen(...args),
}));

vi.mock('@tauri-apps/api/window', () => ({
	getCurrentWindow: (...args: unknown[]) => getCurrentWindow(...args),
}));

type Mod = typeof import('./eventRelay');

// Fresh module (and fresh singleton state) per call.
async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./eventRelay');
}

/** Stub a main-window Tauri-webview environment: the happy path. */
function stubMainWindow(): void {
	vi.stubGlobal('window', {});
	getCurrentWindow.mockReturnValue({ label: 'main' });
}

const HYDRATE_CALLS = [
	['tracking:session:updated', {}],
	['scan:status:changed', {}],
];

beforeEach(() => {
	emit.mockReset();
	getCurrentWindow.mockReset();
	listen.mockReset();
	handoverUnlisten.mockReset();
	handoverCallback = undefined;
	// Capture the handover callback so a test can fire it; resolve with a spyable
	// unlisten so teardown can be asserted.
	listen.mockImplementation((_event: string, cb: () => void) => {
		handoverCallback = cb;
		return Promise.resolve(handoverUnlisten);
	});
});

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('startEventRelay environment guards', () => {
	it('is a noop without a window global (non-browser context)', async () => {
		const { startEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(getCurrentWindow).not.toHaveBeenCalled();
		expect(emit).not.toHaveBeenCalled();
		expect(() => stop()).not.toThrow();
	});

	it('is a noop when getCurrentWindow throws (plain browser preview, no Tauri)', async () => {
		vi.stubGlobal('window', {});
		getCurrentWindow.mockImplementation(() => {
			throw new Error('not in Tauri');
		});
		const { startEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(emit).not.toHaveBeenCalled();
		expect(listen).not.toHaveBeenCalled();
		expect(() => stop()).not.toThrow();
	});

	it('does not hydrate or listen from a non-main window', async () => {
		vi.stubGlobal('window', {});
		getCurrentWindow.mockReturnValue({ label: 'overlay' });
		const { startEventRelay } = await loadModule();
		startEventRelay();
		expect(emit).not.toHaveBeenCalled();
		expect(listen).not.toHaveBeenCalled();
	});
});

describe('startEventRelay on the main window', () => {
	it('emits an initial payload-less hydrate on every forwarded topic', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();
		expect(emit.mock.calls).toEqual(HYDRATE_CALLS);
	});

	it('attaches the substrate-handover listener', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();
		expect(listen).toHaveBeenCalledWith('substrate:native-installed', expect.any(Function));
	});

	it('is idempotent: a second start does not hydrate again', async () => {
		stubMainWindow();
		const { startEventRelay, stopEventRelay } = await loadModule();
		const first = startEventRelay();
		const second = startEventRelay();
		// One hydrate only (the first start); the second is a no-op.
		expect(emit.mock.calls).toEqual(HYDRATE_CALLS);
		expect(listen).toHaveBeenCalledTimes(1);
		// Both calls hand back the same module-level stop function.
		expect(first).toBe(stopEventRelay);
		expect(second).toBe(stopEventRelay);
	});

	it('re-hydrates every forwarded topic on the substrate handover', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		// The shell signals it has hot-installed the native service spine.
		emit.mockClear();
		handoverCallback?.();
		expect(emit.mock.calls).toEqual(HYDRATE_CALLS);
	});
});

describe('stopEventRelay', () => {
	it('is safe to call when not started', async () => {
		const { stopEventRelay } = await loadModule();
		expect(() => stopEventRelay()).not.toThrow();
	});

	it('detaches the handover listener', async () => {
		stubMainWindow();
		const { startEventRelay, stopEventRelay } = await loadModule();
		startEventRelay();
		// Flush the listen() promise so the unlisten handle is stored.
		await Promise.resolve();
		await Promise.resolve();
		stopEventRelay();
		expect(handoverUnlisten).toHaveBeenCalledTimes(1);
	});

	it('clears state, so a later start hydrates again', async () => {
		stubMainWindow();
		const { startEventRelay, stopEventRelay } = await loadModule();
		startEventRelay();
		await Promise.resolve();
		stopEventRelay();

		emit.mockClear();
		startEventRelay();
		expect(emit.mock.calls).toEqual(HYDRATE_CALLS);
	});
});
