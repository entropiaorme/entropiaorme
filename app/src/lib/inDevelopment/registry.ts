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
