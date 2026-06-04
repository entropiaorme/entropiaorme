import { getDemoApi, guideState } from '../state.svelte';
import type { GuideSurface } from '../types';
import type { StatPref } from '$lib/statsCustomisation';

/** Dashboard-surface demoApi method names (declared here for documentation). */
type DashboardDemoApi = {
	/**
	 * Toggle the guide-only inline overlay spawn. When true, the page's
	 * Recent Events + DashboardWidgets sections hide and the inline
	 * OverlayStrip mounts in their place. Mirrors the character surface's
	 * setFakeScannerVisible pattern (fixed-positioned + flex-centred +
	 * pointer-events-none element rendered at the route root). Setting
	 * to true also resets the lifecycle phase to 'idle', so each fresh
	 * spawn lands the strip in its idle (no-session) state.
	 */
	setOverlayDemoVisible(visible: boolean): void;
	/**
	 * Flip the demo strip's lifecycle phase. False → idle (synthesised
	 * no-session TrackingLive, em-dash stat pills); true → active (real
	 * mid-hunt readout via /demo/tracking/snapshot). The card's play() loop
	 * starts idle, animates a cursor click on the strip's TRACK button,
	 * then sets this to true to flip the strip to mid-hunt.
	 */
	setOverlayDemoTrackingStarted(started: boolean): void;
	/**
	 * Toggle the guide-only fake armour-cost popup that mirrors
	 * RepairCostPanel's initial state (label + Record + Close). Real
	 * popup lives in a separate Tauri webview window which the inline
	 * dashboard strip cannot reach, so the guide renders a styled
	 * stand-in positioned below the strip's Cost button. Setting to
	 * true syncs the popup's coordinates to the Cost button's bounding
	 * rect at call time. Setting to false also resets the recorded-body
	 * flag so the next show starts in the initial state.
	 */
	setOverlayArmourPopupVisible(visible: boolean): void;
	/**
	 * Flip the popup's body between initial (Record + Close) and the
	 * post-record confirmation ("Cost recorded: 1.23 PED"). Fires
	 * synchronised with the SVG flash so the screen-capture and the
	 * recorded value read as one beat on screen.
	 */
	setOverlayArmourPopupRecorded(recorded: boolean): void;
	/**
	 * Trigger the SVG's window-drag animation (CSS transition on the
	 * `armour-svg-window` element). Fires alongside the cursor's Cost
	 * click so the SVG begins moving the repair-terminal window toward
	 * the monitor's bottom-right corner.
	 */
	triggerArmourDrag(): void;
	/**
	 * Trigger the SVG's one-shot flash animation over the docked window
	 * position. Fires synchronised with the cursor's Record click; the
	 * flash + Record click together communicate "screen capture taken".
	 */
	triggerArmourFlash(): void;
	/**
	 * Snap the SVG back to its starting state (window at top-left, flash
	 * invisible) without animation. Run in the loop's gap phase so each
	 * iteration restarts the drag sequence from a known position.
	 */
	resetArmourSvg(): void;
	/** Snapshot dashboardStats + overlayStats for restore on step-exit / replay. */
	snapshotStats(): { dashboard: StatPref[]; overlay: StatPref[] };
	/** Restore both stat stores from a snapshot taken via snapshotStats. */
	restoreStats(snap: { dashboard: StatPref[]; overlay: StatPref[] }): void;
	/** Reset both stat stores to library default prefs (transient, no persist).
	 *  Optional overrides flip specific stat-ids' enabled flags. */
	setDemoStatsBaseline(overrides?: Record<string, boolean>): void;
	/** Toggle a pill's enabled flag on the named surface (transient, no persist). */
	toggleDemoStatPill(surface: 'dashboard' | 'overlay', statId: string): void;
	/** Reorder dashboardStats: moves stat at filtered idx `from` to filtered idx `to`. */
	reorderDemoStat(fromFilteredIdx: number, toFilteredIdx: number): void;
	/** Set the page's dragFilteredIndex $state so the cell renders its drag visual. */
	setDragVisualIndex(idx: number | null): void;
};

/** Module-scope snapshot for the modular-stats card; restored in resetDemo. */
let statsSnapshot: { dashboard: StatPref[]; overlay: StatPref[] } | null = null;

function dashboardApi(): Partial<DashboardDemoApi> {
	return getDemoApi('dashboard') as Partial<DashboardDemoApi>;
}

/** Sub-API exposed by DashboardWidgets.svelte (sub-API composition pattern). */
type DashboardWidgetsApi = { setTab(id: string): void };

function widgetsApi(): Partial<DashboardWidgetsApi> {
	return getDemoApi('dashboard-widgets') as Partial<DashboardWidgetsApi>;
}

/** Sleep in 200ms chunks so loop iterations can break promptly on Next / Back / Close. */
async function abortableWait(ms: number, stillActive: () => boolean): Promise<boolean> {
	const end = Date.now() + ms;
	while (Date.now() < end) {
		await new Promise((r) => setTimeout(r, Math.min(200, end - Date.now())));
		if (!stillActive()) return false;
	}
	return true;
}

export const dashboardSurface: GuideSurface = {
	id: 'dashboard',
	title: 'Dashboard',
	beforeStart(demoApi) {
		const api = demoApi as Partial<DashboardDemoApi>;
		// Reset per-open: overlay-spawn must not leak from a prior guide session
		// (e.g. user dismissed at the overlay step then re-opens: they should
		// land on the intro with the normal page composition, not the fake spawn).
		api.setOverlayDemoVisible?.(false);
		api.setOverlayDemoTrackingStarted?.(false);
	},
	steps: [
		{
			id: 'dashboard-intro',
			anchor: () => document.querySelector<HTMLElement>('[data-guide-anchor="dashboard-area"]'),
			placement: 'bottom-centre',
			placementOffset: { y: 26 },
			prose: {
				title: 'Dashboard',
				body: [
					{ kind: 'p', text: 'The Dashboard is the main page to have open while you are playing. It has 3 areas:' },
					{ kind: 'ul', items: [
						'The modular stats',
						'The recent events',
						'The dashboard widgets'
					] }
				],
				note: 'Note: Guide uses demo data.'
			}
		},
		{
			id: 'overlay-spawn',
			// 2-phase priority cascade. Initially the fake spawn
			// is not mounted, so the closure falls through to the Overlay button;
			// the cutout highlights the button while play() slides the cursor to it.
			// After play() clicks the button and flips setOverlayDemoVisible(true),
			// the fake-spawn img mounts; on the next 120ms anchor poll the spawn
			// element wins (offsetParent !== null) and the cutout transitions over
			// the 350ms d-tween to highlight the screenshot. On loop reset the spawn
			// unmounts and the cascade falls back to the button for the next sweep.
			anchor: () => {
				const spawn = document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-overlay-spawn"]'
				);
				if (spawn && spawn.offsetParent !== null) return spawn;
				return document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-overlay-btn"]'
				);
			},
			placement: 'bottom-centre',
			// Anchored to the always-mounted slot wrapper (not the conditionally-
			// visible strip itself) so the prose card is positioned at the
			// strip's eventual coords from step entry. Avoids the jarring snap
			// up from viewport-bottom that the prior dashboard-overlay-spawn
			// anchor caused during Phase 1's pre-mount window + the inter-loop
			// gap. The slot has the strip's exact dimensions (the strip mounts
			// inside, just opacity-0 when hidden), so flush-below + centred
			// placement lands correctly in all phases.
			placementAnchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="dashboard-overlay-spawn-slot"]'
			),
			prose: {
				title: 'Overlay',
				body: 'The Overlay is the main control area for tracking.'
			},
			async play({ cursor, demoApi }) {
				const stepIdx = guideState.currentStepIndex;
				const stillActive = () =>
					guideState.isActive && guideState.currentStepIndex === stepIdx;
				const api = demoApi as Partial<DashboardDemoApi>;
				// Explicit start-state reset: overlay hidden, lifecycle idle.
				// Forward 1→2 inherits false/false from beforeStart so this
				// is a no-op; back-nav 3→2 inherits true/active because
				// downstream cards no longer revert state in resetDemo (so
				// transitions between overlay-state cards don't flicker), so
				// the reset here re-establishes the lifecycle demo's starting
				// position regardless of where the user came from.
				api.setOverlayDemoVisible?.(false);
				api.setOverlayDemoTrackingStarted?.(false);
				while (stillActive()) {
					// === Phase 1: cursor → dashboard Overlay button, click, strip mounts idle ===
					const btn = document.querySelector<HTMLElement>(
						'[data-guide-anchor="dashboard-overlay-btn"]'
					);
					if (!btn) {
						if (!(await abortableWait(500, stillActive))) break;
						continue;
					}
					const btnRect = btn.getBoundingClientRect();
					// Cursor enters from lower-left of the button so the slide sweeps in
					// diagonally. Re-resolved each iter so window-resize is tracked.
					const startX = Math.max(0, btnRect.left - 260);
					const startY = btnRect.top + 220;
					const startRect = new DOMRect(startX, startY, 0, 0);
					// Snap-to-origin via the {duration: 0} bypass path (writes inline
					// transform directly). Visually positions the cursor at the start.
					await cursor.moveTo(startRect, { duration: 0 });
					cursor.show();
					if (!stillActive()) break;
					// Explicit `from` keyframes so Motion tweens from the snap origin
					// instead of its cached x/y (which on iter 2+ would still point at
					// the button from the prior iter, popping the cursor straight to
					// the target with no visible slide).
					await cursor.moveTo(btn, {
						duration: 900,
						from: { x: startX, y: startY }
					});
					if (!stillActive()) break;
					await cursor.clickRipple();
					cursor.hide();
					if (!stillActive()) break;
					// setOverlayDemoVisible(true) mounts the strip and resets the
					// lifecycle phase to 'idle' (per the dashboard page's onMount
					// handler) so the strip lands in its no-session state first.
					api.setOverlayDemoVisible?.(true);
					// 1.5s dwell so the idle affordances (TRACK button, MOB/TAG
					// segmented control, mob input, trifecta dropdown, em-dash
					// stat pills) dwell on screen before the cursor moves to TRACK.
					if (!(await abortableWait(1500, stillActive))) break;

					// === Phase 2: cursor → strip's TRACK button, click, strip flips active ===
					// The .start-btn class uniquely identifies the strip's green
					// TRACK button (the real overlay's strip lives in a separate
					// Tauri webview, so there's no DOM collision in the dashboard
					// page). Lookup is per-iter so window-resize re-resolves coords.
					const trackBtn = document.querySelector<HTMLElement>('.overlay-strip .start-btn');
					if (!trackBtn) {
						// Strip didn't mount in time; back out cleanly and let the
						// next iteration retry.
						api.setOverlayDemoVisible?.(false);
						if (!(await abortableWait(200, stillActive))) break;
						continue;
					}
					const trackRect = trackBtn.getBoundingClientRect();
					// Cursor enters from below-left of the TRACK button, mirroring
					// the geometry of the first sweep but at a smaller scale
					// (~180px left, ~100px below) since the cursor is already
					// somewhere in the lower-left quadrant after Phase 1's hide.
					const trackStartX = Math.max(40, trackRect.left - 180);
					const trackStartY = trackRect.top + 100;
					const trackStartRect = new DOMRect(trackStartX, trackStartY, 0, 0);
					await cursor.moveTo(trackStartRect, { duration: 0 });
					cursor.show();
					if (!stillActive()) break;
					await cursor.moveTo(trackBtn, {
						duration: 900,
						from: { x: trackStartX, y: trackStartY }
					});
					if (!stillActive()) break;
					await cursor.clickRipple();
					cursor.hide();
					if (!stillActive()) break;
					api.setOverlayDemoTrackingStarted?.(true);
					// 5s dwell on the active mid-hunt readout: the full payoff
					// of the lifecycle demo.
					if (!(await abortableWait(5000, stillActive))) break;

					// === Phase R: reset to unmounted + idle, gap, loop ===
					api.setOverlayDemoVisible?.(false);
					api.setOverlayDemoTrackingStarted?.(false);
					// 700ms gap before the next sweep so the section-swap doesn't read as a flicker.
					if (!(await abortableWait(700, stillActive))) break;
				}
				// Post-loop cleanup runs ONLY for guide-close. For Next / Back /
				// Replay, resetDemo (called before the new step's play()) is the
				// authoritative cleanup. Unconditional cleanup here would race
				// the next card's play(); Phase-R's 700ms abortableWait can
				// take up to 200ms to detect step-exit, so the cleanup block
				// can fire AFTER the next card has already called setVisible/
				// setTracking to true, hiding what the next card just set.
				// `closeGuide` runs resetDemo first then sets isActive=false,
				// so this branch only triggers on close (where resetDemo has
				// already done the work; the redundant calls are safe).
				if (!guideState.isActive) {
					dashboardApi().setOverlayDemoVisible?.(false);
					dashboardApi().setOverlayDemoTrackingStarted?.(false);
					cursor.hide();
				}
			},
			resetDemo() {
				// Belt-and-braces: revert spawn visibility AND lifecycle phase on
				// step exit (Next / Back / Close / Replay all call resetDemo per
				// engine.ts).
				dashboardApi().setOverlayDemoVisible?.(false);
				dashboardApi().setOverlayDemoTrackingStarted?.(false);
			}
		},
		{
			id: 'overlay-mob-section',
			// Anchor on the OverlayStrip's MOB/TAG + mob-name + release-x parent
			// wrapper. Single-element anchor (vs. DOMRect[]) because the affordances
			// sit in the same parent flex container; codex-overview's DOMRect[]
			// pattern is reserved for non-adjacent regions.
			anchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="overlay-mob-section"]'
			),
			placement: 'bottom-centre',
			// Same placement slot as overlay-spawn so the prose card stays seated
			// beneath the strip across the Next-transition. The slot wrapper is
			// always-mounted while guideState.isActive + demoTrackingLive, and the
			// inner strip's outer div renders in both idle and active variants
			// (status='idle' + lastSessionId=null hits the same template branch as
			// status='active'), so the cutout d-attribute tweens smoothly from the
			// prior card's strip-wrapper shape to this card's mob-section shape.
			placementAnchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="dashboard-overlay-spawn-slot"]'
			),
			prose: {
				title: 'Mob and tag',
				body: [
					{ kind: 'p', text: 'Track sessions by mob name or a free-text tag. Later analytics build on this.' },
					{ kind: 'p', text: 'Tags are more versatile when a hunt involves different mobs, so the analysis describes the overall activity.' }
				],
				note: 'Note: OCR auto-detection was attempted but proved too inconsistent. The manual Mob/Tag approach is less flexible but more reliable.'
			},
			async play({ demoApi }) {
				// Re-establish (or no-op) the active lifecycle state: strip
				// mounted, lifecycle 'active'. Idempotent: calling
				// setOverlayDemoVisible(true) when already visible just
				// re-fires the page-handler side-effect (lifecycle → idle),
				// which the immediate setOverlayDemoTrackingStarted(true)
				// flips back to active. Forward 2→3 inherits the brief 80ms
				// hidden window from overlay-spawn's resetDemo (smoothed by
				// the cutout transition). Forward 4→3 (none today) or
				// back-nav 4→3 inherits visible+active directly, so this
				// becomes a clean no-op. No cursor work: the cursor is an
				// action indicator only, and this transition is an idempotent
				// state restore rather than a fresh action.
				const api = demoApi as Partial<DashboardDemoApi>;
				api.setOverlayDemoVisible?.(true);
				api.setOverlayDemoTrackingStarted?.(true);
			}
			// No resetDemo: the next card (overlay-equipment-section) also
			// wants the strip mounted active; reverting state here would
			// cause an 80ms flicker the next card immediately undoes.
			// closeGuide path is handled by the slot wrapper's
			// {#if guideState.isActive} gate, which unmounts on close.
		},
		{
			id: 'overlay-equipment-section',
			// Same anchor / placement / lifecycle posture as overlay-mob-section.
			// The strip stays mounted active across the Next-transition; the
			// cutout d-attribute tweens from the mob-section rect to the
			// equipment-section rect via the 350ms cubic-bezier.
			anchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="overlay-equipment-section"]'
			),
			placement: 'bottom-centre',
			placementAnchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="dashboard-overlay-spawn-slot"]'
			),
			prose: {
				title: 'Equipment',
				body: [
					{ kind: 'ul', items: [
						'Hotbar mode: shows the currently active tool',
						'Trifecta mode: choose which preset to use'
					] }
				],
				note: 'Note: See the equipment area for an intro to the Hotbar and Trifecta.'
			},
			async play({ demoApi }) {
				const api = demoApi as Partial<DashboardDemoApi>;
				api.setOverlayDemoVisible?.(true);
				api.setOverlayDemoTrackingStarted?.(true);
			}
			// No resetDemo: adjacent overlay-state cards (mob ↔ armour) own
			// the same active state, so reverting here would cause a flicker
			// the next card immediately undoes. See overlay-mob-section.
		},
		{
			id: 'overlay-armour-section',
			// 3-phase presence-driven anchor. Cutout highlights the armour
			// section in the strip throughout, AND extends to include the
			// fake popup once it mounts. DOMRect[] pierces both regions
			// independently via the SVG even-odd fill rule (multi-hole
			// cutout shape).
			anchor: () => {
				const section = document.querySelector<HTMLElement>(
					'[data-guide-anchor="overlay-armour-section"]'
				);
				const popup = document.querySelector<HTMLElement>(
					'[data-guide-anchor="overlay-armour-popup"]'
				);
				if (section && popup && popup.offsetParent !== null) {
					return [section.getBoundingClientRect(), popup.getBoundingClientRect()];
				}
				if (section) return section;
				return null;
			},
			placement: 'bottom-centre',
			placementAnchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="dashboard-overlay-spawn-slot"]'
			),
			// Push the prose card below the slot's natural placement to clear
			// the fake popup that mounts below the Cost button (popup sits
			// ~8px below the strip and is ~36px tall, total ~44px below
			// slot.bottom). 40px gets the card just below the popup without
			// excess gap.
			placementOffset: { y: 40 },
			prose: {
				title: 'Armour costs',
				body: [
					{ kind: 'p', text: 'Armour costs can be added to a session. You can type them in manually, or set up automatic screen capture (OCR) of your repair terminal cost.' },
					{ kind: 'p', text: 'Place the repair terminal at the bottom right of the screen and hit Record.' },
					{ kind: 'svg', svg: `<svg viewBox="0 0 220 150" width="220" height="150" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
  <defs>
    <style>
      .armour-svg-monitor { fill: none; stroke: rgba(255,255,255,0.18); stroke-width: 1.5; }
      .armour-svg-stand { fill: rgba(255,255,255,0.08); }
      .armour-svg-window {
        fill: rgb(99, 179, 237);
        opacity: 0.85;
        transition: transform 800ms cubic-bezier(0.4, 0, 0.2, 1);
      }
      .armour-svg-window.docked { transform: translate(132px, 62px); }
      .armour-svg-flash { fill: rgb(255, 255, 255); opacity: 0; }
      @keyframes armourFlash {
        0% { opacity: 0; }
        30% { opacity: 0.9; }
        100% { opacity: 0; }
      }
    </style>
  </defs>
  <rect class="armour-svg-monitor" x="6" y="6" width="208" height="118" rx="6"/>
  <rect class="armour-svg-stand" x="98" y="124" width="24" height="10" rx="1"/>
  <rect class="armour-svg-stand" x="78" y="134" width="64" height="4" rx="1"/>
  <rect class="armour-svg-window" id="armour-svg-window" x="14" y="14" width="64" height="44" rx="3"/>
  <rect class="armour-svg-flash" id="armour-svg-flash" x="146" y="76" width="64" height="44" rx="3"/>
</svg>` }
				],
				note: 'Note: Enable repair OCR in settings. Cost for (L) armour is work in progress.'
			},
			async play({ cursor, demoApi }) {
				// Looped sync: cursor → Cost click → fake popup mounts + SVG
				// window drags; cursor → Record click → SVG flash; reset; loop.
				// Drag is ~800ms; the 1200ms post-Cost dwell gives the drag time
				// to complete before the cursor leaves for Record. Flash is a
				// one-shot 500ms keyframe; the 1500ms post-flash dwell lets the
				// user absorb the "screen capture taken" beat before reset.
				const stepIdx = guideState.currentStepIndex;
				const stillActive = () =>
					guideState.isActive && guideState.currentStepIndex === stepIdx;
				const api = demoApi as Partial<DashboardDemoApi>;
				// Re-establish lifecycle state (same handoff as overlay-mob /
				// overlay-equipment cards). Strip must be active so the Cost
				// button is enabled + the armour-section anchor resolves.
				api.setOverlayDemoVisible?.(true);
				api.setOverlayDemoTrackingStarted?.(true);
				// Give the strip a tick to mount before the anchor lookup
				// resolves the Cost button.
				if (!(await abortableWait(200, stillActive))) return;
				while (stillActive()) {
					// === Phase 1: cursor → Cost button, click, popup mounts + drag starts ===
					const costBtn = document.querySelector<HTMLElement>(
						'[data-guide-anchor="overlay-armour-cost-btn"]'
					);
					if (!costBtn) {
						if (!(await abortableWait(500, stillActive))) break;
						continue;
					}
					const costRect = costBtn.getBoundingClientRect();
					const startX = Math.max(40, costRect.left - 220);
					const startY = costRect.top + 180;
					await cursor.moveTo(new DOMRect(startX, startY, 0, 0), { duration: 0 });
					cursor.show();
					if (!stillActive()) break;
					await cursor.moveTo(costBtn, {
						duration: 900,
						from: { x: startX, y: startY }
					});
					if (!stillActive()) break;
					await cursor.clickRipple();
					cursor.hide();
					if (!stillActive()) break;
					api.setOverlayArmourPopupVisible?.(true);
					api.triggerArmourDrag?.();
					// 1.2s dwell: 800ms drag + 400ms slack so the docked window
					// settles before the cursor leaves for Record.
					if (!(await abortableWait(1200, stillActive))) break;

					// === Phase 2: cursor → Record button, click, flash fires, popup closes ===
					const recordBtn = document.querySelector<HTMLElement>(
						'[data-guide-anchor="overlay-armour-record-btn"]'
					);
					if (!recordBtn) {
						// Popup didn't mount in time; back out and let the next
						// iteration retry from a clean slate.
						api.setOverlayArmourPopupVisible?.(false);
						api.resetArmourSvg?.();
						if (!(await abortableWait(200, stillActive))) break;
						continue;
					}
					const recordRect = recordBtn.getBoundingClientRect();
					const recordStartX = Math.max(40, recordRect.left - 160);
					const recordStartY = recordRect.top + 120;
					await cursor.moveTo(new DOMRect(recordStartX, recordStartY, 0, 0), { duration: 0 });
					cursor.show();
					if (!stillActive()) break;
					await cursor.moveTo(recordBtn, {
						duration: 700,
						from: { x: recordStartX, y: recordStartY }
					});
					if (!stillActive()) break;
					await cursor.clickRipple();
					api.triggerArmourFlash?.();
					api.setOverlayArmourPopupRecorded?.(true);
					cursor.hide();
					// 2s dwell on the "Cost recorded: 1.23 PED" confirmation.
					// The 500ms SVG flash fades during this window so the user
					// reads the capture + recorded value as a single beat.
					if (!(await abortableWait(2000, stillActive))) break;

					// === Phase R: hide popup, reset SVG, brief gap before next iteration ===
					api.setOverlayArmourPopupVisible?.(false);
					api.resetArmourSvg?.();
					if (!(await abortableWait(700, stillActive))) break;
				}
				// Post-loop cleanup ONLY for guide-close (see overlay-spawn for the
				// race rationale). Step-transitions are handled by resetDemo.
				if (!guideState.isActive) {
					dashboardApi().setOverlayArmourPopupVisible?.(false);
					dashboardApi().resetArmourSvg?.();
					cursor.hide();
				}
			},
			resetDemo() {
				// Per-card state revert: popup + SVG. Overlay strip state is
				// intentionally NOT reverted: adjacent overlay-state cards
				// share active state, so reverting on each transition causes
				// the 80ms hide/show flicker (the previous bug). closeGuide
				// path is handled by the slot wrapper's {#if guideState.isActive}
				// gate, which unmounts on close regardless.
				dashboardApi().setOverlayArmourPopupVisible?.(false);
				dashboardApi().resetArmourSvg?.();
			}
		},
		{
			id: 'dashboard-widgets',
			// Multi-hole cutout: pierces the
			// recent-events panel AND the widgets panel as two separate
			// regions under one dim layer. Both anchors live in the natural
			// flow (gated on !(isActive && demoOverlayVisible) in +page.svelte),
			// so the play() hides the overlay strip first to un-hide them.
			anchor: () => {
				const events = document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-recent-events"]'
				);
				const widgets = document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-widgets-area"]'
				);
				if (events && widgets) {
					return [events.getBoundingClientRect(), widgets.getBoundingClientRect()];
				}
				if (widgets) return widgets;
				if (events) return events;
				return null;
			},
			placement: 'top-right',
			prose: {
				title: 'Dashboard widgets',
				body: [
					{ kind: 'p', text: 'Recent events show globals, quest completions, alerts, and more.' },
					{ kind: 'p', text: 'Switch between monitoring widgets: loot charts, composition, and quest info.' }
				]
			},
			async play({ cursor, demoApi }) {
				const stepIdx = guideState.currentStepIndex;
				const stillActive = () =>
					guideState.isActive && guideState.currentStepIndex === stepIdx;
				const api = demoApi as Partial<DashboardDemoApi>;
				// Hide the overlay strip so the recent-events + widgets sections
				// un-hide (they're gated on the negated overlay-visible flag in
				// +page.svelte). Mirrors overlay-spawn's explicit start-state
				// reset: idempotent on forward 5→6, restores correct state on
				// back-nav from a future card.
				api.setOverlayDemoVisible?.(false);
				api.setOverlayDemoTrackingStarted?.(false);
				// Reset to Loot Pulse so each guide opening / replay starts
				// the cycle from the same tab regardless of where the prior
				// session left it.
				widgetsApi().setTab?.('pulse');
				// Beat for the un-hide + tab snap to settle before the loop's
				// first tab-button query.
				if (!(await abortableWait(300, stillActive))) return;
				while (stillActive()) {
					// Dwell on Loot Pulse (initial state).
					if (!(await abortableWait(1800, stillActive))) break;

					// === Phase 1: cursor → Loot Composition tab, click, switch ===
					const lootBtn = document.querySelector<HTMLElement>(
						'[data-guide-anchor="dashboard-widgets-area"] [role="tab"][data-tab-id="loot"]'
					);
					if (lootBtn) {
						const rect = lootBtn.getBoundingClientRect();
						const startX = Math.max(40, rect.left - 200);
						const startY = rect.top + 90;
						await cursor.moveTo(new DOMRect(startX, startY, 0, 0), { duration: 0 });
						cursor.show();
						if (!stillActive()) break;
						await cursor.moveTo(lootBtn, {
							duration: 700,
							from: { x: startX, y: startY }
						});
						if (!stillActive()) break;
						await cursor.clickRipple();
						cursor.hide();
						// Guard the state mutation: clickRipple's ~450ms window
						// is otherwise an unchecked gap where step navigation
						// can leave a stale setTab to fire after the next
						// card's play() has already switched the tab.
						if (!stillActive()) break;
						widgetsApi().setTab?.('loot');
					}
					if (!(await abortableWait(1800, stillActive))) break;

					// === Phase 2: cursor → Quests tab, click, switch ===
					const questsBtn = document.querySelector<HTMLElement>(
						'[data-guide-anchor="dashboard-widgets-area"] [role="tab"][data-tab-id="quests"]'
					);
					if (questsBtn) {
						const rect = questsBtn.getBoundingClientRect();
						const startX = Math.max(40, rect.left - 200);
						const startY = rect.top + 90;
						await cursor.moveTo(new DOMRect(startX, startY, 0, 0), { duration: 0 });
						cursor.show();
						if (!stillActive()) break;
						await cursor.moveTo(questsBtn, {
							duration: 700,
							from: { x: startX, y: startY }
						});
						if (!stillActive()) break;
						await cursor.clickRipple();
						cursor.hide();
						if (!stillActive()) break;
						widgetsApi().setTab?.('quests');
					}
					if (!(await abortableWait(1800, stillActive))) break;

					// === Phase R: snap back to Loot Pulse, brief gap, loop ===
					if (!stillActive()) break;
					widgetsApi().setTab?.('pulse');
					if (!(await abortableWait(700, stillActive))) break;
				}
				// Post-loop cleanup gated on close (race-fix convention).
				if (!guideState.isActive) {
					widgetsApi().setTab?.('pulse');
					cursor.hide();
				}
			},
			// No resetDemo: adjacent card (modular-stats) explicitly switches
			// to its target tab on play() start, so reverting here would cause
			// an 80ms 'pulse' flicker during the transition. play() already
			// sets the tab to 'pulse' at start, handling back-nav from later
			// cards. Matches the 3/4/5 no-reset pattern.
		},
		{
			id: 'modular-stats',
			// Multi-hole cutout: Session stats grid + Customise tab button +
			// Customise Stats area. Three holes so the dim layer dims the
			// Loot Pulse / Loot Composition / Quests tab labels but lights
			// the selected Customise tab, connecting the tab strip visually
			// to the area below.
			anchor: () => {
				const grid = document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-stats-grid"]'
				);
				const subtab = document.querySelector<HTMLElement>(
					'[data-guide-anchor="dashboard-widgets-area"] [data-tab-id="customise"]'
				);
				const customise = document.querySelector<HTMLElement>(
					'[data-guide-anchor="customise-stats-area"]'
				);
				const rects: DOMRect[] = [];
				if (grid) rects.push(grid.getBoundingClientRect());
				if (subtab) rects.push(subtab.getBoundingClientRect());
				if (customise) rects.push(customise.getBoundingClientRect());
				if (rects.length > 0) return rects;
				return null;
			},
			placement: 'middle-right',
			// Top-align with the Recent events panel so the prose card sits in
			// the upper-right region rather than viewport-centre. The middle-right
			// placement honours placementAnchor by pinning the card's top to the
			// anchor's top (mirror of bottom-centre's anchor-aware variant).
			placementAnchor: () => document.querySelector<HTMLElement>(
				'[data-guide-anchor="dashboard-recent-events"]'
			),
			prose: {
				title: 'Modular stats',
				body: [
					{ kind: 'p', text: 'Customise what stats to show in your dashboard and overlay.' },
					{ kind: 'p', text: 'Drag and drop stats in the dashboard to reorder them. The order applies to the overlay too.' }
				]
			},
			async play({ cursor, demoApi }) {
				const stepIdx = guideState.currentStepIndex;
				const stillActive = () =>
					guideState.isActive && guideState.currentStepIndex === stepIdx;
				const api = demoApi as Partial<DashboardDemoApi>;
				// Hide overlay so the dashboard sections (stats grid + widgets)
				// render: they're gated on !(isActive && demoOverlayVisible).
				api.setOverlayDemoVisible?.(false);
				api.setOverlayDemoTrackingStarted?.(false);
				// Switch widgets to the Customise tab so the customise-area
				// anchor resolves.
				widgetsApi().setTab?.('customise');
				// Snapshot the live stat prefs and reset both stores to a
				// 3-enabled baseline (defaults minus rate) so each loop iter
				// starts from a known visual state. The snapshot persists at
				// module scope so resetDemo (called from Next/Back/Close/Replay)
				// can restore it.
				statsSnapshot = api.snapshotStats?.() ?? null;
				api.setDemoStatsBaseline?.({ rate: false });
				if (!(await abortableWait(400, stillActive))) {
					if (statsSnapshot) api.restoreStats?.(statsSnapshot);
					return;
				}

				// Three pills to demonstrate (dashboard surface): add pes + multi_max
				// (3 → 5 enabled), then remove loot_tt (5 → 4). Removing an
				// originally-enabled pill shows the bidirectional toggle clearly.
				// After Phase 1 the stats grid shows 4 cells: cycled, net, pes, multi_max.
				const pillsToToggle: Array<{ id: string; label: string }> = [
					{ id: 'pes', label: 'PES' },
					{ id: 'multiplier_max', label: 'Max Multi' },
					{ id: 'loot_tt', label: 'Loot TT' }
				];

				while (stillActive()) {
					// === Phase 1: 3 pill clicks ===
					for (const pill of pillsToToggle) {
						const pillEl = document.querySelector<HTMLElement>(
							`[data-customise-surface="dashboard"] [data-pill-id="${pill.id}"]`
						);
						if (!pillEl) continue;
						const rect = pillEl.getBoundingClientRect();
						const startX = Math.max(40, rect.left - 180);
						const startY = rect.top + 90;
						await cursor.moveTo(new DOMRect(startX, startY, 0, 0), { duration: 0 });
						cursor.show();
						if (!stillActive()) break;
						await cursor.moveTo(pillEl, {
							duration: 600,
							from: { x: startX, y: startY }
						});
						if (!stillActive()) break;
						await cursor.clickRipple();
						// Guard the state mutation: clickRipple's ~450ms window
						// can otherwise leave a stale toggle to fire after the
						// previous step's resetDemo has already restored the
						// snapshot, corrupting subsequent restores.
						if (!stillActive()) break;
						api.toggleDemoStatPill?.('dashboard', pill.id);
						cursor.hide();
						if (!(await abortableWait(450, stillActive))) break;
					}
					if (!stillActive()) break;
					if (!(await abortableWait(800, stillActive))) break;

					// === Phase 2: drag-reorder ===
					// After Phase 1, filtered enabled order is:
					// cycled(0), net(1), pes(2), multi_max(3): 4 cells.
					// Drag multi_max (idx 3) to position 0 (front of grid).
					const FROM_IDX = 3;
					const TO_IDX = 0;
					const sourceCell = document.querySelector<HTMLElement>(
						`[data-stat-cell="${FROM_IDX}"]`
					);
					const targetCell = document.querySelector<HTMLElement>(
						`[data-stat-cell="${TO_IDX}"]`
					);
					if (sourceCell && targetCell) {
						const srcRect = sourceCell.getBoundingClientRect();
						const tgtRect = targetCell.getBoundingClientRect();
						const srcCx = srcRect.left + srcRect.width / 2;
						const srcCy = srcRect.top + srcRect.height / 2;
						const startX = Math.max(40, srcRect.left - 200);
						const startY = srcRect.top + 150;
						await cursor.moveTo(new DOMRect(startX, startY, 0, 0), { duration: 0 });
						cursor.show();
						if (!stillActive()) break;
						// Slide up to the source cell
						await cursor.moveTo(sourceCell, {
							duration: 800,
							from: { x: startX, y: startY }
						});
						if (!stillActive()) break;
						// Pickup: cursor grab + cell drag visual
						cursor.setState('grab');
						api.setDragVisualIndex?.(FROM_IDX);
						if (!(await abortableWait(400, stillActive))) {
							cursor.setState('idle');
							api.setDragVisualIndex?.(null);
							break;
						}
						// Slide to target cell with grab still held
						await cursor.moveTo(targetCell, {
							duration: 900,
							from: { x: srcCx, y: srcCy }
						});
						if (!stillActive()) {
							cursor.setState('idle');
							api.setDragVisualIndex?.(null);
							break;
						}
						// Drop: reorder + clear drag visual + cursor idle
						api.reorderDemoStat?.(FROM_IDX, TO_IDX);
						api.setDragVisualIndex?.(null);
						cursor.setState('idle');
						cursor.hide();
					}
					if (!(await abortableWait(1500, stillActive))) break;

					// === Phase R: reset to baseline (3 enabled) + gap ===
					api.setDemoStatsBaseline?.({ rate: false });
					if (!(await abortableWait(700, stillActive))) break;
				}
				// Post-loop cleanup gated on close (race-fix convention).
				if (!guideState.isActive) {
					if (statsSnapshot) {
						dashboardApi().restoreStats?.(statsSnapshot);
						statsSnapshot = null;
					}
					dashboardApi().setDragVisualIndex?.(null);
					cursor.setState('idle');
					cursor.hide();
				}
			},
			resetDemo() {
				// Restore the snapshotted stat prefs on Next / Back / Close /
				// Replay. The snapshot was captured at play start; module-scope
				// storage lets resetDemo reach it without play()'s closure.
				if (statsSnapshot) {
					dashboardApi().restoreStats?.(statsSnapshot);
					statsSnapshot = null;
				}
				dashboardApi().setDragVisualIndex?.(null);
			}
		}
	]
};
