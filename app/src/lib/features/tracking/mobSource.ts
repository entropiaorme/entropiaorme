/**
 * What the overlay's mob control shows, as pure functions over the
 * session's mob source.
 *
 * A session names its kills' mob from one source the player steers.
 * While the screen is read the control shows one mob, the one standing
 * highest: the pinned mob; else the one the screen reads now; else the
 * one the latest kills were; else the typed backstop; else nothing. Beside
 * it is one action: pin what is shown, or release the pin. Expanding it
 * reveals the others in plain words. Without screen reading (detection off
 * or unavailable, and nothing pinned) the control is the typed mob alone,
 * as it always was.
 */

import type { DetectedCreature, MobSourceView } from '$lib/api';

/** Where the mob shown comes from. */
export type MobStandingKind = 'pinned' | 'reading' | 'continuity' | 'backstop';

export type MobStanding =
	| { kind: MobStandingKind; creature: DetectedCreature }
	| { kind: 'none'; creature: null };

/** Whether the control is steered (screen reading, or a pin to release)
 * rather than the typed mob alone. */
export function isSteered(view: MobSourceView | null | undefined): boolean {
	return Boolean(view && (view.readingScreen || view.pin));
}

/** The one mob the control shows by default. */
export function standing(view: MobSourceView | null | undefined): MobStanding {
	if (!view) return { kind: 'none', creature: null };
	if (view.pin) return { kind: 'pinned', creature: view.pin.creature };
	if (view.readingScreen) {
		if (view.reading) return { kind: 'reading', creature: view.reading };
		if (view.continuity) return { kind: 'continuity', creature: view.continuity };
	}
	if (view.typed) return { kind: 'backstop', creature: view.typed };
	return { kind: 'none', creature: null };
}

/** The one action beside the mob shown: release a pin, or pin what
 * is shown while the screen is read; nothing otherwise. */
export function primaryAction(view: MobSourceView | null | undefined): 'pin' | 'release' | null {
	if (!view) return null;
	if (view.pin) return 'release';
	if (!view.readingScreen) return null;
	return standing(view).kind === 'none' ? null : 'pin';
}

/** One line of the expanded control. */
export interface MobSourceLine {
	kind: 'reading' | 'continuity' | 'backstop';
	label: string;
	creature: DetectedCreature | null;
	/** Said under the line, where it prevents a mistake. */
	note: string | null;
}

/** What expanding the control reveals, in plain words: what the screen reads
 * now and what recent kills suggest (each only while the screen is read and
 * it names something), then the typed backstop, always, so it can be set. */
export function expandedLines(view: MobSourceView | null | undefined): MobSourceLine[] {
	const lines: MobSourceLine[] = [];
	if (view?.readingScreen && view.reading) {
		lines.push({ kind: 'reading', label: 'Reading now', creature: view.reading, note: null });
	}
	if (view?.readingScreen && view.continuity) {
		lines.push({
			kind: 'continuity',
			label: 'Recent kills',
			creature: view.continuity,
			note: 'Kills not read take this. After you change mob, pin the new one if its name is not being read.',
		});
	}
	lines.push({
		kind: 'backstop',
		label: 'Your backstop',
		creature: view?.typed ?? null,
		note: view?.pin ? null : 'Used when nothing else names a kill.',
	});
	return lines;
}

/** The pin request for what is shown: the mob the screen reads (or the
 * latest kills were) is pinned as read, provided the screen still shows it;
 * the backstop alone is pinned as typed. */
export function pinShownTarget(
	view: MobSourceView | null | undefined,
):
	| { kind: 'shown'; creature: DetectedCreature }
	| { kind: 'typed'; species: string; maturity: string | null }
	| null {
	const shown = standing(view);
	switch (shown.kind) {
		case 'reading':
		case 'continuity':
			return { kind: 'shown', creature: shown.creature };
		case 'backstop':
			return {
				kind: 'typed',
				species: shown.creature.species,
				maturity: shown.creature.maturity || null,
			};
		default:
			return null;
	}
}
