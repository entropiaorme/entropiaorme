/**
 * The register of surfaces that ship ahead of their capability.
 *
 * One register rather than a flag per component, so the set stays auditable
 * and cannot quietly become permanent. Graduating a surface means deleting
 * its entry: the `in-development` guard fails a marker with no entry and an
 * entry with no consumer, so neither half can outlive the other unnoticed.
 *
 * Only genuinely misleading surfaces belong here: a control that does
 * nothing when used, or a figure whose value can diverge from the truth
 * with nothing signalling it. A figure that happens to sit beside unbuilt
 * work but is already correct does not need an entry.
 *
 * Copy is user-facing: what is unavailable, and what will make it work.
 */

export interface InDevelopmentSurface {
	/** Stable key shared by the marker, its consumers, and the guard. */
	readonly id: string;
	readonly summary: string;
	readonly graduates: string;
}

export const IN_DEVELOPMENT_SURFACES: readonly InDevelopmentSurface[] = [
	{
		id: 'limited-protection',
		summary:
			'Armour costs can be recorded, reviewed, undone and re-recorded, but recording has not yet been walked in live play.',
		graduates: 'Recording has been walked in live play.',
	},
	{
		id: 'creature-detection',
		summary:
			'Reads the name over the creature you are locked on to and records which creature each kill was. Tried so far only on simulated ordinary hunts, where it named about three kills in four (one in two hundred wrongly) and kept your declared mob for the rest. Off until you turn it on.',
		graduates: 'Detection has been checked against live hunts.',
	},
	{
		id: 'held-tool-reading',
		summary:
			"Reads the tool in hand off the game's own bar and costs each shot from that tool's library entry, leaving the hotbar keys to start consumables. Tried so far only on recorded screens, not yet in a live hunt. Off until you turn it on.",
		graduates: 'Reading the held tool has been walked in live hunts.',
	},
] as const;

/** The registered surface for `id`. Throws on an unregistered id, matching
 * the guard's build-time check so both enforcement points agree. */
export function inDevelopmentSurface(id: string): InDevelopmentSurface {
	const surface = IN_DEVELOPMENT_SURFACES.find((entry) => entry.id === id);
	if (!surface) {
		throw new Error(`in-development surface "${id}" is not registered`);
	}
	return surface;
}
