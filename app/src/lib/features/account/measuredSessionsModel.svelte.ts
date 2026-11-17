/**
 * The review of measured sessions: the whole-history summary, and the
 * sessions the account readings covered, newest first. Where the readings
 * cover a session, its measured figures are its costs; the review compares
 * them with the tracked estimates.
 *
 * Both read through snapshot stores on the account topic: a frame (a
 * reading stored, a measure sealed) re-reads them, overlapping frames
 * coalesce into one follow-up read so an older answer can never overwrite
 * a newer one, and a frame arriving during the first read is not lost. A
 * failed read keeps what is shown and says why; the next good read clears
 * that.
 *
 * Paging follows the backend's keyset pages. Each read of the newest page
 * is folded into the loaded window (see `foldNewest`); older pages load on
 * request. A client-side pager walks the listed rows.
 */

import { ACCOUNT_TOPIC, getMeasuredSessions, getMeasuredSummary } from '$lib/api/account';
import type {
	AccountMeasuredSummary,
	MeasuredSessionPage,
	MeasuredSessionRow,
} from '$lib/api/commands.gen';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { describeError } from '$lib/view/errorState';
import { createTableModel } from '$lib/view/tableModel.svelte';
import { appendOlder, foldNewest, listedRows } from './measuredReview';

export const PAGE_SIZE = 20;

export function createMeasuredSessionsModel() {
	let rows = $state<MeasuredSessionRow[]>([]);
	let cursor = $state<string | null>(null);
	let total = $state(0);
	let read = $state(false);
	let listError = $state<string | null>(null);
	let summaryError = $state<string | null>(null);
	let olderError = $state<string | null>(null);
	let loadingOlder = $state(false);
	let completeOnly = $state(true);
	let expandedId = $state<string | null>(null);

	// The newest page; each answer is folded into the window as it lands
	// (the store runs one read at a time, so answers land in order).
	const newest = createSnapshotStore<MeasuredSessionPage>(ACCOUNT_TOPIC, async () => {
		try {
			const page = await getMeasuredSessions(null, null);
			const folded = foldNewest(rows, cursor, page);
			rows = folded.rows;
			cursor = folded.cursor;
			total = page.total;
			read = true;
			listError = null;
			return page;
		} catch (cause) {
			listError = describeError(cause, 'Could not read the measured sessions.');
			throw cause;
		}
	});

	const summary = createSnapshotStore<AccountMeasuredSummary>(ACCOUNT_TOPIC, async () => {
		try {
			const answer = await getMeasuredSummary();
			summaryError = null;
			return answer;
		} catch (cause) {
			summaryError = describeError(cause, 'Could not read the summary.');
			throw cause;
		}
	});

	const listed = $derived(listedRows(rows, completeOnly));
	const table = createTableModel<MeasuredSessionRow>({ rows: () => listed, pageSize: PAGE_SIZE });

	async function loadOlder() {
		if (!cursor || loadingOlder) return;
		loadingOlder = true;
		olderError = null;
		try {
			const page = await getMeasuredSessions(cursor, null);
			rows = appendOlder(rows, page);
			cursor = page.nextCursor;
			total = page.total;
		} catch (cause) {
			olderError = describeError(cause, 'Could not read older sessions.');
		} finally {
			loadingOlder = false;
		}
	}

	return {
		table,
		/** Whether the list has not answered yet. */
		get loading() {
			return !read && listError === null;
		},
		get loadingOlder() {
			return loadingOlder;
		},
		get error() {
			return listError ?? olderError;
		},
		dismissError() {
			olderError = null;
			listError = null;
		},
		get summary() {
			return summary.current;
		},
		get summaryError() {
			return summary.current ? null : summaryError;
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
			return cursor !== null;
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
		get expandedId() {
			return expandedId;
		},
		toggle(id: string) {
			expandedId = expandedId === id ? null : id;
		},
		loadOlder,
		/** Attach both stores to the account topic, then read; returns the
		 * detach. */
		connect(): () => void {
			const detaches: Array<() => void> = [];
			let closed = false;
			for (const store of [newest, summary]) {
				void store.subscribe().then((unlisten) => {
					if (closed) unlisten();
					else detaches.push(unlisten);
				});
				void store.hydrate();
			}
			return () => {
				closed = true;
				for (const detach of detaches) detach();
			};
		},
	};
}

export type MeasuredSessionsModel = ReturnType<typeof createMeasuredSessionsModel>;
