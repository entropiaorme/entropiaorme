/**
 * Runes-native state for a session's weapon attribution review.
 *
 * An assignment answers with the session's refreshed detail, which the owner
 * applies; the review list then re-reads so a shot the assignment priced
 * changes in place. Every assignment is undoable, so none asks for
 * confirmation.
 */

import {
	assignWeaponShot,
	getWeaponCorrectionWeapons,
	getWeaponShots,
	undoWeaponAssignment,
	type WeaponCorrectionWeapon,
	type WeaponShot,
	type WeaponShotGroup,
} from '$lib/api';
import type { SessionDetail } from '$lib/types/tracking';
import { describeError } from '$lib/view/errorState';
import { reviewGroups } from './weaponReview';

/** Shots fetched per page. */
export const SHOT_PAGE = 50;
/** The most rows one in-place re-read asks for (the backend's page cap). */
const REFRESH_LIMIT = 200;

export type WeaponsState =
	| { status: 'loading' }
	| { status: 'ready'; weapons: WeaponCorrectionWeapon[] }
	| { status: 'error'; message: string };

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
		try {
			options.apply(await run());
			weapons = {};
			await reloadShots();
		} catch (e) {
			error = describeError(e, 'The assignment could not be saved.');
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
		dismissError() {
			error = null;
		},

		/** Price an unpriced shot as one shot of a weapon. */
		assign(shotId: string, equipmentId: number) {
			return act(shotId, () => assignWeaponShot(shotId, equipmentId));
		},
		undo(correctionId: string) {
			return act(correctionId, () => undoWeaponAssignment(correctionId));
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
			return reloadShots();
		},
	};
}

export type WeaponReviewModel = ReturnType<typeof createWeaponReviewModel>;
