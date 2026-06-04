// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/svelte';

// The overlay window's popup orchestration: a hidden popup webview is spawned
// once, and showing a menu must wait on the popup route's readiness handshake
// (its `:ready` event) before emitting the show payload and revealing the
// window; a popup that never reports ready times out into a rendered launch
// error. Every Tauri seam is mocked; the page, OverlayStrip, and the
// handshake logic run for real.
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
		emit: vi.fn(async () => {}),
		getTrackingSnapshot: vi.fn(),
		getOverlayPosition: vi.fn(async () => ({ x: null, y: null })),
		saveOverlayPosition: vi.fn(async () => {}),
		updateSettings: vi.fn(async () => ({})),
		currentWindow: {
			startDragging: vi.fn(async () => {}),
			setSize: vi.fn(async () => {}),
			outerPosition: vi.fn(async () => ({ x: 0, y: 0 })),
			scaleFactor: vi.fn(async () => 1),
		},
		overlayStats: {
			subscribe(fn: (v: never[]) => void): () => void {
				fn([]);
				return () => {};
			},
		},
	};
});

vi.mock('$lib/api', () => {
	class ApiError extends Error {
		constructor(
			public status: number,
			message: string,
		) {
			super(message);
			this.name = 'ApiError';
		}
	}
	return {
		ApiError,
		getTrackingSnapshot: seams.getTrackingSnapshot,
		startTracking: vi.fn(),
		stopTracking: vi.fn(),
		releaseMob: vi.fn(),
		getOverlayPosition: seams.getOverlayPosition,
		saveOverlayPosition: seams.saveOverlayPosition,
		getTrackingTagSuggestions: vi.fn(async () => []),
		lockTrackingTag: vi.fn(),
		getManualMobSuggestions: vi.fn(async () => []),
		lockManualMob: vi.fn(),
		getSessionQuestLinkSuggestion: vi.fn(),
		decideSessionQuestLink: vi.fn(),
		updateSettings: seams.updateSettings,
	};
});

vi.mock('@tauri-apps/api/event', () => ({
	listen: seams.listen,
	emit: seams.emit,
}));

vi.mock('@tauri-apps/api/window', () => ({
	getCurrentWindow: () => seams.currentWindow,
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

vi.mock('$lib/statsCustomisation', () => ({
	overlayStats: seams.overlayStats,
}));

import OverlayPage from './+page.svelte';

const activeSnapshot = {
	status: 'active',
	session_id: 's1',
	elapsed: 60,
	kill_count: 2,
	weaponAttribution: 'trifecta',
	mobEntryMode: 'mob',
	currentMob: 'Atrox',
	trifectaAttribution: {
		activePresetId: 'p1',
		presetName: 'Hunting Set',
		presets: [
			{ id: 'p1', name: 'Hunting Set' },
			{ id: 'p2', name: 'Mining Set' },
		],
		smallWeapon: null,
		bigWeapon: null,
		healTool: null,
	},
};

function fireReady(topic: string, label: string): void {
	for (const cb of seams.listeners.get(topic) ?? []) {
		cb({ payload: { label } });
	}
}

async function mountAndClickTrigger(): Promise<InstanceType<typeof seams.FakeWebviewWindow>> {
	render(OverlayPage);
	const trigger = await screen.findByTitle('Hunting Set');
	trigger.click();

	// The popup webview is created hidden.
	await waitFor(() => {
		expect(seams.FakeWebviewWindow.instances).toHaveLength(1);
	});
	const popup = seams.FakeWebviewWindow.instances[0];
	expect(popup.label).toBe('overlay-menu');
	expect(popup.options.visible).toBe(false);
	return popup;
}

async function mountAndOpenTrifectaMenu(): Promise<InstanceType<typeof seams.FakeWebviewWindow>> {
	const popup = await mountAndClickTrigger();
	// Complete Tauri's creation handshake.
	await act(async () => {
		popup.onceHandlers.get('tauri://created')?.({});
	});
	return popup;
}

beforeEach(() => {
	seams.listeners.clear();
	seams.FakeWebviewWindow.instances = [];
	seams.FakeWebviewWindow.getByLabel.mockResolvedValue(null);
	seams.getTrackingSnapshot.mockResolvedValue(activeSnapshot);
});

afterEach(() => {
	vi.useRealTimers();
});

describe('overlay popup readiness handshake', () => {
	it('withholds the show sequence until the popup route reports ready', async () => {
		const popup = await mountAndOpenTrifectaMenu();

		// Created but not ready: the show payload must not have been emitted and
		// the window must remain hidden.
		expect(popup.emit).not.toHaveBeenCalled();
		expect(popup.show).not.toHaveBeenCalled();

		// A ready event from the WRONG window label must not satisfy the gate.
		await act(async () => {
			fireReady('overlay-menu:ready', 'some-other-window');
		});
		expect(popup.show).not.toHaveBeenCalled();

		// The popup route reports ready: size, position, show payload, reveal.
		await act(async () => {
			fireReady('overlay-menu:ready', 'overlay-menu');
		});
		await waitFor(() => {
			expect(popup.show).toHaveBeenCalledTimes(1);
		});
		expect(popup.setSize).toHaveBeenCalled();
		expect(popup.setPosition).toHaveBeenCalled();
		expect(popup.emit).toHaveBeenCalledWith(
			'overlay-menu:show',
			expect.objectContaining({
				kind: 'trifecta',
				options: [
					{ id: 'p1', name: 'Hunting Set', active: true },
					{ id: 'p2', name: 'Mining Set', active: false },
				],
			}),
		);
		// The trifecta open path requests focus for keyboard navigation.
		expect(popup.setFocus).toHaveBeenCalled();
		// The sequence ORDER is the contract: size and position settle before
		// the show payload goes out, and the window is revealed only after it.
		const order = (mock: { mock: { invocationCallOrder: number[] } }) =>
			mock.mock.invocationCallOrder[0];
		expect(order(popup.setSize)).toBeLessThan(order(popup.emit));
		expect(order(popup.setPosition)).toBeLessThan(order(popup.emit));
		expect(order(popup.emit)).toBeLessThan(order(popup.show));
		expect(order(popup.show)).toBeLessThan(order(popup.setFocus));
		// The trigger reflects the open menu.
		expect(screen.getByTitle('Hunting Set').getAttribute('aria-expanded')).toBe('true');
	});

	it('times out into a rendered launch error when readiness never arrives', async () => {
		// Mount and click on real timers (the async render machinery needs them),
		// then fake the clock BEFORE completing the creation handshake: the 3s
		// readiness race registers its timeout after creation resolves, so it
		// lands on the faked clock and can be driven deterministically.
		const popup = await mountAndClickTrigger();
		vi.useFakeTimers();
		await act(async () => {
			popup.onceHandlers.get('tauri://created')?.({});
		});

		// No ready event: the 3s readiness race must reject and surface.
		await act(async () => {
			await vi.advanceTimersByTimeAsync(3100);
		});

		expect(popup.show).not.toHaveBeenCalled();
		expect(screen.getByText('Popup route did not become ready')).toBeTruthy();
		expect(screen.getByTitle('Hunting Set').getAttribute('aria-expanded')).toBe('false');
	});

	it('reuses the existing popup window and skips the handshake when already created', async () => {
		const existing = new seams.FakeWebviewWindow('overlay-menu', {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);

		render(OverlayPage);
		const trigger = await screen.findByTitle('Hunting Set');
		trigger.click();

		await waitFor(() => {
			expect(existing.show).toHaveBeenCalledTimes(1);
		});
		// No second window was constructed for the already-live popup.
		expect(seams.FakeWebviewWindow.instances).toHaveLength(0);
		expect(existing.emit).toHaveBeenCalledWith(
			'overlay-menu:show',
			expect.objectContaining({ kind: 'trifecta' }),
		);
	});
});
