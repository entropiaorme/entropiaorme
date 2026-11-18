import type { RecentEvent } from '$lib/api';

/** Identity of a feed's newest event, or `null` for an empty feed.
 *
 * The feed arrives newest first and its `id`s are positional (`ne-0` is
 * always the head), so they cannot tell a new event from an old one; the
 * event's own content, timestamp included, can. */
export function feedHead(events: readonly RecentEvent[]): string | null {
	const head = events[0];
	if (head === undefined) return null;
	return [head.timestamp ?? '', head.eventType, head.description, head.value].join('|');
}

/** The feed head the reader has already seen, after a new read.
 *
 * `seen` and `head` are `undefined` until the feed has first been read. The
 * first read is never news, so it becomes the seen head; afterwards the seen
 * head only moves while the reader is watching the feed. */
export function acknowledgeHead(
	seen: string | null | undefined,
	head: string | null | undefined,
	watching: boolean,
): string | null | undefined {
	if (head === undefined) return seen;
	return seen === undefined || watching ? head : seen;
}

/** Whether the feed holds an event the reader has not seen: a non-empty
 * feed whose head moved since the reader last watched it. An emptied feed
 * (a new session starting) is not news. */
export function hasUnseenEvents(
	seen: string | null | undefined,
	head: string | null | undefined,
	watching: boolean,
): boolean {
	return !watching && seen !== undefined && head != null && head !== seen;
}

/** The dot colour for an event's category. */
export function eventToneClass(category: RecentEvent['type']): string {
	switch (category) {
		case 'hof':
			return 'bg-warning [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-warning)_60%,transparent)]';
		case 'quest':
			return 'bg-positive [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-positive)_60%,transparent)]';
		case 'warning':
			return 'bg-negative [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-negative)_60%,transparent)]';
		case 'global':
			return 'bg-accent [box-shadow:0_0_8px_color-mix(in_oklab,var(--color-accent)_60%,transparent)]';
	}
}

/** The time of day an event happened ("2:05 PM"), or `null` when it carries
 * no timestamp. */
export function formatEventTime(timestamp: string | null): string | null {
	if (timestamp === null) return null;
	const at = new Date(timestamp);
	if (Number.isNaN(at.getTime())) return null;
	return at.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
}
