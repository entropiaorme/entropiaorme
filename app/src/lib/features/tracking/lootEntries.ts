/**
 * The question the session's loot correction asks before it takes every
 * line of an item out of the session's loot, or puts them back: the same
 * words wherever it is asked from.
 */
export function lootEntriesPrompt(
	action: 'Archive' | 'Restore',
	quantity: number,
	name: string,
): string {
	return `${action} all ${quantity} "${name}" ${quantity === 1 ? 'entry' : 'entries'}?`;
}
