/**
 * The error slot a character-surface load reports into: the page-level
 * slot the stats and legacy tabs share, or a section-local slot so one
 * failing section of the skilling hub never blanks the others. Every load
 * clears its slot on entry.
 */

export interface PageErrorSlot {
	error: string | null;
}

export function createErrorSlot(): PageErrorSlot {
	let error = $state<string | null>(null);
	return {
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
	};
}
