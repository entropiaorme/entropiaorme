/**
 * Runes-native state for the Equipment armour tab: the limited-set
 * catalogue, what is still unrecorded, and the recent recordings, which
 * can be reviewed and the latest of each stream undone. Recording itself
 * lives in the overlay's Cost popup, where the player is when they repair
 * or scan; this tab re-reads whenever any protection write lands.
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
	archiveProtectionSet,
	createProtectionSet,
	getProtectionOverview,
	PROTECTION_TOPIC,
	type ProtectionCostWindow,
	type ProtectionOverview,
	type ProtectionSet,
	type ProtectionSetKind,
	type ProtectionUndoTarget,
	restoreProtectionSet,
	undoProtectionRecording,
	updateProtectionSet,
} from '$lib/api';
import { formatPed } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';

/** What an undo confirmation is about to take back. */
export interface PendingUndo {
	readonly target: ProtectionUndoTarget;
	readonly title: string;
	readonly detail: string;
}

const EMPTY: ProtectionOverview = {
	sets: [],
	removedSets: [],
	unlimited: { lastRecordedAt: null, sessions: 0, hits: 0 },
	recentCostWindows: [],
	unrecorded: { sessions: 0, hits: 0 },
};

export function createProtectionModel() {
	let overview = $state<ProtectionOverview>({ ...EMPTY });
	let loading = $state(true);
	let saving = $state(false);
	let error = $state<string | null>(null);

	let setModalOpen = $state(false);
	let editingSetId = $state<string | null>(null);
	let setKind = $state<ProtectionSetKind>('armour');
	let setName = $state('');
	let setMarkup = $state('100');
	let removalTarget = $state<ProtectionSet | null>(null);
	let pendingUndo = $state<PendingUndo | null>(null);
	let expandedWindowId = $state<string | null>(null);
	let guide = false;

	const armourSets = $derived(overview.sets.filter((set) => set.kind === 'armour'));
	const plateSets = $derived(overview.sets.filter((set) => set.kind === 'plates'));
	const editingSet = $derived(
		editingSetId ? (overview.sets.find((set) => set.id === editingSetId) ?? null) : null,
	);
	const markupValue = $derived(Number(setMarkup));
	const setSaveDisabled = $derived(
		!setName.trim() || !Number.isFinite(markupValue) || markupValue < 100 || saving,
	);

	async function load(guideMode = false): Promise<void> {
		guide = guideMode;
		loading = true;
		error = null;
		try {
			overview = guideMode ? { ...EMPTY } : await getProtectionOverview();
		} catch (cause) {
			error = describeError(cause, 'Failed to load armour');
		} finally {
			loading = false;
		}
	}

	/** Re-read in place after a write elsewhere, keeping what is shown on failure. */
	async function refresh(): Promise<void> {
		if (guide) return;
		try {
			overview = await getProtectionOverview();
		} catch {
			// The next write re-reads; a transient failure keeps the last good overview.
		}
	}

	/** Follow protection writes from any window; returns the detach function. */
	function subscribe(): Promise<UnlistenFn> {
		return listen(PROTECTION_TOPIC, () => void refresh());
	}

	async function mutate(
		action: () => Promise<ProtectionOverview>,
		failure: string,
	): Promise<boolean> {
		saving = true;
		error = null;
		try {
			overview = await action();
			return true;
		} catch (cause) {
			error = describeError(cause, failure);
			return false;
		} finally {
			saving = false;
		}
	}

	function sessionsText(count: number): string {
		return `${count} ${count === 1 ? 'session' : 'sessions'}`;
	}

	function askUndoWindow(window: ProtectionCostWindow): void {
		const name =
			window.kind === 'repair'
				? 'this unlimited repair'
				: `this ${window.setName ?? 'limited set'} reading`;
		const reach =
			window.allocations.length === 0
				? 'It reached no session.'
				: `Its ${formatPed(window.costPed)} PED comes off ${sessionsText(window.allocations.length)}, which the next recording will offer again.`;
		pendingUndo = {
			target: { kind: 'recording', windowId: Number(window.id) },
			title: `Undo ${name}?`,
			detail:
				window.kind === 'repair'
					? reach
					: `${reach} The set's previous reading becomes its baseline again.`,
		};
	}

	function askUndoReading(set: ProtectionSet): void {
		const reading = set.latestObservation;
		if (!reading || reading.measured) return;
		pendingUndo = {
			target: { kind: 'reading', observationId: Number(reading.id) },
			title: `Undo the ${set.name} reading?`,
			detail: `The ${formatPed(reading.ttValuePed)} PED reading booked nothing. The set's reading before it, if any, becomes its baseline again.`,
		};
	}

	async function confirmUndo(): Promise<void> {
		const pending = pendingUndo;
		if (!pending) return;
		if (
			await mutate(() => undoProtectionRecording(pending.target), 'Failed to undo the recording')
		) {
			pendingUndo = null;
		}
	}

	async function restoreSet(set: ProtectionSet): Promise<void> {
		await mutate(() => restoreProtectionSet(set.id), 'Failed to restore the set');
	}

	function openSet(kind: ProtectionSetKind): void {
		editingSetId = null;
		setKind = kind;
		setName = '';
		setMarkup = '100';
		setModalOpen = true;
	}

	function editSet(set: ProtectionSet): void {
		editingSetId = set.id;
		setKind = set.kind;
		setName = set.name;
		setMarkup = String(set.markupPercent);
		setModalOpen = true;
	}

	async function saveSet(): Promise<void> {
		if (setSaveDisabled) return;
		saving = true;
		error = null;
		try {
			overview = editingSetId
				? await updateProtectionSet(editingSetId, {
						name: setName.trim(),
						markupPercent: markupValue,
					})
				: await createProtectionSet({
						kind: setKind,
						name: setName.trim(),
						markupPercent: markupValue,
					});
			setModalOpen = false;
		} catch (cause) {
			error = describeError(cause, 'Failed to save the set');
		} finally {
			saving = false;
		}
	}

	async function confirmRemoval(): Promise<void> {
		if (!removalTarget) return;
		saving = true;
		error = null;
		try {
			overview = await archiveProtectionSet(removalTarget.id);
			removalTarget = null;
		} catch (cause) {
			error = describeError(cause, 'Failed to remove the set');
		} finally {
			saving = false;
		}
	}

	return {
		get overview() {
			return overview;
		},
		get loading() {
			return loading;
		},
		get saving() {
			return saving;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get armourSets() {
			return armourSets;
		},
		get plateSets() {
			return plateSets;
		},
		get setModalOpen() {
			return setModalOpen;
		},
		set setModalOpen(value: boolean) {
			setModalOpen = value;
		},
		get editingSet() {
			return editingSet;
		},
		get setKind() {
			return setKind;
		},
		get setName() {
			return setName;
		},
		set setName(value: string) {
			setName = value;
		},
		get setMarkup() {
			return setMarkup;
		},
		set setMarkup(value: string) {
			setMarkup = value;
		},
		get setSaveDisabled() {
			return setSaveDisabled;
		},
		get removalTarget() {
			return removalTarget;
		},
		get removalModalOpen() {
			return removalTarget !== null;
		},
		set removalModalOpen(value: boolean) {
			if (!value) removalTarget = null;
		},
		get removedSets() {
			return overview.removedSets;
		},
		get pendingUndo() {
			return pendingUndo;
		},
		get undoModalOpen() {
			return pendingUndo !== null;
		},
		set undoModalOpen(value: boolean) {
			if (!value) pendingUndo = null;
		},
		get expandedWindowId() {
			return expandedWindowId;
		},
		toggleWindow(id: string) {
			expandedWindowId = expandedWindowId === id ? null : id;
		},
		load,
		refresh,
		subscribe,
		askUndoWindow,
		askUndoReading,
		confirmUndo,
		restoreSet,
		openSet,
		editSet,
		saveSet,
		askRemoveSet(set: ProtectionSet) {
			removalTarget = set;
		},
		confirmRemoval,
	};
}

export type ProtectionModel = ReturnType<typeof createProtectionModel>;
