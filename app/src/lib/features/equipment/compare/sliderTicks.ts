/**
 * Layout for the labelled ticks under a slider's track. Each tick sits at
 * its exact position; its label centres under it where there is room and
 * is pushed sideways where labels would collide, so close values stay
 * individually readable and clickable.
 */

export type SliderTick = { value: number; label: string; title: string };

/** Ticks whose values read the same at the slider's precision share one
 * label, since they would slide to the same place; ordered along the track. */
export function groupTicks(ticks: SliderTick[], decimals: number): SliderTick[] {
	const groups = new Map<string, SliderTick>();
	for (const tick of ticks) {
		const key = tick.value.toFixed(decimals);
		const group = groups.get(key);
		if (group) {
			group.label += ` · ${tick.label}`;
			group.title += `, ${tick.title}`;
		} else {
			groups.set(key, { ...tick });
		}
	}
	return [...groups.values()].sort((a, b) => a.value - b.value);
}

export type LabelBox = { centre: number; width: number };

/**
 * The left edge for each label (in the order given), in the row's own
 * units. Labels centre on their tick, then move apart by at least `gap`
 * and stay inside the row. Where the row is too narrow for all of them,
 * the last ones overlap rather than leave it.
 */
export function spreadLabels(boxes: LabelBox[], rowWidth: number, gap = 4): number[] {
	const order = boxes.map((_, index) => index).sort((a, b) => boxes[a].centre - boxes[b].centre);
	const lefts = new Array<number>(boxes.length);
	let edge = 0;
	for (const index of order) {
		const box = boxes[index];
		lefts[index] = Math.max(box.centre - box.width / 2, edge);
		edge = lefts[index] + box.width + gap;
	}
	let limit = rowWidth;
	for (const index of [...order].reverse()) {
		lefts[index] = Math.min(lefts[index], limit - boxes[index].width);
		limit = lefts[index] - gap;
	}
	for (const index of order) lefts[index] = Math.max(0, lefts[index]);
	return lefts;
}

/** Whether a fixed label spanning [left, right] would sit under any placed
 * tick label; the fixed one gives way. */
export function overlapsAny(
	left: number,
	right: number,
	lefts: number[],
	boxes: LabelBox[],
	gap = 4,
): boolean {
	return lefts.some((at, index) => at < right + gap && at + boxes[index].width > left - gap);
}
