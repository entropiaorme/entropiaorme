/**
 * Runes-native state for a session's weapon attribution review.
 *
 * A correction (pricing a shot from a weapon, or marking a hit as an
 * effect's tick) answers with the session's refreshed detail, which the
 * owner applies; the review list then re-reads so the corrected shot
 * changes in place. Every correction is undoable, so a single one asks for
 * no confirmation; assigning a session's unpriced shots all at once states
 * what it will do before it does it, since it moves many figures at once.
 */

import {
	assignFittingWeaponShots,
	assignWeaponShot,
	getWeaponBulkOptions,
	getWeaponCorrectionWeapons,
	getWeaponShots,
	markWeaponShotEffectTick,
	undoWeaponAssignment,
	undoWeaponBatch,
	type WeaponBulkAssignment,
	type WeaponBulkOption,
	type WeaponCorrectionWeapon,
	type WeaponShot,
	type WeaponShotGroup,
} from '$lib/api';
import type { SessionDetail } from '$lib/types/tracking';
import { describeError } from '$lib/view/errorState';
import { describeBulkDone, reviewGroups } from './weaponReview';

/** Shots fetched per page. */
export const SHOT_PAGE = 50;
/** The most rows one in-place re-read asks for (the backend's page cap). */
const REFRESH_LIMIT = 200;

export type WeaponsState =
	| { status: 'loading' }
	| { status: 'ready'; weapons: WeaponCorrectionWeapon[] }
	| { status: 'error'; message: string };

export type BulkOptionsState =
	| { status: 'idle' }
	| { status: 'loading' }
	| { status: 'ready'; options: WeaponBulkOption[] }
	| { status: 'error'; message: string };

/** The busy key while a bulk assignment runs. */
export const BULK_BUSY = 'bulk';

export interface WeaponReviewOptions {
	/** The session detail currently shown. */
	detail: () => SessionDetail;
	/** Replace the shown detail with an assignment's refreshed one. */
	apply: (fresh: SessionDetail) => void;
}

export function createWeaponReviewModel(options: WeaponReviewOptions) {
	let busy = $state<string | null>(null);
	let error = $state<string | null>(null);
	let reviewOpen = $state(false);
	let group = $state<WeaponShotGroup>('unresolved');
	let shots = $state<WeaponShot[]>([]);
	let total = $state(0);
	let loadingShots = $state(false);
	let shotsError = $state<string | null>(null);
	let weapons = $state<Record<string, WeaponsState>>({});
	let bulkOptions = $state<BulkOptionsState>({ status: 'idle' });
	// The weapon chosen to take every shot it fits, awaiting confirmation.
	let pendingBulk = $state<WeaponBulkOption | null>(null);
	// What the last bulk assignment did, until the next correction.
	let bulkNotice = $state<string | null>(null);
	// Guards against a slow page landing after the group moved on.
	let generation = 0;

	const summary = $derived(options.detail().weaponAttribution);
	const groups = $derived(reviewGroups(summary));

	/** Keep the chosen group while it has shots; otherwise the first that does. */
	function settleGroup() {
		if (!groups.some((candidate) => candidate.id === group) && groups.length > 0) {
			group = groups[0].id;
		}
	}

	async function fetchShots(offset: number, limit: number, append: boolean) {
		const sessionId = options.detail().sessionId;
		const asked = group;
		const run = ++generation;
		loadingShots = true;
		shotsError = null;
		try {
			const page = await getWeaponShots(sessionId, asked, offset, limit);
			if (run !== generation) return;
			shots = append ? [...shots, ...page.shots] : page.shots;
			total = page.total;
		} catch (e) {
			if (run === generation) shotsError = describeError(e, 'Could not load these shots.');
		} finally {
			if (run === generation) loadingShots = false;
		}
	}

	/** Re-read the loaded rows in place, keeping how far the player had
	 * scrolled into the list. */
	async function reloadShots() {
		if (!reviewOpen) return;
		settleGroup();
		const limit = Math.min(Math.max(shots.length, SHOT_PAGE), REFRESH_LIMIT);
		await fetchShots(0, limit, false);
	}

	async function act(id: string, run: () => Promise<SessionDetail>) {
		if (busy !== null) return;
		busy = id;
		error = null;
		bulkNotice = null;
		try {
			options.apply(await run());
			weapons = {};
			bulkOptions = { status: 'idle' };
			await reloadShots();
		} catch (e) {
			error = describeError(e, 'The correction could not be saved.');
		} finally {
			busy = null;
		}
	}

	return {
		get summary() {
			return summary;
		},
		get groups() {
			return groups;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		get reviewOpen() {
			return reviewOpen;
		},
		get group() {
			return group;
		},
		get shots() {
			return shots;
		},
		get total() {
			return total;
		},
		get hasMore() {
			return shots.length < total;
		},
		get loadingShots() {
			return loadingShots;
		},
		get shotsError() {
			return shotsError;
		},
		weaponsFor(shotId: string): WeaponsState | undefined {
			return weapons[shotId];
		},
		get bulkOptions() {
			return bulkOptions;
		},
		get pendingBulk() {
			return pendingBulk;
		},
		get bulkNotice() {
			return bulkNotice;
		},
		dismissError() {
			error = null;
		},

		/** Price a shot left without a price as one shot of a weapon. */
		assign(shotId: string, equipmentId: number) {
			return act(shotId, () => assignWeaponShot(shotId, equipmentId));
		},
		/** Mark an unresolved hit as a tick of an effect open when it landed. */
		markTick(shotId: string, windowId: string) {
			return act(shotId, () => markWeaponShotEffectTick(shotId, windowId));
		},
		undo(correctionId: string) {
			return act(correctionId, () => undoWeaponAssignment(correctionId));
		},

		/** Fetch the weapons the session's unpriced shots could all go to. */
		async loadBulkOptions() {
			if (bulkOptions.status === 'loading' || bulkOptions.status === 'ready') return;
			bulkOptions = { status: 'loading' };
			try {
				const offered = await getWeaponBulkOptions(options.detail().sessionId);
				bulkOptions = { status: 'ready', options: offered };
			} catch (e) {
				bulkOptions = { status: 'error', message: describeError(e, 'Could not load weapons.') };
			}
		},
		/** Choose the weapon to take every shot it fits; nothing moves until confirmed. */
		chooseBulk(option: WeaponBulkOption) {
			pendingBulk = option;
			bulkNotice = null;
		},
		cancelBulk() {
			pendingBulk = null;
		},
		/** Assign every unpriced shot the chosen weapon fits to it, as one batch. */
		async confirmBulk() {
			const chosen = pendingBulk;
			if (!chosen) return;
			const outcome: { done?: WeaponBulkAssignment } = {};
			await act(BULK_BUSY, async () => {
				outcome.done = await assignFittingWeaponShots(
					options.detail().sessionId,
					chosen.equipmentId,
				);
				return outcome.done.detail;
			});
			if (outcome.done) {
				pendingBulk = null;
				bulkNotice = describeBulkDone(outcome.done, chosen.name);
			}
		},
		/** Undo a bulk assignment's shots still assigned by it. */
		undoBatch(batchId: string) {
			return act(batchId, () => undoWeaponBatch(batchId));
		},

		async toggleReview() {
			reviewOpen = !reviewOpen;
			if (!reviewOpen) return;
			settleGroup();
			await fetchShots(0, SHOT_PAGE, false);
		},
		async setGroup(next: WeaponShotGroup) {
			if (next === group && shots.length > 0) return;
			group = next;
			shots = [];
			total = 0;
			await fetchShots(0, SHOT_PAGE, false);
		},
		async loadMore() {
			if (loadingShots || shots.length >= total) return;
			await fetchShots(shots.length, SHOT_PAGE, true);
		},

		/** Fetch the weapons a shot could be assigned to, once per shot. */
		async loadWeapons(shotId: string) {
			const known = weapons[shotId];
			if (known && known.status !== 'error') return;
			weapons = { ...weapons, [shotId]: { status: 'loading' } };
			try {
				const offered = await getWeaponCorrectionWeapons(shotId);
				weapons = { ...weapons, [shotId]: { status: 'ready', weapons: offered } };
			} catch (e) {
				weapons = {
					...weapons,
					[shotId]: { status: 'error', message: describeError(e, 'Could not load weapons.') },
				};
			}
		},

		/** The detail changed under us (an assignment from elsewhere): follow it. */
		refresh() {
			weapons = {};
			bulkOptions = { status: 'idle' };
			return reloadShots();
		},
	};
}

export type WeaponReviewModel = ReturnType<typeof createWeaponReviewModel>;
