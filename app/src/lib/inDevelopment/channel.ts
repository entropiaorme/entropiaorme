/**
 * Whether surfaces registered as in-development render at all.
 *
 * The channel is stamped at build time by whoever built the app, not inferred
 * from the build mode: an installer built from the latest source is a
 * production Vite build, so build mode cannot distinguish it from a published
 * release. Only a build published to people who did not build it would set
 * `STABLE_CHANNEL`, and nothing in this repository sets it now that no
 * artefacts are published, so:
 *
 *   - a build stamped as published hides these surfaces, which is what keeps
 *     an unfinished control away from someone who did not build the app;
 *   - a locally built installer, a source build, and the dev server all show
 *     them marked, so the app can be used against what is actually
 *     implemented even where a control is still inert.
 */

/** Folds to a constant at build time. See `app/vite.config.ts`. */
const STABLE_CHANNEL = import.meta.env.STABLE_CHANNEL === '1';

/** The channel decision as a pure function of its input, so both outcomes are
 * testable without a second build. */
export function isInDevelopmentVisible(stableChannel: boolean): boolean {
	return !stableChannel;
}

export const inDevelopment = {
	/** Gate the surface on this. The marker assumes it is already inside a
	 * gated block and does not re-check. */
	get visible(): boolean {
		return isInDevelopmentVisible(STABLE_CHANNEL);
	},
};
