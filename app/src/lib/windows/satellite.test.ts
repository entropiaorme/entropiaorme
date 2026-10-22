// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The satellite lifecycle under test: a hidden popover webview is created at
// most once per label, creation races the Tauri handshake and the route's
// readiness event against timeouts, any failure resets the factory so a
// later call retries, and show/hide drive the window over the event bus.
// Every Tauri seam is mocked; the factory logic runs for real.
const seams = vi.hoisted(() => {
	const listeners = new Map<string, ((event: { payload?: unknown }) => void)[]>();

	class FakeWebviewWindow {
		static instances: FakeWebviewWindow[] = [];
		static getByLabel = vi.fn(async (): Promise<FakeWebviewWindow | null> => null);
		label: string;
		options: Record<string, unknown>;
		onceHandlers = new Map<string, (event: { payload?: unknown }) => void>();
		setSize = vi.fn(async () => {});
		setPosition = vi.fn(async () => {});
		emit = vi.fn(async () => {});
		show = vi.fn(async () => {});
		setFocus = vi.fn(async () => {});
		isFocused = vi.fn(async () => false);

		constructor(label: string, options: Record<string, unknown>) {
			this.label = label;
			this.options = options;
			FakeWebviewWindow.instances.push(this);
		}

		once(event: string, handler: (event: { payload?: unknown }) => void): Promise<void> {
			this.onceHandlers.set(event, handler);
			return Promise.resolve();
		}
	}

	return {
		listeners,
		FakeWebviewWindow,
		listen: vi.fn((topic: string, cb: (event: { payload?: unknown }) => void) => {
			const existing = listeners.get(topic) ?? [];
			listeners.set(topic, [...existing, cb]);
			return Promise.resolve(() => {
				const current = listeners.get(topic) ?? [];
				listeners.set(
					topic,
					current.filter((fn) => fn !== cb),
				);
			});
		}),
	};
});

vi.mock('@tauri-apps/api/event', () => ({
	listen: seams.listen,
}));

vi.mock('@tauri-apps/api/dpi', () => ({
	LogicalSize: class {
		constructor(
			public width: number,
			public height: number,
		) {}
	},
	PhysicalPosition: class {
		constructor(
			public x: number,
			public y: number,
		) {}
	},
}));

vi.mock('@tauri-apps/api/webviewWindow', () => ({
	WebviewWindow: seams.FakeWebviewWindow,
}));

import { createSatelliteWindow } from './satellite';

const options = {
	label: 'satellite-under-test',
	url: '/satellite-under-test',
	width: 240,
	height: 44,
	readyEvent: 'satellite-under-test:ready',
	showEvent: 'satellite-under-test:show',
	hideEvent: 'satellite-under-test:hide',
};

function fireReady(label: string): void {
	for (const cb of seams.listeners.get(options.readyEvent) ?? []) {
		cb({ payload: { label } });
	}
}

const order = (mock: { mock: { invocationCallOrder: number[] } }) =>
	mock.mock.invocationCallOrder[0];

beforeEach(() => {
	seams.listeners.clear();
	seams.FakeWebviewWindow.instances = [];
	seams.FakeWebviewWindow.getByLabel.mockReset();
	seams.FakeWebviewWindow.getByLabel.mockResolvedValue(null);
	seams.listen.mockClear();
});

afterEach(() => {
	vi.useRealTimers();
});

describe('ensure', () => {
	it('creates the window hidden and resolves once created and ready', async () => {
		const satellite = createSatelliteWindow(options);
		const pending = satellite.ensure();

		await vi.waitFor(() => {
			expect(seams.FakeWebviewWindow.instances).toHaveLength(1);
		});
		const popup = seams.FakeWebviewWindow.instances[0];
		expect(popup.label).toBe(options.label);
		// The full satellite option set: hidden until positioned, chromeless,
		// floating, and out of the taskbar and the focus order.
		expect(popup.options).toMatchObject({
			url: options.url,
			width: options.width,
			height: options.height,
			visible: false,
			decorations: false,
			transparent: true,
			alwaysOnTop: true,
			skipTaskbar: true,
			shadow: false,
			resizable: false,
			focus: false,
		});

		// The ready listener is armed BEFORE creation: a route that reports
		// ready before the creation handshake settles must still satisfy the
		// readiness race (the signal cannot be lost in the gap).
		fireReady(options.label);
		popup.onceHandlers.get('tauri://created')?.({});
		await expect(pending).resolves.toBe(popup);
	});

	it('rejects with the tauri://error payload when creation fails', async () => {
		const satellite = createSatelliteWindow(options);
		const pending = satellite.ensure();
		const rejection = expect(pending).rejects.toThrow('webview exploded');

		await vi.waitFor(() => {
			expect(seams.FakeWebviewWindow.instances).toHaveLength(1);
		});
		seams.FakeWebviewWindow.instances[0].onceHandlers.get('tauri://error')?.({
			payload: 'webview exploded',
		});
		await rejection;
	});

	it('times out a creation that never completes, resets, and retries on the next call', async () => {
		vi.useFakeTimers();
		const satellite = createSatelliteWindow(options);
		const pending = satellite.ensure();
		const rejection = expect(pending).rejects.toThrow('Satellite window creation timed out');

		await vi.advanceTimersByTimeAsync(0);
		expect(seams.FakeWebviewWindow.instances).toHaveLength(1);
		// Neither tauri://created nor tauri://error ever arrives.
		await vi.advanceTimersByTimeAsync(3100);
		await rejection;

		// The failure fully reset the factory: a later call constructs a fresh
		// window rather than adopting the promise that already rejected.
		const retry = satellite.ensure();
		await vi.advanceTimersByTimeAsync(0);
		expect(seams.FakeWebviewWindow.instances).toHaveLength(2);
		const second = seams.FakeWebviewWindow.instances[1];
		second.onceHandlers.get('tauri://created')?.({});
		fireReady(options.label);
		await vi.advanceTimersByTimeAsync(0);
		await expect(retry).resolves.toBe(second);
	});

	it('times out when the route never reports ready, ignoring a wrong-label ready', async () => {
		vi.useFakeTimers();
		const satellite = createSatelliteWindow(options);
		const pending = satellite.ensure();
		const rejection = expect(pending).rejects.toThrow('Satellite window did not become ready');

		await vi.advanceTimersByTimeAsync(0);
		seams.FakeWebviewWindow.instances[0].onceHandlers.get('tauri://created')?.({});
		await vi.advanceTimersByTimeAsync(0);
		// A ready event from the WRONG window label must not satisfy the gate.
		fireReady('some-other-window');
		await vi.advanceTimersByTimeAsync(3100);
		await rejection;
	});

	it('rejects with the overridden messages when the host provides them', async () => {
		vi.useFakeTimers();
		const withMessages = {
			...options,
			messages: {
				creationTimeout: 'Popup window creation timed out',
				creationFailed: 'Unknown Tauri popup creation error',
				readyTimeout: 'Popup route did not become ready',
			},
		};

		// Creation timeout carries the host's wording.
		const satellite = createSatelliteWindow(withMessages);
		const creation = satellite.ensure();
		const creationRejection = expect(creation).rejects.toThrow('Popup window creation timed out');
		await vi.advanceTimersByTimeAsync(3100);
		await creationRejection;

		// tauri://error with an empty payload falls back to the host's wording.
		const failed = satellite.ensure();
		const failedRejection = expect(failed).rejects.toThrow('Unknown Tauri popup creation error');
		await vi.advanceTimersByTimeAsync(0);
		seams.FakeWebviewWindow.instances[1].onceHandlers.get('tauri://error')?.({ payload: '' });
		await failedRejection;

		// Readiness timeout carries the host's wording.
		const ready = satellite.ensure();
		const readyRejection = expect(ready).rejects.toThrow('Popup route did not become ready');
		await vi.advanceTimersByTimeAsync(0);
		seams.FakeWebviewWindow.instances[2].onceHandlers.get('tauri://created')?.({});
		await vi.advanceTimersByTimeAsync(3100);
		await readyRejection;
	});

	it('reuses an existing window by label and skips the whole handshake', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		const satellite = createSatelliteWindow(options);

		await expect(satellite.ensure()).resolves.toBe(existing);
		// No second window was constructed and no readiness listener armed.
		expect(seams.FakeWebviewWindow.instances).toHaveLength(0);
		expect(seams.listen).not.toHaveBeenCalled();

		// The resolved window is memoised: a second ensure re-resolves it
		// without another lookup.
		await expect(satellite.ensure()).resolves.toBe(existing);
		expect(seams.FakeWebviewWindow.getByLabel).toHaveBeenCalledTimes(1);
	});
});

describe('show', () => {
	async function reusableSatellite() {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		return { satellite: createSatelliteWindow(options), existing };
	}

	it('sizes and positions, then emits the state, then reveals, then focuses', async () => {
		const { satellite, existing } = await reusableSatellite();
		const state = { kind: 'menu', rows: 3 };

		await satellite.show(state, { x: 10, y: 20, width: 300, height: 120 }, { focus: true });

		expect(existing.setSize).toHaveBeenCalledWith({ width: 300, height: 120 });
		expect(existing.setPosition).toHaveBeenCalledWith({ x: 10, y: 20 });
		expect(existing.emit).toHaveBeenCalledWith(options.showEvent, state);
		expect(existing.show).toHaveBeenCalledTimes(1);
		expect(existing.setFocus).toHaveBeenCalledTimes(1);
		// The sequence ORDER is the contract: size and position settle before
		// the show payload goes out, and the window is revealed only after it,
		// so the satellite never flashes at a stale location.
		expect(order(existing.setSize)).toBeLessThan(order(existing.setPosition));
		expect(order(existing.setPosition)).toBeLessThan(order(existing.emit));
		expect(order(existing.emit)).toBeLessThan(order(existing.show));
		expect(order(existing.show)).toBeLessThan(order(existing.setFocus));
	});

	it('defaults the size to the creation size and skips focus by default', async () => {
		const { satellite, existing } = await reusableSatellite();

		await satellite.show({}, { x: 1, y: 2 });

		expect(existing.setSize).toHaveBeenCalledWith({
			width: options.width,
			height: options.height,
		});
		expect(existing.setFocus).not.toHaveBeenCalled();
	});

	it('skips sizing and positioning entirely when no position is given', async () => {
		const { satellite, existing } = await reusableSatellite();

		await satellite.show({ selfPositioning: true });

		expect(existing.setSize).not.toHaveBeenCalled();
		expect(existing.setPosition).not.toHaveBeenCalled();
		expect(existing.emit).toHaveBeenCalledWith(options.showEvent, { selfPositioning: true });
		expect(existing.show).toHaveBeenCalledTimes(1);
	});

	it('swallows a focus failure (focus is best-effort)', async () => {
		const { satellite, existing } = await reusableSatellite();
		existing.setFocus.mockRejectedValueOnce(new Error('focus refused'));

		await expect(satellite.show({}, { x: 0, y: 0 }, { focus: true })).resolves.toBeUndefined();
	});

	it('only delivers the state payload when the reveal is opted out', async () => {
		const { satellite, existing } = await reusableSatellite();
		const state = { selfRevealing: true };

		// A self-revealing satellite (it measures, positions, and shows itself
		// from the payload) gets the state and nothing else: no host-side
		// reveal, no host-side focus.
		await satellite.show(state, undefined, { reveal: false, focus: true });

		expect(existing.emit).toHaveBeenCalledWith(options.showEvent, state);
		expect(existing.show).not.toHaveBeenCalled();
		expect(existing.setFocus).not.toHaveBeenCalled();
	});
});

describe('emitTo', () => {
	it('emits the event and payload to an existing window', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		const satellite = createSatelliteWindow(options);

		await satellite.emitTo('satellite-under-test:update', { row: 4 });
		expect(existing.emit).toHaveBeenCalledWith('satellite-under-test:update', { row: 4 });
	});

	it('tolerates the window not existing and never creates one', async () => {
		const satellite = createSatelliteWindow(options);

		await expect(satellite.emitTo('satellite-under-test:update', {})).resolves.toBeUndefined();
		expect(seams.FakeWebviewWindow.instances).toHaveLength(0);
	});

	it('swallows an emit failure', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		existing.emit.mockRejectedValueOnce(new Error('window gone'));
		const satellite = createSatelliteWindow(options);

		await expect(satellite.emitTo('satellite-under-test:update', {})).resolves.toBeUndefined();
	});
});

describe('isFocused', () => {
	it("answers the existing window's focus state", async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		existing.isFocused.mockResolvedValueOnce(true);
		const satellite = createSatelliteWindow(options);

		await expect(satellite.isFocused()).resolves.toBe(true);
	});

	it('is false when the window does not exist, and never creates one', async () => {
		const satellite = createSatelliteWindow(options);

		await expect(satellite.isFocused()).resolves.toBe(false);
		expect(seams.FakeWebviewWindow.instances).toHaveLength(0);
	});

	it('is false when the query fails', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		existing.isFocused.mockRejectedValueOnce(new Error('window gone'));
		const satellite = createSatelliteWindow(options);

		await expect(satellite.isFocused()).resolves.toBe(false);
	});
});

describe('hide', () => {
	it('tolerates the window not existing at all', async () => {
		const satellite = createSatelliteWindow(options);

		await expect(satellite.hide()).resolves.toBeUndefined();
		expect(seams.FakeWebviewWindow.getByLabel).toHaveBeenCalledWith(options.label);
	});

	it('resolves the window from a pending ensure and emits the hide event', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		const satellite = createSatelliteWindow(options);

		void satellite.ensure();
		await satellite.hide();
		expect(existing.emit).toHaveBeenCalledWith(options.hideEvent);
	});

	it('resolves cleanly when the pending ensure it awaited ends up failing', async () => {
		vi.useFakeTimers();
		const satellite = createSatelliteWindow(options);
		const pending = satellite.ensure();
		const rejection = expect(pending).rejects.toThrow('Satellite window creation timed out');
		await vi.advanceTimersByTimeAsync(0);

		// hide() awaits the in-flight creation with catch-to-null: the ensure
		// failure surfaces to the ensure caller, never through hide().
		const hidden = satellite.hide();
		await vi.advanceTimersByTimeAsync(3100);
		await rejection;
		await expect(hidden).resolves.toBeUndefined();
		expect(seams.FakeWebviewWindow.instances[0].emit).not.toHaveBeenCalled();
	});

	it('swallows an emit failure on the hide event', async () => {
		const existing = new seams.FakeWebviewWindow(options.label, {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);
		existing.emit.mockRejectedValueOnce(new Error('window gone'));
		const satellite = createSatelliteWindow(options);

		await expect(satellite.hide()).resolves.toBeUndefined();
	});
});
