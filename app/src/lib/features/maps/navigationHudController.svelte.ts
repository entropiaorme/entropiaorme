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
	endNavigation,
	getNavigationSnapshot,
	hideNavigationOverlays,
	markNavigationVisited,
	type NavigationPositionStatus,
	type NavigationRun,
	resolveNavigationHarvest,
	scanMapCoordinates,
	skipNavigationStop,
	startNavigation,
	undoNavigationStop,
	updateNavigationPosition,
} from '$lib/api';
import { getPreference, setPreference } from '$lib/preferences';
import { describeError } from '$lib/view/errorState';
import {
	acceptCartographyContextBroadcast,
	cartographyScanFailureMessage,
} from './cartographyOverlay.svelte';
import { formatGamePoint, type GamePoint } from './coords';
import {
	acceptRouteAreaSelectionCancellation,
	acceptRouteAreaSelectionResult,
	resetRouteAreaSelection,
} from './routeAreaSelection';

function statusFeedback(status: NavigationPositionStatus): string {
	switch (status) {
		case 'updated':
			return 'Position updated.';
		case 'noActiveRun':
			return 'No active route.';
		case 'noRegion':
			return 'Calibrate coordinate capture first.';
		case 'ambiguous':
			return 'Several route points are within range.';
		case 'unreadable':
			return 'The coordinates could not be read.';
		case 'implausible':
			return 'That reading looked implausible.';
		default:
			return 'The position could not be read.';
	}
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
	// Set when a manual Visited lands outside the arrival tolerance: the visit
	// is held until the user confirms a forced record.
	let pendingVisit = $state<{ name: string; distance: number } | null>(null);

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

	async function hydrate() {
		try {
			const next = await getNavigationSnapshot();
			if (next && run) applyCutFeedback(run, next);
			run = next;
			// No run means the setup panel is shown; the overlay only hides on an
			// explicit close, not whenever a route ends.
		} catch {
			feedback = 'Navigation is unavailable.';
		}
	}

	async function loadPrefs() {
		updateIntervalSec = await getPreference('navUpdateIntervalSec', 1);
		visitOnKey = await getPreference('navVisitOnKey', false);
	}

	function applyContext(payload: unknown) {
		const context = acceptCartographyContextBroadcast(payload);
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
	// no visit and stays quiet on a transient read failure.
	async function pollPosition() {
		if (busy) return;
		try {
			const result = await updateNavigationPosition();
			if (result.run) run = result.run;
		} catch {
			// A transient scan failure is silent here; the next tick retries.
		}
	}

	// Capture the current in-game coordinates as the route start. A reading that
	// OCRs cleanly but falls outside the planet's map bounds ('implausible') still
	// carries usable numbers, so it seeds the route with a note rather than being
	// discarded as a failure.
	async function captureStart() {
		if (busy || !planet) return;
		busy = true;
		feedback = null;
		try {
			const result = await scanMapCoordinates(planet);
			if (
				(result.status === 'read' || result.status === 'implausible') &&
				result.lon != null &&
				result.lat != null
			) {
				start = { lon: result.lon, lat: result.lat };
				feedback =
					result.status === 'implausible'
						? `Captured ${formatGamePoint(start)} (reads outside ${planet}).`
						: null;
			} else {
				feedback = cartographyScanFailureMessage(result.status, planet);
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
			run = await startNavigation(
				planet,
				mapViewId,
				start.lon,
				start.lat,
				selectedPinIds,
				visitOnKey,
			);
			start = null;
			selectedPinIds = null;
			resetRouteAreaSelection();
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
			run = await action();
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
			if (result.run) run = result.run;
			if (result.status === 'updated') {
				pendingVisit = null;
				signalBadge('visited');
			} else if (result.status === 'outOfTolerance') {
				pendingVisit = { name: target, distance: result.run?.distanceToActive ?? 0 };
			} else {
				pendingVisit = null;
				feedback = statusFeedback(result.status);
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
			run = await resolveNavigationHarvest(confirm);
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
			return visitOnKey;
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
		hydrate,
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
