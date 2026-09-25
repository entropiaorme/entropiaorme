/**
 * Runes-native state for the Equipment armour tab: the limited-set
 * catalogue, what is still unrecorded, and the recent recordings.
 * Recording itself lives in the overlay's Cost popup, where the player is
 * when they repair or scan.
 */

import {
	archiveProtectionSet,
	createProtectionSet,
	getProtectionOverview,
	type ProtectionOverview,
	type ProtectionSet,
	type ProtectionSetKind,
	updateProtectionSet,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';

const EMPTY: ProtectionOverview = {
	sets: [],
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
		load,
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
