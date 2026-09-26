/**
 * Dashboard stats-grid view model: the enabled-stat projection, the
 * pointer-driven drag-reorder.
 * The only consumer of the stat-customisation state on the dashboard
 * surface; presentation composes over this state.
 */

import {
	dashboardStats,
	isOwnSelection,
	type StatPref,
	scopedStats,
	setDashboardStats,
} from '$lib/statsCustomisation.svelte';
import { type StatsScope, statsScope } from '$lib/statsScope.svelte';

const REORDER_COOLDOWN_MS = 100;
const DRAG_THRESHOLD_PX = 4;

/**
 * Map a position in the DRAWN list back to its slot in the full stored
 * list, by identity rather than by counting enabled entries: under the
 * lifetime scope the drawn list is a non-contiguous subset of the
 * stored one, so counting would land on the wrong stat.
 */
function fullIndexOfVisible(prefs: StatPref[], visible: StatPref[], filteredIndex: number): number {
	const id = visible[filteredIndex]?.id;
	return id === undefined ? -1 : prefs.findIndex((pref) => pref.id === id);
}

/**
 * @param lifetimeAvailable Whether the frame carries a session family to
 * read lifetime figures from. A closure so the model tracks the live
 * value; the default keeps the instance behaviour for callers with no
 * family of their own.
 */
export function createStatsGridModel(lifetimeAvailable: () => boolean = () => false) {
	// The one place the scope is resolved, so the grid and the surfaces
	// around it cannot disagree about which figures are being drawn. The
	// lifetime scope needs a family to read: without one the surface
	// falls back to the instance rather than drawing an empty flip.
	function scope(): StatsScope {
		return statsScope.current === 'lifetime' && lifetimeAvailable() ? 'lifetime' : 'instance';
	}

	// Stats grid drag-reorder via pointer events (not HTML5 drag: the latter
	// cedes cursor control to the OS, so we can't keep the grabbing hand stable
	// through the gesture). dragFilteredIndex tracks the dragged cell's position
	// within the drawn list; the underlying full store list is mutated via
	// fullIndexOfVisible() so undrawn stats stay in their slots.
	let dragFilteredIndex = $state<number | null>(null);
	let dragMoved = $state(false);
	let dragStartX = 0;
	let dragStartY = 0;
	// Cooldown after each reorder so cursor jitter at a cell boundary doesn't
	// ping-pong the layout while the flip animation is still settling.
	let lastReorderAt = 0;

	/** The drawn list for the current scope: the grid's render list. */
	function visible(): StatPref[] {
		return scopedStats(dashboardStats.current, scope());
	}

	/** Reordering persists a global order, so it is offered only while
	 * the drawn set is the user's own selection: dragging the headline
	 * fallback would save an order over stats they never picked. */
	function reorderable(): boolean {
		return isOwnSelection(dashboardStats.current, scope());
	}

	function handlePointerDown(e: PointerEvent, filteredIndex: number) {
		if (e.button !== 0) return;
		if (!reorderable()) return;
		const target = e.currentTarget as HTMLElement;
		target.setPointerCapture(e.pointerId);
		dragFilteredIndex = filteredIndex;
		dragStartX = e.clientX;
		dragStartY = e.clientY;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.add('stat-drag-active');
	}

	function handlePointerMove(e: PointerEvent) {
		if (dragFilteredIndex === null) return;
		// Threshold-gate: don't reorder for sub-pixel jitter on a click.
		if (!dragMoved) {
			const dx = e.clientX - dragStartX;
			const dy = e.clientY - dragStartY;
			if (dx * dx + dy * dy < DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX) return;
			dragMoved = true;
		}
		const now = performance.now();
		if (now - lastReorderAt < REORDER_COOLDOWN_MS) return;
		// Hit-test by walking cells' bounding rects directly. elementFromPoint
		// would return the captured (dragged) element because of pointer capture.
		const cells = document.querySelectorAll<HTMLElement>('[data-stat-cell]');
		let targetFilteredIndex = -1;
		for (const cell of cells) {
			const rect = cell.getBoundingClientRect();
			if (
				e.clientX >= rect.left &&
				e.clientX <= rect.right &&
				e.clientY >= rect.top &&
				e.clientY <= rect.bottom
			) {
				const idx = Number(cell.dataset.statCell);
				if (!Number.isNaN(idx)) targetFilteredIndex = idx;
				break;
			}
		}
		if (targetFilteredIndex < 0 || targetFilteredIndex === dragFilteredIndex) return;
		const full = dashboardStats.current;
		const drawn = visible();
		const sourceFull = fullIndexOfVisible(full, drawn, dragFilteredIndex);
		const targetFull = fullIndexOfVisible(full, drawn, targetFilteredIndex);
		if (sourceFull < 0 || targetFull < 0) return;
		const next = [...full];
		const [moved] = next.splice(sourceFull, 1);
		next.splice(targetFull, 0, moved);
		dashboardStats.current = next;
		dragFilteredIndex = targetFilteredIndex;
		lastReorderAt = now;
	}

	function handlePointerUp(e: PointerEvent) {
		if (dragFilteredIndex === null) return;
		const target = e.currentTarget as HTMLElement;
		if (target?.hasPointerCapture?.(e.pointerId)) {
			target.releasePointerCapture(e.pointerId);
		}
		if (dragMoved) void setDashboardStats(dashboardStats.current);
		dragFilteredIndex = null;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.remove('stat-drag-active');
	}

	function handlePointerCancel() {
		if (dragFilteredIndex === null) return;
		if (dragMoved) void setDashboardStats(dashboardStats.current);
		dragFilteredIndex = null;
		dragMoved = false;
		lastReorderAt = 0;
		document.body.classList.remove('stat-drag-active');
	}

	return {
		/**
		 * The grid's render list, in stored order: the enabled prefs
		 * under the instance scope, narrowed to the lifetime-capable
		 * ones (or the headline fallback) under the lifetime scope.
		 */
		get enabledStats() {
			return visible();
		},
		/** The resolved scope the grid is drawing in; the surfaces around
		 * the grid read this rather than resolving it again. */
		get scope() {
			return scope();
		},
		/** Whether the drawn set may be drag-reordered. */
		get reorderable() {
			return reorderable();
		},
		get dragFilteredIndex() {
			return dragFilteredIndex;
		},

		handlePointerDown,
		handlePointerMove,
		handlePointerUp,
		handlePointerCancel,
	};
}

export type StatsGridModel = ReturnType<typeof createStatsGridModel>;
