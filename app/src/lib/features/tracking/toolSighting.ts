/**
 * What the after-session review of tools read off the screen says, as pure
 * functions over the wire shapes: what each sighting was, how it was
 * settled, what a settlement did, and why one was refused, in the
 * player's words.
 */

import type {
	ToolEntryKeptBecause,
	ToolSighted,
	ToolSighting,
	ToolSightingChange,
	ToolSightingRefusal,
	ToolSightingSettled,
} from '$lib/api';

/** The name a sighting is shown by: the tool's, or the text read, quoted. */
export function sightingName(sighting: ToolSighting): string {
	return sighting.sighted.kind === 'bundled' ? sighting.sighted.name : `“${sighting.sighted.text}”`;
}

function shots(count: number): string {
	return `${count} ${count === 1 ? 'shot' : 'shots'}`;
}

/** What happened while it was in hand. */
export function describeSighted(sighted: ToolSighted): string {
	if (sighted.kind === 'unknown') return 'Seen on the tool bar';
	switch (sighted.tool) {
		case 'weapon':
			if (sighted.shots === null) return 'Held, costed without attachments; shots not counted yet';
			return sighted.shots > 0
				? `Held for ${shots(sighted.shots)}, costed without attachments`
				: 'Held, no shots recorded';
		case 'harvesting':
			return 'Held, costed at its catalogue cost';
		case 'healing':
			return 'Held; its heals were not costed';
	}
}

/** Whether the player can add it to their equipment: a tool of the
 * game's items still to be settled, once its session has ended. */
export function canAdd(sighting: ToolSighting, ended: boolean): boolean {
	return ended && sighting.settled === null && sighting.sighted.kind === 'bundled';
}

/** The one quiet line a settled sighting collapses to. */
export function describeSettled(settled: ToolSightingSettled): string {
	switch (settled.kind) {
		case 'added':
			return settled.created ? 'Added to equipment' : 'Already in equipment';
		case 'dismissed':
			return 'Not a tool you track';
		case 'left':
			return 'Left';
	}
}

/** Why the entry an undone addition made stays, as a short true line. */
export function describeKept(because: ToolEntryKeptBecause): string {
	switch (because) {
		case 'changed':
			return 'it has been changed since';
		case 'notInGameData':
			return 'its tool is no longer in the game’s items';
		case 'otherSighting':
			return 'another detected tool uses it';
		case 'inSession':
			return 'a session may have used it since';
		case 'referenced':
			return 'a correction or a recorded use refers to it';
		case 'inSettings':
			return 'it is on the hotbar or among the carried tools';
	}
}

/** What a settlement or its undoing did, as one line; null when the row
 * itself says it all. */
export function describeChange(change: ToolSightingChange, name: string): string | null {
	switch (change.kind) {
		case 'added': {
			const label = change.entry.label || name;
			if (!change.entry.enabled) {
				return `${label} is in equipment but turned off, so it reads as unclear until it is on.`;
			}
			const added = change.created
				? `${label} added to equipment.`
				: `${label} is already in equipment.`;
			switch (change.uses) {
				case 'notCosted':
					return `${added} Its heals this session were not costed; mark them as paid in the healing review.`;
				case 'asBooked':
					return `${added} This session stays costed as it was.`;
				case 'withoutAttachments':
					return `${added} This session stays costed without attachments; set its attachments up in Equipment for the next.`;
			}
			break;
		}
		case 'dismissed':
			return `${name} will not be asked about again.`;
		case 'left':
			return null;
		case 'reopened':
			switch (change.entry.kind) {
				case 'removed':
					return `${name} removed from equipment.`;
				case 'kept':
					return `${name} stays in equipment: ${describeKept(change.entry.because)}.`;
				case 'none':
					return null;
			}
	}
	return null;
}

/** An entry an addition linked to that is turned off: offered to be
 * turned on, or the tool keeps reading as unclear. */
export function entryToTurnOn(change: ToolSightingChange): { id: string; label: string } | null {
	return change.kind === 'added' && !change.entry.enabled
		? { id: change.entry.id, label: change.entry.label }
		: null;
}

/** Why a settlement was refused, in the player's words. */
export function describeRefusal(refusal: ToolSightingRefusal): string {
	switch (refusal.kind) {
		case 'notFound':
			return 'That is no longer in this session.';
		case 'otherSession':
			return 'That belongs to another session.';
		case 'alreadySettled':
			switch (refusal.settled) {
				case 'added':
					return 'Already added to equipment.';
				case 'dismissed':
					return 'Already marked as not a tool you track.';
				case 'left':
					return 'Already left.';
			}
			break;
		case 'notSettled':
			return 'There is nothing to undo.';
		case 'sessionRunning':
			return 'Tools can be added once the session has ended.';
		case 'notATool':
			return 'Only a tool from the game’s items can be added.';
		case 'notInGameData':
			return 'This tool is no longer in the game’s items.';
		case 'needsSetUp':
			return 'Add this one from Equipment, where its healing can be set.';
	}
	return 'That could not be done.';
}
