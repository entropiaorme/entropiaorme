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
