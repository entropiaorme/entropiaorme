/**
 * The stock check: the app's stock compared with what the account shows,
 * from the latest reading or from an inventory pasted from the account
 * site, and each difference settled the way the player says.
 *
 * The draft lives in the backend. Every change here is one revision sent to
 * it, in the order made, and the draft it answers with is the one shown, so
 * whatever else edits the draft edits the same one. A typed filter is
 * matched by the app's one search and reaches the backend as the items it
 * matched.
 *
 * Read from the main window only: the account commands are refused to
 * every other window.
 */

import {
	applyStockReconciliation,
	getStockReconciliationDraft,
	planStockReconciliation,
	planStockReconciliationFromPaste,
	reviseStockReconciliation,
	undoStockReconciliation,
} from '$lib/api/account';
import type {
	StockReconciliationApplied,
	StockReconciliationRevision,
} from '$lib/api/commands.gen';
import { createAccountStatus } from '$lib/features/account/accountStatus.svelte';
import { trackingSnapshot } from '$lib/stores/trackingStore.svelte';
import { describeError } from '$lib/view/errorState';
import { createSearchIndex } from '$lib/view/search';
import {
	asSentence,
	bulkOutcome,
	type Choice,
	type Draft,
	type Line,
	splitSettled,
	standaloneChoice,
} from './reconciliation';

export type CheckStage = 'start' | 'review' | 'done';

/** Where a refused change is said: beside the line it was made on, on the
 * selection bar, or under the Shrapnel sold at TT. */
export type RevisionScope =
	| { kind: 'line'; item: string }
	| { kind: 'bulk' }
	| { kind: 'shrapnel' };

export const TRACKING_REFUSAL =
	'Stop tracking to check your stock: what the session loots is still landing.';
export const NO_READING_REFUSAL =
	'Your account has not been read yet. Connect it in Settings, or paste an inventory below.';

export function createReconciliationModel() {
	let stage = $state<CheckStage>('start');
	let draft = $state<Draft | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let planning = $state<'reading' | 'paste' | null>(null);
	let readingError = $state<string | null>(null);
	let pasteError = $state<string | null>(null);

	let query = $state('');
	let selected = $state<Set<string>>(new Set());
	let pending = $state(0);
	let revisionError = $state<{ scope: RevisionScope; message: string } | null>(null);
	/** What the last bulk choice came to, when it did not fit every line. */
	let bulkNote = $state<string | null>(null);
	/** Every revision waits for the one before, so the draft shown is the
	 * last one answered in the order the changes were made. */
	let queue: Promise<void> = Promise.resolve();

	let applying = $state(false);
	let applyError = $state<string | null>(null);
	let outcome = $state<StockReconciliationApplied | null>(null);
	/** The draft as it was when applied, which the result is read against. */
	let applied = $state<Draft | null>(null);
	let undoing = $state(false);
	let undone = $state(false);
	let undoError = $state<string | null>(null);

	const account = createAccountStatus();
	const tracking = $derived(trackingSnapshot.current?.status === 'active');
	const hasReadings = $derived(account.current?.hasReadings ?? false);
	const lastReadingAt = $derived(account.current?.lastReadingAt ?? null);
	/** Why the check cannot start from the latest reading, if it cannot. */
	const readingRefusal = $derived(
		tracking ? TRACKING_REFUSAL : account.current && !hasReadings ? NO_READING_REFUSAL : null,
	);
	/** Why a pasted inventory cannot be checked, if it cannot. */
	const pasteRefusal = $derived(tracking ? TRACKING_REFUSAL : null);

	const lines = $derived(draft?.lines ?? []);
	const index = $derived(createSearchIndex(lines, (line) => line.itemName));
	const visible = $derived(index.filter(query));
	const selectedLines = $derived(lines.filter((line) => selected.has(line.itemName)));
	const hiddenSelected = $derived(
		selectedLines.length - visible.filter((line) => selected.has(line.itemName)).length,
	);
	const allVisibleSelected = $derived(
		visible.length > 0 && visible.every((line) => selected.has(line.itemName)),
	);

	function adopt(next: Draft) {
		draft = next;
		stage = 'review';
		query = '';
		selected = new Set();
		revisionError = null;
		bulkNote = null;
		applyError = null;
		outcome = null;
		undone = false;
		undoError = null;
	}

	async function load() {
		loading = true;
		error = null;
		try {
			const [found] = await Promise.all([
				getStockReconciliationDraft(),
				trackingSnapshot.hydrate(),
				account.refresh(),
			]);
			if (found) adopt(found);
		} catch (cause) {
			error = describeError(cause, 'Could not read the stock check');
		} finally {
			loading = false;
		}
	}

	/** Follow the live session and the account; returns the detach. */
	async function subscribe(): Promise<() => void> {
		const detachAccount = account.connect();
		const stopTracking = await trackingSnapshot.subscribe();
		return () => {
			detachAccount();
			stopTracking();
		};
	}

	async function planFromReading() {
		if (planning || readingRefusal) return;
		planning = 'reading';
		readingError = null;
		try {
			adopt(await planStockReconciliation());
		} catch (cause) {
			readingError = asSentence(describeError(cause, 'Could not compare with your account'));
		} finally {
			planning = null;
		}
	}

	async function planFromPaste(text: string) {
		if (planning || pasteRefusal) return;
		if (!text.trim()) {
			pasteError = 'Paste the inventory first.';
			return;
		}
		planning = 'paste';
		pasteError = null;
		try {
			adopt(await planStockReconciliationFromPaste({ text }));
		} catch (cause) {
			pasteError = asSentence(describeError(cause, 'Could not read that inventory'));
		} finally {
			planning = null;
		}
	}

	/** Send one change; the draft answered replaces the one shown. A refused
	 * change leaves the draft as it was and says why where it was made. No
	 * change is taken while the check is being applied, and one still queued
	 * when the player starts again is dropped: it belongs to a draft no
	 * longer shown. */
	function revise(revision: StockReconciliationRevision, scope: RevisionScope): Promise<void> {
		const target = draft;
		if (!target || applying) return Promise.resolve();
		pending += 1;
		const current = () => draft?.id === target.id;
		const run = queue.then(async () => {
			try {
				if (!current()) return;
				const next = await reviseStockReconciliation({ draftId: target.id, revision });
				if (!current()) return;
				draft = next.draft;
				revisionError = null;
				bulkNote =
					scope.kind === 'bulk' && revision.kind === 'set'
						? bulkOutcome(revision.items.length, next.unchanged)
						: null;
			} catch (cause) {
				if (!current()) return;
				bulkNote = null;
				revisionError = {
					scope,
					message: asSentence(describeError(cause, 'That change did not fit')),
				};
			} finally {
				pending -= 1;
			}
		});
		queue = run;
		return run;
	}

	function setLine(line: Line, choice: Choice): Promise<void> {
		if (line.disposition.kind === choice) return Promise.resolve();
		return revise(
			{ kind: 'set', items: [line.itemName], choice },
			{ kind: 'line', item: line.itemName },
		);
	}

	/** Take a line out of its conversion; the rest of the conversion breaks
	 * up if it no longer holds both sides. */
	function setApart(line: Line): Promise<void> {
		return revise(
			{ kind: 'set', items: [line.itemName], choice: standaloneChoice(line) },
			{ kind: 'line', item: line.itemName },
		);
	}

	/** Set the selected lines to a choice: the backend sets it on the lines
	 * it fits and says why it did not fit the rest. */
	function setSelection(choice: Choice): Promise<void> {
		const items = selectedLines.map((line) => line.itemName);
		if (items.length === 0) return Promise.resolve();
		return revise({ kind: 'set', items, choice }, { kind: 'bulk' });
	}

	/** Make the selected lines one conversion: the items that went in and
	 * the one that came out. */
	async function groupSelection(): Promise<void> {
		const items = selectedLines.map((line) => line.itemName);
		await revise({ kind: 'group', items }, { kind: 'bulk' });
		if (revisionError === null) selected = new Set();
	}

	/** Bring a previously accepted line back into review. */
	function reconsider(item: string): Promise<void> {
		return revise({ kind: 'reconsider', items: [item] }, { kind: 'line', item });
	}

	function setShrapnelSold(quantity: number | null): Promise<void> {
		return revise(
			{ kind: 'shrapnelSoldAtTt', quantity: quantity !== null && quantity > 0 ? quantity : null },
			{ kind: 'shrapnel' },
		);
	}

	function toggle(item: string) {
		const next = new Set(selected);
		if (next.has(item)) next.delete(item);
		else next.add(item);
		selected = next;
	}

	/** Select every line the filter shows, or clear them when all are. */
	function toggleVisible() {
		const next = new Set(selected);
		if (allVisibleSelected) for (const line of visible) next.delete(line.itemName);
		else for (const line of visible) next.add(line.itemName);
		selected = next;
	}

	function clearSelection() {
		selected = new Set();
	}

	async function apply() {
		const target = draft;
		if (!target || applying) return;
		applying = true;
		applyError = null;
		try {
			// Changes still on their way land first.
			await queue;
			const result = await applyStockReconciliation({ draftId: target.id });
			if (result.outcome === 'sessionTracked') {
				applyError = TRACKING_REFUSAL;
				return;
			}
			applied = draft;
			outcome = result;
			stage = 'done';
		} catch (cause) {
			applyError = asSentence(describeError(cause, 'Could not apply the check'));
		} finally {
			applying = false;
		}
	}

	/** The reconciliation applied, while one stands to take back. */
	const reconciliationId = $derived(
		outcome && outcome.outcome !== 'sessionTracked' ? outcome.reconciliationId : null,
	);
	/** The items applying settled, split into those it recorded something
	 * for and those it left as they are, by the draft as it was applied. */
	const settled = $derived(
		splitSettled(outcome?.outcome === 'applied' ? outcome.settled : [], applied),
	);
	/** Whether there is anything to take back: a check that recorded
	 * nothing has nothing to undo. */
	const canUndo = $derived(reconciliationId !== null && settled.changed.length > 0);

	async function undo() {
		if (!reconciliationId || !canUndo || undoing || undone) return;
		undoing = true;
		undoError = null;
		try {
			await undoStockReconciliation({ id: reconciliationId });
			undone = true;
		} catch (cause) {
			undoError = asSentence(describeError(cause, 'Could not take the check back'));
		} finally {
			undoing = false;
		}
	}

	/** Back to choosing where to compare from; the draft stays in the
	 * backend until another check replaces it. */
	function startOver() {
		stage = 'start';
		draft = null;
		applied = null;
		outcome = null;
		revisionError = null;
		bulkNote = null;
		readingError = null;
		pasteError = null;
		undone = false;
		undoError = null;
	}

	return {
		get stage() {
			return stage;
		},
		get draft() {
			return draft;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get tracking() {
			return tracking;
		},
		get lastReadingAt() {
			return lastReadingAt;
		},
		get readingRefusal() {
			return readingRefusal;
		},
		get pasteRefusal() {
			return pasteRefusal;
		},
		get planning() {
			return planning;
		},
		get readingError() {
			return readingError;
		},
		get pasteError() {
			return pasteError;
		},
		get query() {
			return query;
		},
		set query(value: string) {
			query = value;
		},
		get visible() {
			return visible;
		},
		get selected(): ReadonlySet<string> {
			return selected;
		},
		get selectedLines() {
			return selectedLines;
		},
		get hiddenSelected() {
			return hiddenSelected;
		},
		get allVisibleSelected() {
			return allVisibleSelected;
		},
		/** Whether a change is still on its way to the backend. */
		get revising() {
			return pending > 0;
		},
		get revisionError() {
			return revisionError;
		},
		get bulkNote() {
			return bulkNote;
		},
		get applying() {
			return applying;
		},
		get applyError() {
			return applyError;
		},
		get outcome() {
			return outcome;
		},
		get reconciliationId() {
			return reconciliationId;
		},
		get settled() {
			return settled;
		},
		get canUndo() {
			return canUndo;
		},
		get undoing() {
			return undoing;
		},
		get undone() {
			return undone;
		},
		get undoError() {
			return undoError;
		},
		load,
		subscribe,
		planFromReading,
		planFromPaste,
		setLine,
		setApart,
		setSelection,
		groupSelection,
		reconsider,
		setShrapnelSold,
		toggle,
		toggleVisible,
		clearSelection,
		apply,
		undo,
		startOver,
	};
}

export type ReconciliationModel = ReturnType<typeof createReconciliationModel>;
