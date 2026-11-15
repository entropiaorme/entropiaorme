/**
 * The review of measured sessions: the sessions the account readings
 * covered, newest first, each beside its tracked figures, and the pair
 * summed over the complete ones.
 *
 * Paging follows the backend's keyset pages: the first page loads with
 * the view, and older sessions load a page at a time on request, growing
 * the loaded window; the list and the sum both read that window. A
 * client-side pager walks the listed rows. An account frame (a reading
 * stored, a measure sealed) re-reads the newest page in place, keeping
 * older loaded pages, the pager, and any open row.
 */

import { listen } from '@tauri-apps/api/event';
import { ACCOUNT_TOPIC, getMeasuredSessions } from '$lib/api/account';
import type { MeasuredSessionRow } from '$lib/api/commands.gen';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { listedRows, summarise } from './measuredReview';

export const PAGE_SIZE = 20;

export function createMeasuredSessionsModel() {
	let rows = $state<MeasuredSessionRow[]>([]);
	let total = $state(0);
	let nextCursor = $state<string | null>(null);
	let loading = $state(true);
	let loadingMore = $state(false);
	let error = $state<string | null>(null);
	let completeOnly = $state(true);
	let expandedId = $state<string | null>(null);

	const listed = $derived(listedRows(rows, completeOnly));
	const table = createTableModel<MeasuredSessionRow>({ rows: () => listed, pageSize: PAGE_SIZE });
	const summary = $derived(summarise(rows));

	async function load() {
		loading = true;
		error = null;
		try {
			const page = await getMeasuredSessions(null, null);
			rows = page.sessions;
			nextCursor = page.nextCursor;
			total = page.total;
		} catch (cause) {
			error = describeError(cause, 'Could not read the measured sessions.');
		} finally {
			loading = false;
		}
	}

	/** Re-read the newest page and fold it into the loaded window: a row
	 * already loaded takes its fresh figures, a session new since the load
	 * joins at the top. A failed re-read keeps what is shown. */
	async function refresh() {
		if (loading) return;
		try {
			const page = await getMeasuredSessions(null, null);
			const fresh = new Map(page.sessions.map((row) => [row.id, row]));
			const known = new Set(rows.map((row) => row.id));
			const added = page.sessions.filter((row) => !known.has(row.id));
			rows = [...added, ...rows.map((row) => fresh.get(row.id) ?? row)];
			total = page.total;
			if (rows.length === page.sessions.length) nextCursor = page.nextCursor;
		} catch {
			// The next frame re-reads.
		}
	}

	async function loadOlder() {
		if (!nextCursor || loadingMore) return;
		loadingMore = true;
		error = null;
		try {
			const page = await getMeasuredSessions(nextCursor, null);
			const known = new Set(rows.map((row) => row.id));
			rows = [...rows, ...page.sessions.filter((row) => !known.has(row.id))];
			nextCursor = page.nextCursor;
			total = page.total;
		} catch (cause) {
			error = describeError(cause, 'Could not read older sessions.');
		} finally {
			loadingMore = false;
		}
	}

	return {
		table,
		get loading() {
			return loading;
		},
		get loadingMore() {
			return loadingMore;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		/** Every session with readings, loaded or not. */
		get total() {
			return total;
		},
		/** The sessions loaded so far. */
		get loaded() {
			return rows.length;
		},
		get hasOlder() {
			return nextCursor !== null;
		},
		get completeOnly() {
			return completeOnly;
		},
		set completeOnly(value: boolean) {
			completeOnly = value;
			table.page = 0;
		},
		get listed() {
			return listed;
		},
		get summary() {
			return summary;
		},
		get expandedId() {
			return expandedId;
		},
		toggle(id: string) {
			expandedId = expandedId === id ? null : id;
		},
		load,
		loadOlder,
		/** Load, and re-read on every account frame; returns the detach. */
		connect(): () => void {
			let detach: (() => void) | undefined;
			let closed = false;
			void listen(ACCOUNT_TOPIC, () => {
				void refresh();
			}).then((unlisten) => {
				if (closed) unlisten();
				else detach = unlisten;
			});
			void load();
			return () => {
				closed = true;
				detach?.();
			};
		},
	};
}

export type MeasuredSessionsModel = ReturnType<typeof createMeasuredSessionsModel>;
