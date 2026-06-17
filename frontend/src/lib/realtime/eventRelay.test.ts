import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Mock the side-effecting seams. The module under test holds singleton stream
// state (`source`), so each test re-imports it fresh via vi.resetModules() +
// dynamic import() so relay state is order-independent.
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

vi.mock('$lib/api', () => ({
	EVENTS_STREAM_URL: 'http://127.0.0.1:8421/api/events',
}));

/**
 * Minimal EventSource stand-in: records constructed instances, captures the
 * per-topic listeners, and exposes the open/close lifecycle to the test.
 */
class FakeEventSource {
	static instances: FakeEventSource[] = [];
	url: string;
	closed = false;
	onopen: (() => void) | null = null;
	listeners = new Map<string, ((event: { data: string }) => void)[]>();

	constructor(url: string) {
		this.url = url;
		FakeEventSource.instances.push(this);
	}

	addEventListener(topic: string, listener: (event: { data: string }) => void): void {
		const existing = this.listeners.get(topic) ?? [];
		this.listeners.set(topic, [...existing, listener]);
	}

	close(): void {
		this.closed = true;
	}

	fire(topic: string, data: string): void {
		for (const listener of this.listeners.get(topic) ?? []) {
			listener({ data });
		}
	}
}

type Mod = typeof import('./eventRelay');

// Fresh module (and fresh singleton stream state) per call.
async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./eventRelay');
}

/** Stub a browser-like main-window environment: the happy path. */
function stubMainWindow(): void {
	vi.stubGlobal('window', {});
	vi.stubGlobal('EventSource', FakeEventSource);
	getCurrentWindow.mockReturnValue({ label: 'main' });
}

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
	FakeEventSource.instances = [];
});

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('startEventRelay environment guards', () => {
	it('is a noop without a window global (non-browser context)', async () => {
		vi.stubGlobal('EventSource', FakeEventSource);
		const { startEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(FakeEventSource.instances).toHaveLength(0);
		expect(getCurrentWindow).not.toHaveBeenCalled();
		expect(() => stop()).not.toThrow();
	});

	it('is a noop without an EventSource global', async () => {
		vi.stubGlobal('window', {});
		const { startEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(getCurrentWindow).not.toHaveBeenCalled();
		expect(() => stop()).not.toThrow();
	});

	it('is a noop when getCurrentWindow throws (plain browser preview, no Tauri)', async () => {
		vi.stubGlobal('window', {});
		vi.stubGlobal('EventSource', FakeEventSource);
		getCurrentWindow.mockImplementation(() => {
			throw new Error('not in Tauri');
		});
		const { startEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(FakeEventSource.instances).toHaveLength(0);
		expect(() => stop()).not.toThrow();
	});

	it('does not open a stream from a non-main window', async () => {
		vi.stubGlobal('window', {});
		vi.stubGlobal('EventSource', FakeEventSource);
		getCurrentWindow.mockReturnValue({ label: 'overlay' });
		const { startEventRelay } = await loadModule();
		startEventRelay();
		expect(FakeEventSource.instances).toHaveLength(0);
	});
});

describe('startEventRelay on the main window', () => {
	it('opens one stream on the events URL and listens on both forwarded topics', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		expect(FakeEventSource.instances).toHaveLength(1);
		const stream = FakeEventSource.instances[0];
		expect(stream.url).toBe('http://127.0.0.1:8421/api/events');
		expect([...stream.listeners.keys()]).toEqual([
			'tracking.session.updated',
			'scan.status.changed',
		]);
	});

	it('is idempotent: a second start does not open a second stream', async () => {
		stubMainWindow();
		const { startEventRelay, stopEventRelay } = await loadModule();
		const first = startEventRelay();
		const second = startEventRelay();
		expect(FakeEventSource.instances).toHaveLength(1);
		// Both calls hand back the same module-level stop function.
		expect(first).toBe(stopEventRelay);
		expect(second).toBe(stopEventRelay);
	});

	it('emits a payload-less nudge on every forwarded topic when the stream opens', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		FakeEventSource.instances[0].onopen?.();
		expect(emit.mock.calls).toEqual([
			['tracking:session:updated', {}],
			['scan:status:changed', {}],
		]);
	});

	it('re-emits a frame as the full envelope on the colon-form Tauri topic', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		const envelope = {
			type: 'tracking.session.updated',
			event_version: 1,
			occurred_at: '2026-06-04T10:00:00Z',
			payload: { session_id: 'abc' },
		};
		FakeEventSource.instances[0].fire('tracking.session.updated', JSON.stringify(envelope));
		expect(emit).toHaveBeenCalledTimes(1);
		expect(emit).toHaveBeenCalledWith('tracking:session:updated', envelope);
	});

	it('swallows a malformed frame without emitting', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		FakeEventSource.instances[0].fire('scan.status.changed', 'not json{');
		expect(emit).not.toHaveBeenCalled();
	});

	it('force-cycles the stream onto the native producer on the substrate handover', async () => {
		stubMainWindow();
		const { startEventRelay } = await loadModule();
		startEventRelay();

		expect(listen).toHaveBeenCalledWith('substrate:native-installed', expect.any(Function));
		expect(FakeEventSource.instances).toHaveLength(1);
		const firstStream = FakeEventSource.instances[0];

		// The shell signals it has hot-installed the native service spine.
		handoverCallback?.();

		// The pre-handover stream is closed and a fresh one opened on the same URL.
		expect(firstStream.closed).toBe(true);
		expect(FakeEventSource.instances).toHaveLength(2);
		expect(FakeEventSource.instances[1].url).toBe('http://127.0.0.1:8421/api/events');

		// The reconnected stream re-hydrates every forwarded topic on open.
		emit.mockClear();
		FakeEventSource.instances[1].onopen?.();
		expect(emit.mock.calls).toEqual([
			['tracking:session:updated', {}],
			['scan:status:changed', {}],
		]);
	});
});

describe('stopEventRelay', () => {
	it('closes the stream, and a later start opens a fresh one', async () => {
		stubMainWindow();
		const { startEventRelay, stopEventRelay } = await loadModule();
		const stop = startEventRelay();
		expect(stop).toBe(stopEventRelay);

		stop();
		expect(FakeEventSource.instances[0].closed).toBe(true);

		startEventRelay();
		expect(FakeEventSource.instances).toHaveLength(2);
	});

	it('is safe to call when no stream is open', async () => {
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
});
