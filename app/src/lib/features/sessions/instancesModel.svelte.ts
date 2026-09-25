/**
 * The recorded-instances view model: the session list load, row
 * expand/collapse with its detail fetch, deletion, re-filing, and the
 * client-side pager. Presentation lives in the review surface; it
 * composes over this state.
 *
 * Optionally scoped to one definition, which is how the review surface
 * reads it: the scope narrows the server's count as well as its rows, so
 * the pager reports that definition's own bounds. Unscoped, this is the
 * whole recorded history.
 *
 * Paging is two-layered by design (the ledger tab's shape): the server
 * side stays keyset (an opaque cursor grows the loaded window on demand
 * as the pager steps past it), while the client-side pager over the
 * loaded window is the shared table model; the server's count gives the
 * pager its true bounds.
 *
 * Armour cost is recorded when the player repairs, possibly sessions after
 * the play it covers, so a row whose session still awaits a recording is
 * marked: its net is not final. A recording or undo from the overlay
 * re-reads the loaded rows and their marks in place.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
	deleteSession,
	getSessionDetail,
	getTrackingSessions,
	getUnrecordedArmourSessions,
	HEALING_TOPIC,
	PROTECTION_TOPIC,
	reassignSession,
} from '$lib/api';
import type { SessionDetail, TrackingSession } from '$lib/types/tracking';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';

export const PAGE_SIZE = 10;

/** The most rows one in-place refresh re-reads (the backend's page cap). */
const REFRESH_LIMIT = 200;

export interface InstancesModelOptions {
	/** The definition whose instances to read; null (or omitted) reads
	 * the whole history. Read at fetch time, so a caller can switch the
	 * definition under review and reload. */
	definitionId?: () => string | null;
}

export function createInstancesModel(options: InstancesModelOptions = {}) {
	const scope = () => options.definitionId?.() ?? undefined;
	let sessions = $state<TrackingSession[]>([]);
	// The whole-table session count from the server, so the pager reports
	// true bounds rather than the loaded window's size.
	let total = $state(0);
	let loading = $state(true);
	let error = $state<string | null>(null);
	// Keyset pagination: the cursor for the next server page (null once
	// every session is loaded), and whether a "load more" fetch is in flight.
	let nextCursor = $state<string | null>(null);
	let loadingMore = $state(false);
	let expandedSessionId = $state<string | null>(null);
	let expandedDetail = $state<SessionDetail | null>(null);
	let loadingDetail = $state(false);
	let confirmDeleteId = $state<string | null>(null);
	let deleting = $state(false);
	// The in-flight guard for a re-file write. The chooser's own open
	// state belongs to the menu that renders it.
	let reassigning = $state(false);
	// Loaded sessions whose armour cost no recording covers yet.
	let armourPending = $state<ReadonlySet<string>>(new Set());

	// Pure pager over the loaded window: no search, category, or sort, so
	// the paged rows keep the backend's ordering unchanged.
	const table = createTableModel<TrackingSession>({
		rows: () => sessions,
		pageSize: PAGE_SIZE,
	});

	/** Re-read which loaded sessions still await an armour recording. A
	 * failed read keeps the previous marks rather than clearing them. */
	async function refreshArmourMarks(): Promise<void> {
		const ids = sessions.map((session) => session.id);
		try {
			armourPending = new Set(ids.length === 0 ? [] : await getUnrecordedArmourSessions(ids));
		} catch {
			// The next protection write re-reads.
		}
	}

	/** After an armour recording or a healing correction: re-read the loaded
	 * rows' figures in place, keeping the page and any open row, then their
	 * marks. */
	async function refreshCosts(): Promise<void> {
		if (loading || sessions.length === 0) return;
		try {
			const limit = Math.min(sessions.length, REFRESH_LIMIT);
			const page = await getTrackingSessions(undefined, limit, scope());
			const fresh = new Map(page.sessions.map((session) => [session.id, session]));
			sessions = sessions.map((session) => fresh.get(session.id) ?? session);
		} catch {
			// Keep the rows shown; the next write re-reads.
		}
		await refreshArmourMarks();
	}

	/** Follow armour recordings and healing corrections from any window,
	 * both of which move session costs; returns the detach function. */
	async function subscribeCostChanges(): Promise<UnlistenFn> {
		const stops = await Promise.all(
			[PROTECTION_TOPIC, HEALING_TOPIC].map((topic) => listen(topic, () => void refreshCosts())),
		);
		return () => {
			for (const stop of stops) stop();
		};
	}

	async function loadSessions() {
		loading = true;
		error = null;
		// A reload is a fresh read of a possibly different definition, so the
		// pager and any open row go back to the top rather than pointing
		// into the previous scope's window.
		table.page = 0;
		expandedSessionId = null;
		expandedDetail = null;
		try {
			const page = await getTrackingSessions(undefined, undefined, scope());
			sessions = page.sessions;
			nextCursor = page.nextCursor;
			total = page.total;
		} catch (e) {
			error = describeError(e, 'Failed to load sessions');
		} finally {
			loading = false;
		}
		await refreshArmourMarks();
	}

	// Fetch the next keyset page and append it, growing the client
	// paginator's range. Older sessions stay reachable without loading the
	// whole history up front.
	async function loadMoreSessions() {
		if (!nextCursor || loadingMore) return;
		error = null;
		loadingMore = true;
		try {
			const page = await getTrackingSessions(nextCursor, undefined, scope());
			sessions = [...sessions, ...page.sessions];
			nextCursor = page.nextCursor;
			total = page.total;
		} catch (e) {
			error = describeError(e, 'Failed to load more sessions');
		} finally {
			loadingMore = false;
		}
		await refreshArmourMarks();
	}

	// Pager bounds from the server total: the client pages the loaded
	// window, and stepping past it fetches the next keyset page on demand.
	const totalPages = $derived(Math.max(1, Math.ceil(total / PAGE_SIZE)));

	async function nextPage() {
		const nextStart = (table.page + 1) * PAGE_SIZE;
		if (nextStart >= total) return;
		if (nextStart >= sessions.length && nextCursor) await loadMoreSessions();
		if (nextStart < sessions.length) table.page++;
	}

	function prevPage() {
		if (table.page > 0) table.page--;
	}

	async function toggleSession(id: string) {
		if (expandedSessionId === id) {
			expandedSessionId = null;
			expandedDetail = null;
			return;
		}

		expandedSessionId = id;
		expandedDetail = null;
		loadingDetail = true;
		try {
			expandedDetail = await getSessionDetail(id);
		} catch {
			expandedDetail = null;
		} finally {
			loadingDetail = false;
		}
	}

	async function handleDelete(id: string) {
		if (deleting) return;
		error = null;
		deleting = true;
		try {
			await deleteSession(id);
			sessions = sessions.filter((s) => s.id !== id);
			total = Math.max(0, total - 1);
			if (expandedSessionId === id) {
				expandedSessionId = null;
				expandedDetail = null;
			}
		} catch (e) {
			error = describeError(e, 'Failed to delete session');
		}
		deleting = false;
		confirmDeleteId = null;
	}

	/** Move an instance to another definition. Under a scoped read the row
	 * leaves this list, so it is dropped locally rather than refetched;
	 * unscoped it stays, and only its stamped name may have moved, which
	 * the reopened detail carries. */
	async function reassign(id: string, definitionId: string): Promise<boolean> {
		if (reassigning) return false;
		error = null;
		reassigning = true;
		try {
			await reassignSession(id, definitionId);
			if (scope() !== undefined) {
				sessions = sessions.filter((s) => s.id !== id);
				total = Math.max(0, total - 1);
				if (expandedSessionId === id) {
					expandedSessionId = null;
					expandedDetail = null;
				}
			} else if (expandedSessionId === id) {
				expandedDetail = await getSessionDetail(id).catch(() => expandedDetail);
			}
			return true;
		} catch (e) {
			error = describeError(e, 'Failed to move the session');
			return false;
		} finally {
			reassigning = false;
		}
	}

	function collapseAll() {
		expandedSessionId = null;
		expandedDetail = null;
	}

	return {
		table,

		get sessions() {
			return sessions;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get nextCursor() {
			return nextCursor;
		},
		get loadingMore() {
			return loadingMore;
		},
		get total() {
			return total;
		},
		get totalPages() {
			return totalPages;
		},
		get expandedSessionId() {
			return expandedSessionId;
		},
		get expandedDetail() {
			return expandedDetail;
		},
		/** Writable so the detail view's own refetch (which a mob rename
		 * forces, because the backend regroups the breakdown) lands back
		 * here rather than only inside that component. */
		set expandedDetail(value: SessionDetail | null) {
			expandedDetail = value;
		},
		get loadingDetail() {
			return loadingDetail;
		},
		get confirmDeleteId() {
			return confirmDeleteId;
		},
		set confirmDeleteId(value: string | null) {
			confirmDeleteId = value;
		},
		get deleting() {
			return deleting;
		},
		get reassigning() {
			return reassigning;
		},

		/** The session's armour cost awaits a recording, so its net is not final. */
		armourPending(id: string): boolean {
			return armourPending.has(id);
		},

		loadSessions,
		loadMoreSessions,
		refreshCosts,
		subscribeCostChanges,
		nextPage,
		prevPage,
		toggleSession,
		handleDelete,
		reassign,
		collapseAll,
	};
}

export type InstancesModel = ReturnType<typeof createInstancesModel>;
