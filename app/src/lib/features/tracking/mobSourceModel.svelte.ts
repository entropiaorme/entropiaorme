/**
 * Runes-native state for the overlay's mob control: whether it is expanded,
 * the creature picked from the typeahead and waiting for the player to say
 * what it is for, and the writes that steer the session's mob source.
 *
 * Expanded is either sticky (the player opened it, by click or keyboard,
 * and it stays until they close it) or held by the pointer resting on the
 * control or its panel; the pointer leaving lets it close after a short
 * grace, so crossing from the control into the panel does not close it.
 * Nothing is expanded while the control is the typed mob alone.
 *
 * Saving is a phase: while a write runs every other one is refused, and the
 * controls say so by being disabled. Each write re-reads the snapshot after
 * it lands.
 */

import type { ManualMobSuggestion, MobPinTarget, MobSourceView } from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { isSteered, pinShownTarget } from './mobSource';

/** How long the panel stays after the pointer leaves it, so the pointer can
 * cross from the control into the panel below it. */
export const HOVER_CLOSE_MS = 250;

export interface MobSourceDeps {
	/** The session's mob source as the snapshot last read it. */
	view: () => MobSourceView | null;
	/** Whether surfaces still in development are shown on this build: the
	 * steered control is one; without it the control is the typed mob. */
	enabled: () => boolean;
	pin: (target: MobPinTarget) => Promise<unknown>;
	unpin: () => Promise<unknown>;
	/** Set the typed creature (the backstop while the screen is read). */
	setTyped: (species: string, maturity: string) => Promise<unknown>;
	/** Clear the typed creature. */
	clearTyped: () => Promise<unknown>;
	/** Re-read the snapshot. */
	refresh: () => Promise<unknown>;
}

export type MobSourceBusy = 'pin' | 'release' | 'backstop' | 'clear' | 'typed';

export function createMobSourceModel(deps: MobSourceDeps) {
	let sticky = $state(false);
	let hovered = $state(false);
	let draft = $state<ManualMobSuggestion | null>(null);
	let busy = $state<MobSourceBusy | null>(null);
	let error = $state<string | null>(null);
	let closeTimer: ReturnType<typeof setTimeout> | undefined;

	const steered = $derived(deps.enabled() && isSteered(deps.view()));
	const expanded = $derived(steered && (sticky || hovered));
	// The typeahead's input: in the panel while steered, else in the strip
	// while no creature is typed (a typed one shows as a label instead).
	const inputShown = $derived(steered ? expanded : !deps.view()?.typed);

	function clearCloseTimer() {
		if (closeTimer === undefined) return;
		clearTimeout(closeTimer);
		closeTimer = undefined;
	}

	async function write(kind: MobSourceBusy, action: () => Promise<unknown>, failure: string) {
		if (busy !== null) return false;
		busy = kind;
		error = null;
		try {
			await action();
			await deps.refresh();
			return true;
		} catch (e) {
			error = describeError(e, failure);
			return false;
		} finally {
			busy = null;
		}
	}

	return {
		get steered() {
			return steered;
		},
		get expanded() {
			return expanded;
		},
		get inputShown() {
			return inputShown;
		},
		get draft() {
			return draft;
		},
		get busy() {
			return busy;
		},
		get error() {
			return error;
		},
		dismissError() {
			error = null;
		},

		/** The toggle (a click or a key): open it for good, or close it once
		 * it is open for good. Open only under the pointer, a click keeps it
		 * open rather than closing what the player just reached for. */
		toggle() {
			clearCloseTimer();
			if (expanded && sticky) {
				sticky = false;
				hovered = false;
				draft = null;
			} else {
				sticky = true;
			}
		},
		/** Close it (Escape, or an action that is done with it). */
		close() {
			clearCloseTimer();
			sticky = false;
			hovered = false;
			draft = null;
		},
		pointerEnter() {
			clearCloseTimer();
			hovered = true;
		},
		pointerLeave() {
			clearCloseTimer();
			closeTimer = setTimeout(() => {
				closeTimer = undefined;
				hovered = false;
			}, HOVER_CLOSE_MS);
		},
		/** The player is working in the panel (typing, picking): keep it open
		 * whatever the pointer does. */
		hold() {
			clearCloseTimer();
			if (steered) sticky = true;
		},

		/** Pin what the control shows. */
		async pinShown() {
			const target = pinShownTarget(deps.view());
			if (!target) return false;
			return write('pin', () => deps.pin(target), 'Could not pin the creature.');
		},
		async release() {
			return write('release', () => deps.unpin(), 'Could not release the pinned creature.');
		},

		/** A creature picked from the typeahead: while steered it waits for the
		 * player to pin it or keep it as the backstop; otherwise it is the
		 * typed mob at once, as it always was. */
		async choose(option: ManualMobSuggestion) {
			if (!steered) {
				return write(
					'typed',
					() => deps.setTyped(option.species, option.maturity),
					'Failed to declare mob',
				);
			}
			draft = option;
			return true;
		},
		cancelDraft() {
			draft = null;
		},
		async pinDraft() {
			const picked = draft;
			if (!picked) return false;
			const done = await write(
				'pin',
				() =>
					deps.pin({ kind: 'typed', species: picked.species, maturity: picked.maturity || null }),
				'Could not pin the creature.',
			);
			if (done) draft = null;
			return done;
		},
		async backstopDraft() {
			const picked = draft;
			if (!picked) return false;
			const done = await write(
				'backstop',
				() => deps.setTyped(picked.species, picked.maturity),
				'Could not set the backstop.',
			);
			if (done) draft = null;
			return done;
		},
		/** Clear the typed creature (the backstop, or the typed mob). */
		async clearTyped() {
			return write('clear', () => deps.clearTyped(), 'Could not clear the creature.');
		},

		destroy() {
			clearCloseTimer();
		},
	};
}

export type MobSourceModel = ReturnType<typeof createMobSourceModel>;
