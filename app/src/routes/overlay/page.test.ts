// @vitest-environment happy-dom

import { act, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

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
		getSessionDefinitions: vi.fn(async () => [
			{ id: '1', name: 'ARIS Dailies' },
			{ id: '2', name: 'Solo Run' },
		]),
		decideWeaponMismatch: vi.fn(async () => true),
		getOverlayPosition: vi.fn(async () => ({ x: null, y: null })),
		saveOverlayPosition: vi.fn(async () => {}),
		updateSettings: vi.fn(async () => ({})),
		currentWindow: {
			startDragging: vi.fn(async () => {}),
			setSize: vi.fn(async () => {}),
			outerPosition: vi.fn(async () => ({ x: 0, y: 0 })),
			scaleFactor: vi.fn(async () => 1),
		},
		overlayStats: { current: [] as never[] },
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
		setSessionConfig: vi.fn(),
		getSessionDefinitions: seams.getSessionDefinitions,
		selectDefinition: vi.fn(),
		getManualMobSuggestions: vi.fn(async () => []),
		lockManualMob: vi.fn(),
		getActivityOptions: vi.fn(async () => ({
			definitionId: null,
			definitionName: null,
			visible: false,
			adHocSegments: false,
			readyCount: 0,
			options: [],
			active: [],
		})),
		activateActivity: vi.fn(),
		deactivateActivity: vi.fn(),
		beginQuestHandIn: vi.fn(async () => ({
			questId: 1,
			questName: 'AI Daily',
			waiting: false,
			candidate: null,
		})),
		getQuests: vi.fn(async () => []),
		CONSUMABLES_TOPIC: 'consumables:updated',
		getConsumableDoses: vi.fn(async () => ({
			now: 0,
			doses: [],
			reloadSpeed: {
				equippedPercent: 0,
				consumedPercent: 0,
				inEffectPercent: 0,
				itemLimitPercent: 15,
				consumedLimitPercent: 20,
				totalLimitPercent: 30,
			},
			options: [],
		})),
		startConsumableDose: vi.fn(),
		removeConsumableDose: vi.fn(),
		restoreConsumableDose: vi.fn(),
		updateSettings: seams.updateSettings,
		decideWeaponMismatch: seams.decideWeaponMismatch,
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

// Only the state seam is stubbed; the pure `scopedStats` filter stays
// real so the strip narrows its pills the way it does in the app.
vi.mock('$lib/statsCustomisation.svelte', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/statsCustomisation.svelte')>()),
	overlayStats: seams.overlayStats,
}));

vi.mock('$lib/statsScope.svelte', () => ({
	statsScope: { current: 'instance' },
	setStatsScope: vi.fn(),
	initStatsScope: vi.fn().mockResolvedValue(undefined),
	STATS_SCOPE_CHANGED_EVENT: 'stats-scope-changed',
}));

import OverlayPage from './+page.svelte';

// happy-dom has no Web Animations API, and Svelte drives transition
// completion (and outro element removal) through `animation.onfinish`; the
// stub finishes instantly on a microtask so the notice rail's fades settle.
beforeAll(() => {
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

const activeSnapshot = {
	status: 'active',
	session_id: 's1',
	elapsed: 60,
	kill_count: 2,
	sessionName: 'ARIS Dailies',
	sessionDefinitionId: '1',
	currentMob: 'Atrox',
	currentTool: 'Sollomate Opalo',
};

// Idle, so the session picker is a live control: the popup vehicle for the
// readiness handshake below.
const idleSnapshot = {
	status: 'idle',
	sessionName: 'ARIS Dailies',
	sessionDefinitionId: '1',
};

const PICKER_TITLE = 'ARIS Dailies; pick the session for the next run';

function fireReady(topic: string, label: string): void {
	for (const cb of seams.listeners.get(topic) ?? []) {
		cb({ payload: { label } });
	}
}

async function mountAndClickTrigger(): Promise<InstanceType<typeof seams.FakeWebviewWindow>> {
	seams.getTrackingSnapshot.mockResolvedValue(idleSnapshot);
	render(OverlayPage);
	const trigger = await screen.findByTitle(PICKER_TITLE);
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

async function mountAndOpenSessionMenu(): Promise<InstanceType<typeof seams.FakeWebviewWindow>> {
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
		const popup = await mountAndOpenSessionMenu();

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
				kind: 'definition',
				definitions: [
					{ id: '1', name: 'ARIS Dailies', selected: true },
					{ id: '2', name: 'Solo Run', selected: false },
				],
			}),
		);
		// The picker's open path requests focus for keyboard navigation.
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
		expect(screen.getByTitle(PICKER_TITLE).getAttribute('aria-expanded')).toBe('true');
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
		expect(screen.getByTitle(PICKER_TITLE).getAttribute('aria-expanded')).toBe('false');
	});

	it('reuses the existing popup window and skips the handshake when already created', async () => {
		const existing = new seams.FakeWebviewWindow('overlay-menu', {});
		seams.FakeWebviewWindow.instances = [];
		seams.FakeWebviewWindow.getByLabel.mockResolvedValue(existing);

		seams.getTrackingSnapshot.mockResolvedValue(idleSnapshot);
		render(OverlayPage);
		const trigger = await screen.findByTitle(PICKER_TITLE);
		trigger.click();

		await waitFor(() => {
			expect(existing.show).toHaveBeenCalledTimes(1);
		});
		// No second window was constructed for the already-live popup.
		expect(seams.FakeWebviewWindow.instances).toHaveLength(0);
		expect(existing.emit).toHaveBeenCalledWith(
			'overlay-menu:show',
			expect.objectContaining({ kind: 'definition' }),
		);
	});
});

describe('the weapon guardrail cue', () => {
	it('sends the decision and re-reads the readout', async () => {
		const withCue = {
			...activeSnapshot,
			weaponGuardrail: {
				hotbarTool: 'Sollomate Opalo',
				recordingTool: 'Korss H400 (L)',
				since: 1_784_600_000,
				shots: 2,
			},
		};
		seams.getTrackingSnapshot.mockResolvedValue(withCue);
		render(OverlayPage);
		const confirm = await screen.findByLabelText('Confirm Korss H400 (L)');
		seams.getTrackingSnapshot.mockResolvedValue({
			...activeSnapshot,
			currentTool: 'Korss H400 (L)',
		});
		confirm.click();
		await waitFor(() => {
			expect(seams.decideWeaponMismatch).toHaveBeenCalledWith('confirm');
		});
		await waitFor(() => {
			expect(screen.queryByTestId('weapon-guardrail-alert')).toBeNull();
		});
		expect(screen.getByText('Korss H400 (L)')).toBeTruthy();
	});

	it('reports a decision that could not be saved', async () => {
		seams.getTrackingSnapshot.mockResolvedValue({
			...activeSnapshot,
			weaponGuardrail: {
				hotbarTool: 'Sollomate Opalo',
				recordingTool: 'Korss H400 (L)',
				since: 1_784_600_000,
				shots: 2,
			},
		});
		seams.decideWeaponMismatch.mockRejectedValueOnce(
			new Error('The decision could not be saved; nothing changed'),
		);
		render(OverlayPage);
		(await screen.findByLabelText('Keep Sollomate Opalo')).click();
		expect(
			await screen.findByText('The decision could not be saved; nothing changed'),
		).toBeTruthy();
	});
});

describe('snapshot fields reaching the strip', () => {
	it('shows the held healing tool from a snapshot that reports one', async () => {
		// Guards the mapping itself: the strip's healer branch keys on
		// currentToolKind, so a field the backend emits but the page never
		// copies leaves the branch unreachable in the built app.
		seams.getTrackingSnapshot.mockResolvedValue({
			...activeSnapshot,
			currentTool: 'Restoration Chip',
			currentToolKind: 'healing',
		});
		render(OverlayPage);
		expect(await screen.findByText('Restoration Chip')).toBeTruthy();
	});
});

describe('armour-cost popup', () => {
	it('hands the popup its state and leaves sizing, placement and reveal to the popup', async () => {
		render(OverlayPage);
		const trigger = await screen.findByTitle("Record an armour repair or a limited set's reading");
		trigger.click();

		await waitFor(() => {
			expect(seams.FakeWebviewWindow.instances).toHaveLength(1);
		});
		const popup = seams.FakeWebviewWindow.instances[0];
		expect(popup.label).toBe('overlay-armour-cost');
		expect(popup.options.visible).toBe(false);

		await act(async () => {
			popup.onceHandlers.get('tauri://created')?.({});
		});
		await act(async () => {
			fireReady('overlay-armour-cost:ready', 'overlay-armour-cost');
		});

		await waitFor(() => {
			expect(popup.emit).toHaveBeenCalledWith('overlay-armour-cost:show', {
				repairOcrEnabled: false,
				anchor: { centerX: expect.any(Number), top: expect.any(Number) },
			});
		});
		await waitFor(() => {
			expect(trigger.getAttribute('aria-expanded')).toBe('true');
		});
		// The popup measures its panel and sizes, positions, reveals and
		// focuses itself from the payload; a host-side reveal would flash the
		// window at a stale location before it has measured anything.
		expect(popup.show).not.toHaveBeenCalled();
		expect(popup.setSize).not.toHaveBeenCalled();
		expect(popup.setPosition).not.toHaveBeenCalled();
		expect(popup.setFocus).not.toHaveBeenCalled();
	});
});
