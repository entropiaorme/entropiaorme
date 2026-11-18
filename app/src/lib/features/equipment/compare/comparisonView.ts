/**
 * Pure presentation logic for the comparison: labels, the unpriceable
 * text, and the chart's series and axes. Everything reads the backend's
 * computed comparison; no figure is derived here.
 */

import type { ComparedSetup, LoadoutComparison } from '$lib/api/comparison';
import { NO_DATA } from '$lib/utils/format';

/** Each setup's label, in the comparison's order: its weapon and any
 * amplifier, numbered when two would read the same. */
export function setupLabels(comparison: LoadoutComparison): string[] {
	const base = comparison.setups.map((setup: ComparedSetup) => {
		const amp = setup.components.find((component) => component.role === 'amplifier');
		return amp ? `${setup.name} + ${amp.name}` : setup.name;
	});
	const seen = new Map<string, number>();
	return base.map((label) => {
		const count = (seen.get(label) ?? 0) + 1;
		seen.set(label, count);
		const repeated = base.filter((other) => other === label).length > 1;
		return repeated ? `${label} (${count})` : label;
	});
}

/** A per-kill PEC figure: two decimals, three below one PEC. */
export function formatKillPec(value: number): string {
	if (!Number.isFinite(value)) return NO_DATA;
	if (Math.abs(value) < 1) return value.toFixed(3);
	if (Math.abs(value) >= 1000) return Math.round(value).toLocaleString('en-GB');
	return value.toFixed(2);
}

export function formatSeconds(value: number | null): string {
	if (value === null || !Number.isFinite(value)) return NO_DATA;
	return value < 10 ? `${value.toFixed(2)} s` : `${value.toFixed(1)} s`;
}

export function formatHp(hp: number): string {
	return Math.round(hp).toLocaleString('en-GB');
}

/** Why a setup has no figures, as a sentence. */
export function describeUnmodelled(setup: ComparedSetup): string | null {
	switch (setup.unmodelled) {
		case 'no_damage':
			return 'The catalogue publishes no damage for this weapon.';
		case 'no_cost':
			return 'A use costs nothing, so there is nothing to compare.';
		case 'missing_efficiency':
			return 'A weapon or amplifier has no catalogue Efficiency, so its return cannot be modelled.';
		default:
			return null;
	}
}

export interface ChartSeries {
	/** The setup's position in the comparison. */
	index: number;
	/** Its chart colour. */
	slot: number;
	label: string;
	points: { hp: number; loss: number; cycled: number; seconds: number | null }[];
}

export interface ChartMarker {
	hp: number;
	loss: number;
}

export interface ChartModel {
	series: ChartSeries[];
	markers: ChartMarker[];
	hpMax: number;
	lossMax: number;
}

/** A series' loss at an HP between its sampled points. */
function lossAt(series: ChartSeries, hp: number): number | null {
	const points = series.points;
	if (points.length === 0) return null;
	if (hp <= points[0].hp) return points[0].loss;
	for (let index = 1; index < points.length; index += 1) {
		const right = points[index];
		if (hp <= right.hp) {
			const left = points[index - 1];
			const t = right.hp === left.hp ? 0 : (hp - left.hp) / (right.hp - left.hp);
			return left.loss + t * (right.loss - left.loss);
		}
	}
	return points[points.length - 1].loss;
}

/** The loss-per-kill chart over 0 to the curve's top HP: a line per priced
 * setup, in its colour `slots[index]` (null for a setup no longer listed,
 * which is left off), and a marker wherever the cheapest setup changes.
 * Null when nothing is priced. */
export function buildChart(
	comparison: LoadoutComparison,
	slots: (number | null)[],
): ChartModel | null {
	const labels = setupLabels(comparison);
	const series: ChartSeries[] = comparison.setups.flatMap((setup, index) =>
		setup.curve.length === 0 || slots[index] === null
			? []
			: [
					{
						index,
						slot: slots[index] ?? index + 1,
						label: labels[index],
						points: setup.curve.map((point) => ({
							hp: point.hp,
							loss: point.lossPec,
							cycled: point.cycledPec,
							seconds: point.seconds,
						})),
					},
				],
	);
	if (series.length === 0) return null;
	const markers: ChartMarker[] = [];
	for (let position = 1; position < comparison.winners.length; position += 1) {
		const segment = comparison.winners[position];
		const line = series.find((entry) => entry.index === segment.index);
		const loss = line ? lossAt(line, segment.fromHp) : null;
		if (loss !== null) markers.push({ hp: segment.fromHp, loss });
	}
	return {
		series,
		markers,
		hpMax: comparison.hpMax,
		lossMax: Math.max(0, ...series.flatMap((entry) => entry.points.map((point) => point.loss))),
	};
}

/** Round tick values for an axis from zero to about `max`. */
export function niceTicks(max: number, target = 4): number[] {
	if (!(max > 0) || !Number.isFinite(max)) return [0];
	const raw = max / target;
	const magnitude = 10 ** Math.floor(Math.log10(raw));
	const step =
		[1, 2, 2.5, 5, 10].map((unit) => unit * magnitude).find((candidate) => candidate >= raw) ?? raw;
	const ticks: number[] = [];
	for (let value = 0; value <= max + step * 1e-9; value += step) {
		ticks.push(Number(value.toPrecision(12)));
	}
	if (ticks[ticks.length - 1] < max) {
		ticks.push(Number((ticks[ticks.length - 1] + step).toPrecision(12)));
	}
	return ticks;
}

/** Round tick values covering `[min, max]`, for a zoomed axis. */
export function niceRangeTicks(min: number, max: number, target = 4): number[] {
	if (!Number.isFinite(min) || !Number.isFinite(max) || !(max > min)) return [min];
	const raw = (max - min) / target;
	const magnitude = 10 ** Math.floor(Math.log10(raw));
	const step =
		[1, 2, 2.5, 5, 10].map((unit) => unit * magnitude).find((candidate) => candidate >= raw) ?? raw;
	const ticks: number[] = [];
	for (let value = Math.ceil(min / step - 1e-9) * step; value <= max + step * 1e-9; value += step) {
		ticks.push(Number(value.toPrecision(12)));
	}
	return ticks;
}

/** One setup's answers for a number of kills of one mob HP. */
export interface PlanFigures {
	/** TT cycled; markup paid on top is part of the leak. */
	cycledPed: number;
	/** PED lost beyond a perfect setup (unlimited, 100% Efficiency), and
	 * its parts. */
	leakPed: number;
	markupPed: number;
	efficiencyPed: number;
	unmodelledPed: number;
	/** What even a perfect setup would lose on the same TT. */
	houseEdgePed: number;
	/** Expected loss: the house edge plus the leak. */
	lossPed: number;
	/** Firing time, when the attack rate is known. */
	seconds: number | null;
	/** Uses fired. */
	uses: number;
	/** The share of damage dealt that lands past the kill at this HP. */
	overkillShare: number;
}

/** A setup's figures for killing `kills` mobs of `hp`. */
export function planFigures(setup: ComparedSetup, kills: number, hp: number): PlanFigures | null {
	const economics = setup.economics;
	if (!economics) return null;
	const ttPerUse = economics.costPerUsePec - economics.consumedPremiumPec;
	const usesPerKill = usesAt(setup, hp);
	if (!(ttPerUse > 0) || usesPerKill === null || !(usesPerKill > 0)) return null;
	const uses = kills * usesPerKill;
	const ped = (perUse: number) => (uses * perUse) / 100;
	const markupPed = ped(economics.consumedPremiumPec);
	const efficiencyPed = ped(economics.efficiencyShortfallPec);
	const unmodelledPed = ped(economics.outsideModelPec);
	const leakPed = markupPed + efficiencyPed + unmodelledPed;
	const houseEdgePed = ped(economics.houseEdgePec);
	return {
		cycledPed: ped(ttPerUse),
		leakPed,
		markupPed,
		efficiencyPed,
		unmodelledPed,
		houseEdgePed,
		lossPed: houseEdgePed + leakPed,
		seconds: economics.usesPerMinute === null ? null : (uses * 60) / economics.usesPerMinute,
		uses,
		overkillShare: Math.max(
			0,
			1 - hp / (usesPerKill * economics.damagePerPec * economics.costPerUsePec),
		),
	};
}

/** How many of each limited part `uses` wears out: its decay over its full TT. */
export function partsUsedUp(
	setup: ComparedSetup,
	uses: number,
): { role: ComparedSetup['components'][number]['role']; count: number }[] {
	return setup.components.flatMap((component) => {
		if (!component.isLimited || component.maxTtPed === null || component.decayPecPerUse <= 0)
			return [];
		return [
			{ role: component.role, count: (uses * component.decayPecPerUse) / 100 / component.maxTtPed },
		];
	});
}

/** Expected uses per kill at `hp`, between the curve's sampled points. */
function usesAt(setup: ComparedSetup, hp: number): number | null {
	const curve = setup.curve;
	if (curve.length === 0 || hp < curve[0].hp || hp > curve[curve.length - 1].hp) return null;
	const right = curve.findIndex((point) => point.hp >= hp);
	if (right <= 0) return curve[0].uses;
	const [a, b] = [curve[right - 1], curve[right]];
	return a.uses + ((hp - a.hp) / (b.hp - a.hp)) * (b.uses - a.uses);
}

/** A PED figure for the budget columns. */
export function formatPed(value: number): string {
	if (!Number.isFinite(value)) return NO_DATA;
	if (Math.abs(value) >= 100) return Math.round(value).toLocaleString('en-GB');
	return value.toFixed(value >= 10 ? 1 : 2);
}

/** A whole count, grouped. */
export function formatCount(value: number | null): string {
	if (value === null || !Number.isFinite(value)) return NO_DATA;
	return Math.floor(value).toLocaleString('en-GB');
}

/** A long duration as hours and minutes. */
export function formatDuration(seconds: number | null): string {
	if (seconds === null || !Number.isFinite(seconds)) return NO_DATA;
	const minutes = Math.round(seconds / 60);
	if (minutes < 60) return `${minutes} min`;
	return `${Math.floor(minutes / 60)} h ${String(minutes % 60).padStart(2, '0')} min`;
}

/** A typed amount: "250k", "1.5m", "150,000", or a plain number. */
export function parseAmount(text: string): number | null {
	const match = text
		.trim()
		.toLowerCase()
		.replace(/,/g, '')
		.match(/^\+?(\d+(?:\.\d+)?)\s*([km]?)$/);
	if (!match) return null;
	const scale = match[2] === 'k' ? 1e3 : match[2] === 'm' ? 1e6 : 1;
	return Number(match[1]) * scale;
}

/** Whether a setup's weapon is a limited item. */
export function isLimitedSetup(setup: ComparedSetup): boolean {
	return setup.components.some((component) => component.role === 'weapon' && component.isLimited);
}

/** A setup's expected loss per kill of `hp`, PED. */
export function lossPerKill(setup: ComparedSetup, hp: number): number | null {
	return planFigures(setup, 1, hp)?.lossPed ?? null;
}

/** How many kills a year make held capital worth it against a reference. */
export type Payback =
	| {
			kind: 'pays_back';
			/** Kills a year that save the yearly charge on the capital. */
			perYear: number;
			capitalCostPed: number;
			/** PED saved per kill. */
			saving: number;
	  }
	| { kind: 'never'; capitalCostPed: number; saving: number };

export function payback(
	setup: ComparedSetup,
	reference: ComparedSetup,
	hp: number,
	capitalPed: number,
	rate: number,
): Payback | null {
	const own = lossPerKill(setup, hp);
	const theirs = lossPerKill(reference, hp);
	if (own === null || theirs === null) return null;
	const capitalCostPed = capitalPed * rate;
	const saving = theirs - own;
	if (!(saving > 0)) return { kind: 'never', capitalCostPed, saving };
	return { kind: 'pays_back', perYear: capitalCostPed / saving, capitalCostPed, saving };
}

const compactNumber = new Intl.NumberFormat('en-GB', {
	notation: 'compact',
	maximumFractionDigits: 1,
});

/** A large figure in short form: 375K, 1.2M. */
export function formatCompact(value: number): string {
	if (!Number.isFinite(value)) return NO_DATA;
	return compactNumber.format(value);
}
