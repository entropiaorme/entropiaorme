/**
 * How much of a holding an action takes, entered either as a count of units
 * or as PED of TT.
 *
 * Both are the same amount read two ways: a stack of ore is thought of in
 * PED, a stack of Shrapnel by the PED it comes to rather than by its tens of
 * thousands of units, an asset by how many there are. The units are what is
 * recorded, so PED is divided back through the unit TT here, once, and the
 * entry rounds to whole units except where it names the whole holding.
 *
 * An item with no TT (a strongbox, a token) has no PED to enter, so it is
 * entered by count alone.
 */
import { formatQuantity, formatTt, isZeroTt } from '$lib/utils/format';

export type AmountMode = 'units' | 'ped';

/** Whether an amount of this item can be entered as PED of TT. */
export function canEnterByPed(unitTt: number): boolean {
	return Number.isFinite(unitTt) && unitTt > 0 && !isZeroTt(unitTt);
}

/** The TT a count of units comes to, to the ten-thousandth of a PED. */
export function pedForUnits(units: number, unitTt: number): number {
	return Math.round(units * unitTt * 10000) / 10000;
}

/**
 * The units an amount of PED comes to. A figure within half a unit of the
 * whole holding is the whole holding, so "all of it" in PED stays exactly all
 * of it even when the held count is fractional or the unit TT an average.
 */
export function unitsForPed(ped: number, unitTt: number, held?: number | null): number {
	if (!canEnterByPed(unitTt) || !Number.isFinite(ped) || ped <= 0) return 0;
	const raw = ped / unitTt;
	if (held !== undefined && held !== null && held > 0 && Math.abs(raw - held) < 0.5) {
		return held;
	}
	return Math.round(raw);
}

/** The amount in the other form, as the line beneath the field reads it. */
export function amountEquivalent(
	units: number | null,
	unitTt: number,
	mode: AmountMode,
): string | null {
	if (units === null || !(units > 0) || !canEnterByPed(unitTt)) return null;
	return mode === 'ped'
		? `${formatQuantity(units)} ${units === 1 ? 'unit' : 'units'}`
		: `${formatTt(pedForUnits(units, unitTt))} PED`;
}

/** The whole holding in the form the field is in: "12.34 PED" or "5000". */
export function heldInMode(held: number, unitTt: number, mode: AmountMode): string {
	return mode === 'ped' && canEnterByPed(unitTt)
		? `${formatTt(pedForUnits(held, unitTt))} PED`
		: formatQuantity(held);
}
