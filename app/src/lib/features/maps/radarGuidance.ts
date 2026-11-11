/**
 * Where the radar guidance overlay draws. The overlay is a click-through
 * window laid exactly over the game's radar disc, and draws in a 0..100
 * box: the centre is the player, the box's edge the disc's edge.
 *
 * A tree within the radar's reach is drawn where it is on the radar, its
 * distance scaled by the edge range the game prints on the radar's rings.
 * Without that range (the ring distances are hidden, or did not read) the
 * overlay shows direction only and never a distance-scaled mark.
 *
 * Two things on the radar are read again while guidance runs, so the
 * overlay never draws over them: the player's dot at the centre, and the
 * strip where the ring distances are printed.
 */

/** Told to the guidance windows whenever radar guidance's status moves. */
export const RADAR_GUIDANCE_CHANGED_EVENT = 'radar-guidance-changed';

/** The disc's edge, in the overlay's 0..100 box. */
export const RADAR_EDGE = 50;

/**
 * How far out the overlay draws: a hair inside the edge, so a tree's dot
 * there stays wholly in the window. A tree further out than a dot can be
 * drawn whole is shown by direction only.
 */
export const DRAW_LIMIT = 47;

/**
 * Where the bearing line starts: clear of the player's dot at the centre.
 * The game draws that dot, and guidance finds the radar again by it, so
 * the overlay must never cover it.
 */
export const LINE_START = 6;

/** The tree's dot, as the overlay draws it. */
export const DOT_RADIUS = 3.5;

/**
 * Room kept between anything drawn and the ring-distance strip: half the
 * line's width and its glow.
 */
export const STRIP_CLEARANCE = 3;

/**
 * A position older than this many poll intervals is not steered from: the
 * player may have moved well away from it.
 */
export const STEERING_AGE_INTERVALS = 3;

export type Point = { x: number; y: number };
export type Segment = { from: Point; to: Point };

/** The ring-distance strip, in shares of the radar's radius. */
export type LabelStrip = { from: number; to: number; halfHeight: number };

export type GuidanceMarks = {
	/** The bearing line's pieces, outside the ring-distance strip. */
	segments: Segment[];
	/** The tree where it is on the radar; null when it cannot be drawn. */
	dot: Point | null;
};

type Box = { x0: number; y0: number; x1: number; y1: number };

function along(radians: number, radius: number): Point {
	return {
		x: RADAR_EDGE + Math.sin(radians) * radius,
		y: RADAR_EDGE - Math.cos(radians) * radius,
	};
}

/** The strip, in the overlay's box, widened by the clearance. */
export function stripBox(strip: LabelStrip): Box {
	return {
		x0: RADAR_EDGE + strip.from * RADAR_EDGE - STRIP_CLEARANCE,
		x1: RADAR_EDGE + strip.to * RADAR_EDGE + STRIP_CLEARANCE,
		y0: RADAR_EDGE - strip.halfHeight * RADAR_EDGE - STRIP_CLEARANCE,
		y1: RADAR_EDGE + strip.halfHeight * RADAR_EDGE + STRIP_CLEARANCE,
	};
}

/** The parts of `segment` outside `box` (Liang-Barsky clipping). */
function outside(segment: Segment, box: Box | null): Segment[] {
	if (!box) return [segment];
	const { from, to } = segment;
	const dx = to.x - from.x;
	const dy = to.y - from.y;
	let enter = 0;
	let leave = 1;
	for (const [p, q] of [
		[-dx, from.x - box.x0],
		[dx, box.x1 - from.x],
		[-dy, from.y - box.y0],
		[dy, box.y1 - from.y],
	]) {
		if (p === 0) {
			if (q < 0) return [segment];
			continue;
		}
		const t = q / p;
		if (p < 0) enter = Math.max(enter, t);
		else leave = Math.min(leave, t);
	}
	if (enter >= leave) return [segment];
	const at = (t: number): Point => ({ x: from.x + dx * t, y: from.y + dy * t });
	const pieces: Segment[] = [];
	if (enter > 0) pieces.push({ from, to: at(enter) });
	if (leave < 1) pieces.push({ from: at(leave), to });
	return pieces;
}

function touches(dot: Point, box: Box | null): boolean {
	if (!box) return false;
	const nearX = Math.min(Math.max(dot.x, box.x0), box.x1);
	const nearY = Math.min(Math.max(dot.y, box.y0), box.y1);
	return Math.hypot(dot.x - nearX, dot.y - nearY) < DOT_RADIUS;
}

/**
 * The overlay's marks for a tree `distanceM` metres away at
 * `bearingDegrees` clockwise from north, on a radar whose edge is
 * `edgeRangeM` metres from the player (null when unknown), with nothing
 * drawn inside the ring-distance `strip`.
 */
export function guidanceMarks(
	bearingDegrees: number,
	distanceM: number | null,
	edgeRangeM: number | null,
	strip: LabelStrip | null,
): GuidanceMarks {
	const radians = (bearingDegrees * Math.PI) / 180;
	const box = strip ? stripBox(strip) : null;
	const from = along(radians, LINE_START);
	const onRadar =
		edgeRangeM != null && edgeRangeM > 0 && distanceM != null && distanceM >= 0
			? (distanceM / edgeRangeM) * RADAR_EDGE
			: null;
	let to: Point;
	let dot: Point | null = null;
	if (onRadar == null || onRadar > DRAW_LIMIT) {
		to = along(radians, DRAW_LIMIT);
	} else if (onRadar < LINE_START) {
		// A tree closer than the line's start would be drawn over the
		// player's dot: the direction alone is shown, from the line's start.
		to = from;
	} else {
		dot = along(radians, onRadar);
		to = dot;
	}
	return {
		segments: outside({ from, to }, box),
		dot: dot && !touches(dot, box) ? dot : null,
	};
}

/**
 * Whether a position is recent enough to steer from: its age when it was
 * received, plus the time since, within a few poll intervals. A position
 * never read is not current.
 */
export function positionIsCurrent(
	ageAtReceiptS: number | null,
	receivedAtMs: number,
	nowMs: number,
	pollIntervalS: number,
): boolean {
	if (ageAtReceiptS == null) return false;
	const age = ageAtReceiptS + Math.max(0, nowMs - receivedAtMs) / 1000;
	return age <= STEERING_AGE_INTERVALS * Math.max(1, pollIntervalS);
}
