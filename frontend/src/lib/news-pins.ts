import type { NewsEntry, SlotId } from './news';

/*
 * Slot defaults for the three-slot pinned-cards strip at the top of /news.
 * Each slot is a named role with its own visual register: the community
 * slot points users at the conversation surface, the release slot anchors
 * the latest version announcement (terse-technical changelog, not a
 * narrative article), the foundations slot holds a piece of
 * project-foundational context. Defaults render when an article occupying
 * the slot has not supplied an override via pin_blurb / pin_icon / pin_cta.
 *
 * Render order in the strip follows `order`; rendering itself skips
 * vacant slots so the grid collapses to N columns where N = count of
 * populated slots (0/1/2/3 all render correctly without empty cells).
 */
export type SlotDefaults = {
	label: string;
	cta: string;
	order: number;
};

export const SLOT_DEFAULTS: Record<SlotId, SlotDefaults> = {
	community: { label: 'Community', cta: 'Read article', order: 0 },
	release: { label: 'Release', cta: 'Read announcement', order: 1 },
	foundations: { label: 'Foundations', cta: 'Read article', order: 2 },
};

export const SLOT_ORDER: SlotId[] = ['community', 'release', 'foundations'];

/*
 * Resolve which article occupies each slot given the current feed cache.
 *
 * Per slot:
 *   1. If any article has explicit `pin_slot: <slot>`, the latest-by-date
 *      among them wins.
 *   2. For the release slot only, fall back to the latest article with
 *      `category: 'changelog'` when no explicit assignment exists. Lets
 *      every release announcement pin without authoring a frontmatter
 *      flag; an explicit `pin_slot: release` on any article (changelog or
 *      otherwise) still overrides the auto-derive when present.
 *   3. Otherwise the slot stays vacant.
 *
 * Returns a map from slot id to either the occupying entry or null.
 */
export function resolvePinSlots(entries: NewsEntry[]): Record<SlotId, NewsEntry | null> {
	const slots: Record<SlotId, NewsEntry | null> = {
		community: null,
		release: null,
		foundations: null,
	};

	for (const slot of SLOT_ORDER) {
		const explicit = entries
			.filter((e) => e.pin_slot === slot)
			.slice()
			.sort((a, b) => b.date.localeCompare(a.date));

		if (explicit.length > 0) {
			slots[slot] = explicit[0];
			continue;
		}

		if (slot === 'release') {
			const latestChangelog = entries
				.filter((e) => e.category === 'changelog')
				.slice()
				.sort((a, b) => b.date.localeCompare(a.date))[0];
			if (latestChangelog) slots.release = latestChangelog;
		}
	}

	return slots;
}

/*
 * Slugs of all currently-pinned entries. The /news chronological list
 * filters these out so pinned articles do not duplicate below the strip.
 */
export function pinnedSlugSet(slots: Record<SlotId, NewsEntry | null>): Set<string> {
	const slugs = new Set<string>();
	for (const slot of SLOT_ORDER) {
		const entry = slots[slot];
		if (entry) slugs.add(entry.slug);
	}
	return slugs;
}
