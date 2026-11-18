/**
 * Local cost-per-use preview for the equipment form. The formula mirrors the
 * backend cost engine's per-use pricing (eo-services/src/cost_engine.rs) for
 * instant feedback while the form is edited: markup applies to decay only
 * (percent / 100, limited items only), each damage-enhancer slot adds 10% to
 * weapon decay and ammo, and the decay-absorption devices take their
 * catalogue shares of the weapon's decay
 * (implant first, then the absorber/extender on the remainder) at their own
 * markups. Past the server's attack-rate limit, every line grows by the
 * attack-rate factor for the weapon's catalogue rate under the reload speed
 * in effect (see `attackRate.ts`). Consumables do not move it. The stored
 * entry's authoritative cost comes back from the save.
 */

import { attackRateFactor } from './attackRate';

/** The economy fields the preview reads from a catalogue selection. */
export interface PreviewComponent {
	/** Decay per use, PEC. */
	decay: number;
	/** Ammo burn per use, PEC. */
	ammoBurn: number;
	isLimited: boolean;
	/** Decay-absorption share, percent (implants and absorbers/extenders). */
	absorptionPercent?: number | null;
	/** Catalogue attack rate, attacks a minute (weapons only). */
	usesPerMinute?: number | null;
}

export interface CostPreviewInput {
	weapon: PreviewComponent | null;
	amp: PreviewComponent | null;
	scope: PreviewComponent | null;
	absorber: PreviewComponent | null;
	implant: PreviewComponent | null;
	/** Weapon markup percent (limited items only). */
	markupPercent: number;
	ampMarkupPercent: number;
	scopeMarkupPercent: number;
	absorberMarkupPercent: number;
	implantMarkupPercent: number;
	damageEnhancers: number;
	/** The reload speed in effect, percent, after the game's stacking limit. */
	reloadSpeedPercent: number;
}

const share = (device: PreviewComponent | null) =>
	Math.min(Math.max((device?.absorptionPercent ?? 0) / 100, 0), 1);

const limitedMult = (device: PreviewComponent | null, markupPercent: number) =>
	device?.isLimited ? markupPercent / 100 : 1.0;

/** Estimated cost per use in PEC, or null while no weapon is selected. */
export function previewCostPerUse(input: CostPreviewInput): number | null {
	const { weapon, amp, scope, absorber, implant, markupPercent, damageEnhancers } = input;
	if (!weapon) return null;
	const enhancerMult = 1 + damageEnhancers * 0.1;
	const scaledDecay = weapon.decay * enhancerMult;
	const implantDecay = scaledDecay * share(implant);
	const absorberDecay = (scaledDecay - implantDecay) * share(absorber);
	const weaponDecay = scaledDecay - implantDecay - absorberDecay;
	let cost =
		weaponDecay * limitedMult(weapon, markupPercent) +
		implantDecay * limitedMult(implant, input.implantMarkupPercent) +
		absorberDecay * limitedMult(absorber, input.absorberMarkupPercent) +
		weapon.ammoBurn * enhancerMult;
	if (amp) {
		cost += amp.decay * limitedMult(amp, input.ampMarkupPercent) + amp.ammoBurn;
	}
	if (scope) {
		cost += scope.decay * limitedMult(scope, input.scopeMarkupPercent);
	}
	return cost * attackRateFactor(weapon.usesPerMinute, input.reloadSpeedPercent);
}
