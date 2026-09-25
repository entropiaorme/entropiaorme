/**
 * Dashboard guide-demo view model: the guide-only overlay-spawn and fake
 * armour-popup state, the demo tracking projection, the widgets-tab
 * snapshot/restore across the guide lifecycle, and the demoApi surface the
 * dashboard route registers. Pure guide plumbing; nothing here runs while a
 * tour is not driving it.
 */

import { getTrackingSnapshot, type TrackingLive } from '$lib/api';
import { getDemoApi } from '$lib/guide/state.svelte';
import type { DemoApi } from '$lib/guide/types';
import { resetArmourSvg, triggerArmourDrag, triggerArmourFlash } from './armourSvgDemo';
import type { StatsGridModel, StatsSnapshot } from './statsGridModel.svelte';

/** The fake armour-cost popup's fixed width, shared with the demo mount so
 *  the popup centres under the strip's Cost button. */
export const ARMOUR_POPUP_WIDTH = 220;

export function createGuideDemoModel(statsGrid: StatsGridModel) {
	// The overlay-strip's display fields, projected from the demo tracking
	// snapshot. Drives the inline OverlayStrip mount that replaces the spawn
	// screenshot during the dashboard guide's overlay-spawn step.
	let demoTrackingLive = $state<TrackingLive | null>(null);
	// Lifecycle phase for the demo overlay strip. The overlay-spawn card
	// mounts the strip in 'idle' first, then animates a cursor click on the
	// strip's TRACK button to flip to 'active' (mid-hunt readout).
	let overlayStripPhase = $state<'idle' | 'active'>('idle');
	// Fake armour-cost popup visibility + position. The real armour-cost UI
	// lives in a separate Tauri webview window which the dashboard's inline
	// strip cannot reach, so the guide renders a styled stand-in. Position is
	// computed from the Cost button's bounding rect at show time so the
	// stand-in lands directly below the button.
	let demoArmourPopupVisible = $state(false);
	// Two-state popup body: false = initial (label + Record + Enter
	// manually); true = post-record confirmation. Auto-resets to false
	// whenever the popup is hidden so the next show starts clean.
	let demoArmourPopupRecorded = $state(false);
	let armourPopupTop = $state(0);
	let armourPopupLeft = $state(0);
	// When true, the Recent Events + DashboardWidgets islands hide and the
	// fake overlay-window spawn mounts in their place. Driven by the surface
	// module's setOverlayDemoVisible demoApi call.
	let demoOverlayVisible = $state(false);

	// DashboardWidgets active-tab snapshot across the guide lifecycle: the
	// guide opens on 'pulse' regardless of where the user left it; their
	// choice returns on close.
	let snapshotWidgetsTab: string | undefined;

	function syncArmourPopupPosition() {
		const btn = document.querySelector<HTMLElement>(
			'[data-guide-anchor="overlay-armour-cost-btn"]',
		);
		if (!btn) return;
		const rect = btn.getBoundingClientRect();
		armourPopupTop = rect.bottom + 8;
		armourPopupLeft = rect.left + rect.width / 2 - ARMOUR_POPUP_WIDTH / 2;
	}

	/** Project the demo tracking snapshot into the strip's render shape while
	 *  the guide is active; clear it on close. */
	async function refreshDemoTracking(active: boolean) {
		try {
			const live = active ? await getTrackingSnapshot() : null;
			demoTrackingLive = live && {
				status: live.status ?? 'idle',
				elapsed: live.elapsed,
				currentTool: live.currentTool,
				currentMob: live.currentMob,
				sessionName: live.sessionName,
				skillBoostPercent: live.skillBoostPercent,
				currentActivity: live.currentActivity,
				hotbarKeysEnabled: live.hotbarKeysEnabled,
				weaponGuardrail: live.weaponGuardrail,
				unpricedShots: live.unpricedShots,
				repairOcrEnabled: live.repairOcrEnabled,
			};
		} catch {
			// Transient demo read failure: the tour degrades to no inline strip.
		}
	}

	/** Snapshot the widgets tab on guide-open + force 'pulse'; restore the
	 *  pre-guide tab on close. Microtask defer for the restore so the widget
	 *  has a tick to remount before receiving setTab (its mount gate flips
	 *  when the guide deactivates). */
	function syncWidgetsTab(active: boolean) {
		if (active && snapshotWidgetsTab === undefined) {
			const wapi = getDemoApi('dashboard-widgets') as {
				setTab?: (id: string) => void;
				getTab?: () => string;
			};
			snapshotWidgetsTab = wapi.getTab?.() ?? 'pulse';
			wapi.setTab?.('pulse');
		} else if (!active && snapshotWidgetsTab !== undefined) {
			const restored = snapshotWidgetsTab;
			snapshotWidgetsTab = undefined;
			queueMicrotask(() => {
				const wapi = getDemoApi('dashboard-widgets') as {
					setTab?: (id: string) => void;
				};
				wapi.setTab?.(restored);
			});
		}
	}

	/** The 'dashboard' demoApi surface the route registers on mount. */
	function demoApi(): DemoApi {
		return {
			setOverlayDemoVisible: (visible: boolean) => {
				demoOverlayVisible = visible;
				// Reset the lifecycle phase on (un)mount so each guide opening
				// starts the strip in idle regardless of where the prior
				// session left it.
				overlayStripPhase = 'idle';
			},
			setOverlayDemoTrackingStarted: (started: boolean) => {
				overlayStripPhase = started ? 'active' : 'idle';
			},
			setOverlayArmourPopupVisible: (visible: boolean) => {
				// Sync position before mounting so the stand-in lands under
				// the Cost button on first frame rather than flashing at the
				// prior coordinates.
				if (visible) syncArmourPopupPosition();
				else demoArmourPopupRecorded = false; // reset body so next show starts clean
				demoArmourPopupVisible = visible;
			},
			setOverlayArmourPopupRecorded: (recorded: boolean) => {
				demoArmourPopupRecorded = recorded;
			},
			snapshotStats: () => statsGrid.snapshotStats(),
			restoreStats: (snap: StatsSnapshot) => statsGrid.restoreStats(snap),
			setDemoStatsBaseline: (overrides?: Record<string, boolean>) =>
				statsGrid.setDemoStatsBaseline(overrides),
			toggleDemoStatPill: (surface: 'dashboard' | 'overlay', statId: string) =>
				statsGrid.toggleDemoStatPill(surface, statId),
			reorderDemoStat: (fromFilteredIdx: number, toFilteredIdx: number) =>
				statsGrid.reorderDemoStat(fromFilteredIdx, toFilteredIdx),
			setDragVisualIndex: (idx: number | null) => statsGrid.setDragVisualIndex(idx),
			triggerArmourDrag,
			triggerArmourFlash,
			resetArmourSvg,
		};
	}

	return {
		get demoTrackingLive() {
			return demoTrackingLive;
		},
		get overlayStripPhase() {
			return overlayStripPhase;
		},
		get demoArmourPopupVisible() {
			return demoArmourPopupVisible;
		},
		get demoArmourPopupRecorded() {
			return demoArmourPopupRecorded;
		},
		get armourPopupTop() {
			return armourPopupTop;
		},
		get armourPopupLeft() {
			return armourPopupLeft;
		},
		get demoOverlayVisible() {
			return demoOverlayVisible;
		},

		refreshDemoTracking,
		syncWidgetsTab,
		demoApi,
	};
}

export type GuideDemoModel = ReturnType<typeof createGuideDemoModel>;
