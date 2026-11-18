/**
 * The route-HUD controller: the navigation run's transient state (route setup
 * fields, position/visit feedback, the transient in-strip badges) and the
 * command handlers over the navigation IPC surface. Lifecycle wiring (the
 * event listeners, the position-poll effect, window drag and sizing) stays
 * in the route shell; this keeps that route a thin shell over the feature
 * module, the same split `mapsController` uses.
 */

import {
	beginNavigationAreaSelection,
	type CoordRefusal,
	endNavigation,
	getInputStatus,
	getNavigationSnapshot,
	getRadarGuidance,
	hideNavigationOverlays,
	markNavigationVisited,
	type NavigationPositionResult,
	type NavigationRun,
	type PositionPollStop,
	type RadarGuidance,
	resolveNavigationHarvest,
	resumeNavigationPosition,
	scanMapCoordinates,
	showNavigationOverlays,
	skipNavigationStop,
	startNavigation,
	undoNavigationStop,
	updateNavigationPosition,
} from '$lib/api';
import { getPreference, setPreference } from '$lib/preferences';
import { describeError } from '$lib/view/errorState';
import { acceptCartographyContextBroadcast } from './cartographyOverlay.svelte';
import { coordRefusalMessage, scannedPoint } from './coordinateRead';
import type { GamePoint } from './coords';
import { positionIsCurrent } from './radarGuidance';
import {
	acceptRouteAreaSelectionCancellation,
	acceptRouteAreaSelectionResult,
	resetRouteAreaSelection,
} from './routeAreaSelection';

function statusFeedback(result: NavigationPositionResult, planet: string): string {
	switch (result.status) {
		case 'updated':
			return 'Position updated.';
		case 'noActiveRun':
			return 'No active route.';
		case 'notRead':
			return result.refusal
				? coordRefusalMessage(result.refusal, planet)
				: 'The position could not be read.';
		case 'superseded':
			return 'The route changed while your position was read.';
		case 'jumpRefused':
			return 'Your position did not confirm; try again in a moment.';
		case 'ambiguous':
			return 'Several route points are within range.';
		default:
			return 'The position could not be read.';
	}
}

/**
 * The one line the HUD says about radar guidance, or null when there is
 * nothing to say: the radar is placed and its distances are read.
 */
export function guidanceNote(guidance: RadarGuidance | null): string | null {
	if (!guidance) return null;
	switch (guidance.kind) {
		case 'placed':
			switch (guidance.scaleMissing) {
				case 'labelsUnread':
					return "Show ring distances in the game's interface options to see distances on the radar.";
				case 'noRings':
					return "The radar's rings could not be seen, so distances are not shown on it.";
				case 'noReader':
					return 'Distances on the radar need the text recogniser, which is not available.';
				default:
					return null;
			}
		case 'lost':
			switch (guidance.reason) {
				case 'radarNotFound':
					return 'The radar is not on the screen.';
				case 'gameWindowNotFound':
					return 'The game window was not found.';
				case 'gameWindowOffScreen':
					return 'The game window is off the screen that can be captured.';
				case 'captureFailed':
					return 'The screen could not be captured.';
			}
			return null;
		case 'off':
			switch (guidance.stopped) {
				case 'captureNotAllowed':
					return 'Screen capture was not allowed, so radar guidance stopped.';
				case 'radarNotSeen':
					return 'The radar was off the screen for a while, so radar guidance stopped.';
				case 'readerUnavailable':
					return 'Radar guidance is unavailable on this installation.';
				case 'failed':
					return 'Radar guidance stopped unexpectedly.';
				default:
					return null;
			}
		default:
			return null;
	}
}

// The poll says why it could not read only once the same failure has held
// for two polls in a row, so a passing window does not flicker a message.
const POLL_REFUSALS_BEFORE_NOTE = 2;

/** Why the poll stopped, in the player's terms. */
function pollStopNote(stop: PositionPollStop): string | null {
	switch (stop) {
		case 'captureNotAllowed':
			return 'Screen capture was not allowed, so position updates stopped.';
		case 'notReading':
			return 'Your position has not read for a while, so position updates stopped.';
		case 'overlaysHidden':
			return null;
	}
}

/** One refusal, for telling whether two polls failed the same way. */
function refusalKey(refusal: CoordRefusal): string {
	return `${refusal.reason}:${refusal.lon ?? ''}:${refusal.lat ?? ''}`;
}

export function createNavigationHudController() {
	let run = $state<NavigationRun | null>(null);
	let busy = $state(false);
	let feedback = $state<string | null>(null);
	// Route setup lives here (not on the Maps page) so a single-monitor player
	// can plan while the game is fullscreen. Planet/map context arrives from the
	// main surface over the shared cartography-context broadcast.
	let planet = $state<string | null>(null);
	let mapViewId = $state<number | null>(null);
	let start = $state<GamePoint | null>(null);
	// Null selects every eligible tree; a confirmed map-area selection carries
	// the exact pin allow-list previewed on the main map.
	let selectedPinIds = $state<number[] | null>(null);
	let selectionRequestId = $state<number | null>(null);
	let nextSelectionRequestId = 0;
	// The position is polled on the observe path every interval while a route
	// is live. Visiting on F records each tree the moment the game's interact
	// key is pressed instead of detecting the cut from harvests. Both are
	// remembered across routes.
	let updateIntervalSec = $state(1);
	let visitOnKey = $state(false);
	// A machine that cannot read keys never sees F, so the mode is withheld
	// there and cuts are detected from harvests alone.
	let keyless = $state(false);
	// Set when a manual Visited lands outside the arrival tolerance: the visit
	// is held until the user confirms a forced record.
	let pendingVisit = $state<{ name: string; distance: number } | null>(null);
	// Radar guidance as the backend last reported it.
	let guidance = $state<RadarGuidance | null>(null);
	// The position poll: one read at a time. The backend decides whether it
	// may read; when it has stopped (every read refused for a while, or
	// screen capture not allowed) the HUD says why and offers to resume.
	let polling = false;
	let pollStop = $state<PositionPollStop | null>(null);
	let lastRefusal: string | null = null;
	let pollRefusals = 0;
	let pollNote = $state<string | null>(null);
	// When the run last arrived, and the clock the position's age is
	// measured against: a position too old is not steered from.
	let runReceivedAt = $state(0);
	let now = $state(Date.now());
	// Each start-position capture's number: a read that lands after another
	// was asked for, or after the planet or map changed, is dropped.
	let captureRequest = 0;

	const active = $derived(run?.stops.find((stop) => stop.status === 'active') ?? null);
	// A harvest was detected beyond the arrival radius; the overlay asks whether
	// it was the intended tree rather than dropping it.
	const pendingHarvest = $derived(run?.pendingHarvest ?? null);
	const cutCount = $derived(run?.stops.filter((stop) => stop.status === 'visited').length ?? 0);
	const totalStops = $derived(run?.stops.length ?? 0);
	const canStart = $derived(!busy && start != null && planet != null);

	// Transient in-strip acknowledgements (replacing the old bottom text): a
	// badge in the distance slot for two seconds when a tree is resolved, and a
	// full-strip out-of-order notice for three seconds when a harvest matched a
	// later tree and the path was recomputed. The badge distinguishes an actual
	// harvest (green "Tree Cut") from merely marking a tree visited without
	// cutting it (yellow "Visited"), so the two outcomes read differently.
	let badge = $state<'cut' | 'visited' | null>(null);
	let badgeTimer: ReturnType<typeof setTimeout> | null = null;
	function signalBadge(kind: 'cut' | 'visited') {
		badge = kind;
		if (badgeTimer) clearTimeout(badgeTimer);
		badgeTimer = setTimeout(() => (badge = null), 2000);
	}
	let outOfOrder = $state(false);
	let outOfOrderTimer: ReturnType<typeof setTimeout> | null = null;
	function signalOutOfOrder() {
		outOfOrder = true;
		if (outOfOrderTimer) clearTimeout(outOfOrderTimer);
		outOfOrderTimer = setTimeout(() => (outOfOrder = false), 3000);
	}

	// A harvest or an F press advances the route from the backend. Diffing the
	// previous snapshot against the next surfaces which tree was recorded, and
	// whether it was reached out of order (so the remaining path was recomputed).
	function applyCutFeedback(prev: NavigationRun, next: NavigationRun) {
		const before = new Map(prev.stops.map((stop) => [stop.id, stop.status]));
		const prevActiveId = prev.stops.find((stop) => stop.status === 'active')?.id;
		for (const stop of next.stops) {
			const priorStatus = before.get(stop.id);
			if (
				stop.status === 'visited' &&
				priorStatus != null &&
				priorStatus !== 'visited' &&
				(stop.completionSource === 'harvest' || stop.completionSource === 'key')
			) {
				if (priorStatus === 'active' || stop.id === prevActiveId) {
					signalBadge('cut');
				} else {
					signalOutOfOrder();
				}
				pendingVisit = null;
			}
		}
	}

	function adoptRun(next: NavigationRun | null) {
		run = next;
		runReceivedAt = Date.now();
	}

	/** The position's age moves on while no new one arrives. */
	function tick() {
		now = Date.now();
	}

	async function refreshGuidance() {
		try {
			guidance = await getRadarGuidance();
		} catch {
			// The next guidance change re-reads it.
		}
	}

	// Guidance stopped (the radar off the screen, capture not allowed, or a
	// failure): the player asks it to look again, which may ask the
	// desktop's consent to capture once more.
	async function lookForRadarAgain() {
		try {
			await resumeNavigationPosition();
			forgetPollState();
			await showNavigationOverlays();
		} finally {
			await refreshGuidance();
		}
	}

	async function hydrate() {
		try {
			const next = await getNavigationSnapshot();
			if (next && run) applyCutFeedback(run, next);
			adoptRun(next);
			// No run means the setup panel is shown; the overlay only hides on an
			// explicit close, not whenever a route ends.
		} catch {
			feedback = 'Navigation is unavailable.';
		}
	}

	async function loadPrefs() {
		updateIntervalSec = await getPreference('navUpdateIntervalSec', 1);
		visitOnKey = await getPreference('navVisitOnKey', false);
		try {
			keyless = (await getInputStatus()).keyless;
		} catch {
			keyless = false;
		}
	}

	function applyContext(payload: unknown) {
		const context = acceptCartographyContextBroadcast(payload);
		// A start captured on another planet is not a start on this one.
		if (context.planet !== planet) start = null;
		planet = context.planet;
		mapViewId = context.mapViewId;
	}

	function setVisitOnKey(value: boolean) {
		visitOnKey = value;
		void setPreference('navVisitOnKey', value);
	}

	function persistInterval() {
		updateIntervalSec = Math.min(60, Math.max(1, Math.round(updateIntervalSec || 1)));
		void setPreference('navUpdateIntervalSec', updateIntervalSec);
	}

	// The position poll runs the observe-only path on a fixed interval while a
	// route is live, so the radar dot and bearing track the player. It records
	// no visit, takes one read at a time, and says why it cannot read only once
	// the reason has held for two polls. When screen capture is not allowed it
	// stops, rather than asking the player for consent on every tick.
	async function pollPosition() {
		if (busy || polling || pollStop) return;
		polling = true;
		try {
			const result = await updateNavigationPosition();
			if (result.run) adoptRun(result.run);
			if (result.status === 'updated') {
				pollRefusals = 0;
				lastRefusal = null;
				pollNote = null;
			} else if (result.status === 'pollStopped' && result.stopped) {
				pollStop = result.stopped;
				pollNote = pollStopNote(result.stopped);
			} else if (result.status === 'notRead' && result.refusal) {
				const key = refusalKey(result.refusal);
				pollRefusals = key === lastRefusal ? pollRefusals + 1 : 1;
				lastRefusal = key;
				if (pollRefusals >= POLL_REFUSALS_BEFORE_NOTE) {
					pollNote = coordRefusalMessage(result.refusal, run?.planet ?? planet ?? 'this planet');
				}
			}
		} catch {
			// A transient failure is silent here; the next tick retries.
		} finally {
			polling = false;
		}
	}

	function forgetPollState() {
		pollStop = null;
		pollRefusals = 0;
		lastRefusal = null;
		pollNote = null;
	}

	// The player asks the poll to read again: the backend lets it, and a
	// capture may ask the desktop's consent once more.
	async function resumePolling() {
		try {
			await resumeNavigationPosition();
			forgetPollState();
		} catch {
			pollNote = 'Position updates could not be resumed.';
		}
	}

	// Capture the current in-game coordinates as the route start. Only a read
	// position seeds the route; a refusal says why and carries no number.
	async function captureStart() {
		if (busy || !planet) return;
		busy = true;
		feedback = null;
		const request = ++captureRequest;
		const forPlanet = planet;
		const forMap = mapViewId;
		try {
			const result = await scanMapCoordinates(forPlanet);
			if (request !== captureRequest || planet !== forPlanet || mapViewId !== forMap) return;
			const point = scannedPoint(result);
			if (point) {
				start = point;
			} else if (result.kind === 'refused') {
				feedback = coordRefusalMessage(result.refusal, forPlanet);
			}
		} catch (cause) {
			feedback = describeError(cause, 'The current position could not be captured');
		} finally {
			busy = false;
		}
	}

	async function chooseRouteArea() {
		if (busy || !planet) return;
		busy = true;
		feedback = null;
		const requestId = ++nextSelectionRequestId;
		selectionRequestId = requestId;
		try {
			await beginNavigationAreaSelection(requestId, planet, mapViewId);
			feedback = 'Open Maps to choose a route area.';
		} catch (cause) {
			selectionRequestId = null;
			feedback = describeError(cause, 'The map selection could not be opened');
		} finally {
			busy = false;
		}
	}

	function applyRouteAreaSelection(payload: unknown) {
		const result = acceptRouteAreaSelectionResult(payload);
		if (
			!result ||
			result.requestId !== selectionRequestId ||
			result.planet !== planet ||
			result.mapViewId !== mapViewId
		) {
			return;
		}
		selectedPinIds = result.pinIds;
		selectionRequestId = null;
		feedback = null;
	}

	function applyRouteAreaSelectionCancelled(payload: unknown) {
		if (acceptRouteAreaSelectionCancellation(payload) === selectionRequestId) {
			selectionRequestId = null;
			feedback = null;
		}
	}

	function useAllTrees() {
		selectedPinIds = null;
		resetRouteAreaSelection();
	}

	async function beginRoute() {
		if (!canStart || !start || !planet) return;
		busy = true;
		feedback = null;
		try {
			const started = await startNavigation(
				planet,
				mapViewId,
				start.lon,
				start.lat,
				selectedPinIds,
				visitOnKey && !keyless,
			);
			adoptRun(started);
			start = null;
			selectedPinIds = null;
			resetRouteAreaSelection();
			forgetPollState();
			// The main surface repositions the HUD and radar around the live route.
		} catch (cause) {
			feedback = describeError(cause, 'The route could not be created');
		} finally {
			busy = false;
		}
	}

	async function closeOverlay() {
		if (busy) return;
		busy = true;
		try {
			await hideNavigationOverlays();
		} finally {
			busy = false;
		}
	}

	async function act(action: () => Promise<NavigationRun>) {
		if (busy) return;
		busy = true;
		feedback = null;
		pendingVisit = null;
		try {
			adoptRun(await action());
		} catch {
			feedback = 'The route could not be updated.';
		} finally {
			busy = false;
		}
	}

	// Visited records the active tree. Outside the arrival tolerance the visit
	// is held for an explicit confirmation (force).
	async function markVisited(force: boolean) {
		if (busy) return;
		busy = true;
		feedback = null;
		const target = active?.name ?? 'this tree';
		try {
			const result = await markNavigationVisited(force);
			if (result.run) adoptRun(result.run);
			if (result.status === 'updated') {
				pendingVisit = null;
				signalBadge('visited');
				// The player read the screen themselves: the poll may read again.
				forgetPollState();
			} else if (result.status === 'outOfTolerance') {
				pendingVisit = { name: target, distance: result.run?.distanceToActive ?? 0 };
				forgetPollState();
			} else {
				pendingVisit = null;
				feedback = statusFeedback(result, run?.planet ?? planet ?? 'this planet');
			}
		} catch {
			feedback = 'The visit could not be recorded.';
		} finally {
			busy = false;
		}
	}

	// A harvest landed beyond the arrival radius. EU trees cut from far away, so
	// the overlay asks whether this was the intended tree; confirm records it,
	// dismiss leaves the route untouched.
	async function resolveHarvest(confirm: boolean) {
		if (busy) return;
		busy = true;
		feedback = null;
		pendingVisit = null;
		try {
			adoptRun(await resolveNavigationHarvest(confirm));
			feedback = null;
			if (confirm) signalBadge('cut');
		} catch {
			feedback = 'The harvest could not be updated.';
		} finally {
			busy = false;
		}
	}

	// Closing the HUD ends the visible navigation interaction; starting a new
	// route is the way to replan.
	async function endRoute() {
		if (busy) return;
		busy = true;
		try {
			await endNavigation();
			await hideNavigationOverlays();
		} finally {
			busy = false;
		}
	}

	function dispose() {
		if (badgeTimer) clearTimeout(badgeTimer);
		if (outOfOrderTimer) clearTimeout(outOfOrderTimer);
	}

	return {
		get run() {
			return run;
		},
		get busy() {
			return busy;
		},
		get feedback() {
			return feedback;
		},
		get planet() {
			return planet;
		},
		get start() {
			return start;
		},
		get active() {
			return active;
		},
		get pendingHarvest() {
			return pendingHarvest;
		},
		get pendingVisit() {
			return pendingVisit;
		},
		clearPendingVisit() {
			pendingVisit = null;
		},
		get cutCount() {
			return cutCount;
		},
		get totalStops() {
			return totalStops;
		},
		get canStart() {
			return canStart;
		},
		get badge() {
			return badge;
		},
		get outOfOrder() {
			return outOfOrder;
		},
		get visitOnKey() {
			return visitOnKey && !keyless;
		},
		get keyless() {
			return keyless;
		},
		get updateIntervalSec() {
			return updateIntervalSec;
		},
		set updateIntervalSec(value: number) {
			updateIntervalSec = value;
		},
		get selectedTreeCount() {
			return selectedPinIds?.length ?? null;
		},
		get guidance() {
			return guidance;
		},
		/** Whether the route's position is recent enough to steer from. */
		get positionCurrent() {
			return (
				run != null && positionIsCurrent(run.positionAgeS, runReceivedAt, now, updateIntervalSec)
			);
		},
		/** The poll stopped and can be resumed by the player. */
		get pollStopped() {
			return pollStop === 'notReading' || pollStop === 'captureNotAllowed';
		},
		/** The quiet status line: feedback first, then the poll, then guidance. */
		get note() {
			return (
				feedback ??
				pollNote ??
				(run?.status === 'active' &&
				!positionIsCurrent(run.positionAgeS, runReceivedAt, now, updateIntervalSec)
					? 'Your position is not current.'
					: null) ??
				guidanceNote(guidance)
			);
		},
		/** Guidance stopped by itself, so the player may ask it to look again. */
		get guidanceStopped() {
			return (
				guidance?.kind === 'off' &&
				guidance.stopped != null &&
				guidance.stopped !== 'readerUnavailable'
			);
		},
		hydrate,
		tick,
		resumePolling,
		refreshGuidance,
		lookForRadarAgain,
		loadPrefs,
		applyContext,
		setVisitOnKey,
		persistInterval,
		pollPosition,
		captureStart,
		chooseRouteArea,
		applyRouteAreaSelection,
		applyRouteAreaSelectionCancelled,
		useAllTrees,
		beginRoute,
		closeOverlay,
		markVisited,
		resolveHarvest,
		endRoute,
		skip: () => act(skipNavigationStop),
		undo: () => act(undoNavigationStop),
		dispose,
	};
}

export type NavigationHudController = ReturnType<typeof createNavigationHudController>;
