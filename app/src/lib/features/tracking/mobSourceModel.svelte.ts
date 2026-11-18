/**
 * Runes-native state for the overlay's mob control: the writes that steer
 * the session's mob source, one at a time.
 *
 * While the screen is read the control shows the standing mob and opens a
 * sub-window that says where it comes from and takes a typed mob, to pin or
 * to keep as the backstop. Otherwise the control is the typed mob alone:
 * a field in the readout while none is typed, a label once one is.
 *
 * Saving is a phase: while a write runs every other one is refused, and the
 * controls say so by being disabled. Each write re-reads the snapshot after
 * it lands.
 */

import type { ManualMobSuggestion, MobPinTarget, MobSourceView } from '$lib/api';
import { describeError } from '$lib/view/errorState';
import { isSteered, pinShownTarget } from './mobSource';

export interface MobSourceDeps {
	/** The session's mob source as the snapshot last read it. */
	view: () => MobSourceView | null;
	/** Whether the control can steer at all; a control with nothing to
	 * write through is the typed mob alone. Steerable unless said. */
	steerable?: () => boolean;
	pin: (target: MobPinTarget) => Promise<unknown>;
	unpin: () => Promise<unknown>;
	/** Set the typed mob (the backstop while the screen is read). */
	setTyped: (species: string, maturity: string) => Promise<unknown>;
	/** Clear the typed mob. */
	clearTyped: () => Promise<unknown>;
	/** Re-read the snapshot. */
	refresh: () => Promise<unknown>;
}

export type MobSourceBusy = 'pin' | 'release' | 'backstop' | 'clear' | 'typed';

export function createMobSourceModel(deps: MobSourceDeps) {
	let busy = $state<MobSourceBusy | null>(null);
	let error = $state<string | null>(null);

	const steered = $derived((deps.steerable?.() ?? true) && isSteered(deps.view()));
	// The typed mob's field sits in the readout only while the control is
	// the typed mob alone and none is typed (a typed one shows as a label);
	// while steered, typing is in the control's sub-window.
	const inputShown = $derived(!steered && !deps.view()?.typed);

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
		get inputShown() {
			return inputShown;
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

		/** Pin what the control shows. */
		async pinShown() {
			const target = pinShownTarget(deps.view());
			if (!target) return false;
			return write('pin', () => deps.pin(target), 'Could not pin the mob.');
		},
		async release() {
			return write('release', () => deps.unpin(), 'Could not release the pinned mob.');
		},

		/** A mob picked from the readout's own field: the typed mob at once. */
		async choose(option: ManualMobSuggestion) {
			return write(
				'typed',
				() => deps.setTyped(option.species, option.maturity),
				'Failed to declare mob',
			);
		},
		/** Pin a mob typed in the control's sub-window. */
		async pinTyped(species: string, maturity: string) {
			return write(
				'pin',
				() => deps.pin({ kind: 'typed', species, maturity: maturity || null }),
				'Could not pin the mob.',
			);
		},
		/** Keep a mob typed in the control's sub-window as the backstop. */
		async backstop(species: string, maturity: string) {
			return write(
				'backstop',
				() => deps.setTyped(species, maturity),
				'Could not set the backstop.',
			);
		},
		/** Clear the typed mob (the backstop, or the typed mob). */
		async clearTyped() {
			return write('clear', () => deps.clearTyped(), 'Could not clear the mob.');
		},
	};
}

export type MobSourceModel = ReturnType<typeof createMobSourceModel>;
