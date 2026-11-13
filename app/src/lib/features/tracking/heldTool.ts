/**
 * What the overlay says about the tool in hand while it is read off the
 * game screen, as pure functions over the session's tool source.
 *
 * While the screen is the source the strip shows what the screen shows,
 * never a tool guessed from damage: a tool by name (in the provisional tone
 * when it is known only from the game's data), a quiet "No tool", a name
 * that is not a tracked tool, muted, or "Tool unclear" in the attention
 * tone the moment the bar does not say. Expanding it says why, in the
 * player's terms, with the action that settles it. A covered bar is quiet:
 * the tool before it was covered stays, marked. The damage cues (a weapon
 * mismatch, hits above or below the listed range) stand as they do with the
 * keys, worded for the screen. While reading is turned on but the screen is
 * not being read, the hotbar's ordinary display and its cues stand, with
 * one marker saying so.
 */

import type { ToolEntry, ToolSourceView, ToolUncertainty, WeaponHitMargin } from '$lib/api';
import { listNames, NO_DATA } from '$lib/utils/format';

/** How the tool shown reads: each maps to one of the strip's tones. */
export type HeldToolTone = 'normal' | 'provisional' | 'muted' | 'quiet' | 'attention';

/** An action the expanded control offers. */
export type HeldToolAction =
	/** Declare a library entry in hand: it stands until the screen shows
	 * what is held. `name` is the entry as the player tells it apart. */
	| { kind: 'declare'; equipmentId: number; name: string; label: string; accessibleName: string }
	/** Open the equipment picker. */
	| { kind: 'choose'; label: string };

export interface HeldToolDisplay {
	/** What the strip shows in the tool's place. */
	label: string;
	tone: HeldToolTone;
	/** A small word under it, where the state needs one. */
	marker: string | null;
	/** What expanding it says, line by line. */
	lines: string[];
	actions: HeldToolAction[];
}

/** The quiet line under the tool: a hit cue, or the activity it implies. */
export interface HeldToolFeedback {
	text: string;
	/** A strip tone class. */
	tone: string;
	/** Said on hover, and in the panel. */
	detail: string | null;
	testId: string;
}

const CHOOSE: HeldToolAction = { kind: 'choose', label: 'Choose a tool' };

const UNREADABLE = { lines: ['The tool bar cannot be read.'], actions: [CHOOSE] };

function declare(entry: { id: number; label: string }, label: string): HeldToolAction {
	return {
		kind: 'declare',
		equipmentId: entry.id,
		name: entry.label,
		label,
		accessibleName: `Use ${entry.label}`,
	};
}

function declareEach(entries: ToolEntry[], single: string): HeldToolAction[] {
	return entries.map((entry) => declare(entry, entries.length === 1 ? single : 'Use this one'));
}

/** Why the bar does not say, and what settles it. Entries gone from the
 * library by the time it is shown leave the bar unreadable as far as the
 * player can act on it. */
function unclear(why: ToolUncertainty): { lines: string[]; actions: HeldToolAction[] } {
	switch (why.kind) {
		case 'twins': {
			const labels = why.entries.map((entry) => entry.label);
			if (labels.length === 0) return UNREADABLE;
			const line =
				labels.length === 1
					? `Could be: ${labels[0]}.`
					: `${labels.length === 2 ? 'Two entries' : 'These entries'} look the same on the bar: ${listNames(labels)}.`;
			return { lines: [line], actions: [...declareEach(why.entries, 'Use it'), CHOOSE] };
		}
		case 'disabled': {
			const labels = why.entries.map((entry) => entry.label);
			if (labels.length === 0) return UNREADABLE;
			const verb = labels.length === 1 ? 'is' : 'are';
			return {
				lines: [`${listNames(labels)} ${verb} turned off in your equipment.`],
				actions: [...declareEach(why.entries, 'Use it'), CHOOSE],
			};
		}
		case 'contended': {
			if (why.candidates.length === 0) return UNREADABLE;
			const names = why.candidates.map((candidate) => candidate.name).join(', ');
			const more = why.more > 0 ? ` (and ${why.more} more)` : '';
			const owned = why.candidates.flatMap((candidate) =>
				candidate.libraryId === null ? [] : [{ id: candidate.libraryId, label: candidate.name }],
			);
			return {
				lines: [`Could be: ${names}${more}.`],
				actions: [...owned.map((entry) => declare(entry, 'Use this one')), CHOOSE],
			};
		}
		case 'unknown':
			return { lines: [`Read as "${why.text}".`], actions: [CHOOSE] };
		case 'unreadable':
			return UNREADABLE;
	}
}

/**
 * What the strip shows of the tool in hand while the screen is the source,
 * or null when the hotbar's ordinary display applies.
 */
export function heldToolDisplay(source: ToolSourceView | null | undefined): HeldToolDisplay | null {
	if (!source?.readingScreen) return null;
	const held = source.held;
	if (source.manual && held.kind === 'tool') {
		return {
			label: held.name,
			tone: 'normal',
			marker: 'Chosen',
			lines: ['Chosen by you until the screen shows what is held.'],
			actions: [],
		};
	}
	switch (held.kind) {
		case 'notRead':
			return { label: NO_DATA, tone: 'quiet', marker: null, lines: ['Not read yet.'], actions: [] };
		case 'tool':
			return held.provisional
				? {
						label: held.name,
						tone: 'provisional',
						marker: null,
						lines: ['Not in your equipment: costed without attachments.'],
						actions: [],
					}
				: { label: held.name, tone: 'normal', marker: null, lines: [], actions: [] };
		case 'nothing':
			return { label: 'No tool', tone: 'quiet', marker: null, lines: [], actions: [] };
		case 'notTracked':
			return {
				label: held.name,
				tone: 'muted',
				marker: null,
				lines: ['Not a tracked tool.'],
				actions: [],
			};
		case 'uncertain':
			return { label: 'Tool unclear', tone: 'attention', marker: null, ...unclear(held.why) };
		case 'notVisible':
			return {
				label: held.last ?? NO_DATA,
				tone: held.last !== null && held.provisional ? 'provisional' : 'muted',
				marker: 'Covered',
				lines: ['The tool bar is covered.'],
				actions: [CHOOSE],
			};
	}
}

/** Whether the screen shows outright what is held now: a declaration from
 * the picker would be refused, so the picker is not offered. */
export function screenShowsTool(source: ToolSourceView | null | undefined): boolean {
	if (!source?.readingScreen || source.manual) return false;
	const kind = source.held.kind;
	return kind === 'tool' || kind === 'nothing' || kind === 'notTracked';
}

/** Why the picker is not offered while the screen shows what is held. */
export const SCREEN_SHOWS_TOOL = 'The screen shows what is held';

/** What the screen not being read means, while reading the tool in hand
 * is turned on; null otherwise. */
export function screenNotReadLine(source: ToolSourceView | null | undefined): string | null {
	switch (source?.notReading) {
		case 'stopped':
			return 'The screen stopped being read: hotbar keys are in use.';
		case 'notStarted':
			return 'Screen capture is not running: turn the setting off and on to start it. Hotbar keys are in use.';
		default:
			return null;
	}
}

/** The marker the strip shows beside the hotbar display while the screen is
 * not being read. */
export const SCREEN_NOT_READ_MARKER = 'Screen not read';

/** The tool in hand, for marking it in the equipment picker: what the screen
 * shows while it is the source (never a weapon guessed from damage), else
 * the readout's tool. */
export function toolInHand(
	source: ToolSourceView | null | undefined,
	currentTool: string | null | undefined,
): string | null {
	if (!source?.readingScreen) return currentTool ?? null;
	const held = source.held;
	if (held.kind === 'tool') return held.name;
	if (held.kind === 'notVisible') return held.last;
	return null;
}

/** The damage cues' words, for the source the tool in hand comes from: the
 * hotbar's (as they always were), or the screen's. */
export function cueCopy(readingScreen: boolean) {
	const source = readingScreen ? 'the screen shows' : 'the hotbar shows';
	return {
		weaponMismatch: (recording: string, declared: string) =>
			`Damage fits ${recording}; ${source} ${declared}`,
		confirm: (recording: string) =>
			readingScreen
				? `Confirm ${recording}: record it from now and reprice the shots since the tool in hand last changed`
				: `Confirm ${recording}: record it from now and reprice the shots since your last hotbar press`,
		harvestMismatch: (expected: string, observed: string | null) =>
			readingScreen
				? `Board output says ${expected}; the screen shows ${observed ?? 'no tool'}`
				: `Board output says ${expected}; hotbar shows ${observed ?? 'no tool'}`,
		hit: (cue: WeaponHitMargin) => {
			const check = readingScreen
				? 'check which set-up is on in Equipment.'
				: 'press the slot of the weapon you are firing.';
			return cue === 'boosted'
				? `Recent hits are above this weapon's listed damage, so they are priced as boosted. If you have no damage buff running, ${check}`
				: `Recent hits are below this weapon's listed damage, so they are priced as reduced. If that looks wrong, ${check}`;
		},
	};
}

/** The line the expanded tool control adds while a hit cue stands. */
export const HIT_CUE_CHECK = 'Check which set-up is on in Equipment.';
