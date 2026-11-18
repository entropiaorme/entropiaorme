/**
 * A measured session minute by minute, in plain words: one row for each
 * step between two readings, with what was looted beside what arrived and
 * the weapon and healing spend measured beside the tracked, and the
 * minutes that need a look said as sentences.
 *
 * What needs a look: something arrived that no loot line explains and the
 * player has not decided yet, loot the chat log recorded that never
 * arrived, and a stretch where the tracked weapon or healing cost drifted
 * away from what the account measured. Arrivals already recorded or
 * ignored are listed with what was decided, and need nothing more.
 *
 * Times are the readings', local, by the app's time format; nothing here
 * sums a cost of its own.
 */

import type {
	AccountSessionMinutes,
	ArrivalNotProposed,
	MinuteArrival,
	MinuteFigure,
	MinuteMissingLoot,
	MinuteNoCorrection,
	MinuteStretch,
	MinuteUnmeasured,
	SessionMinute,
} from '$lib/api/commands.gen';
import { formatPed, formatQuantity, formatTt, NO_DATA } from '$lib/utils/format';
import { formatMeasured, formatTracked } from './measure';
import { placeName, type ReadingClock, readingClock } from './measureDetails';

/** A lane with a tracked counterpart. */
export type MinuteLaneKind = 'weapon' | 'healing';

/** The clock for a session's minutes: time alone within one day. */
export function minutesClock(minutes: AccountSessionMinutes): ReadingClock {
	return readingClock(minutes.steps.flatMap((step) => [step.between.from, step.between.to]));
}

/** "Rifle", or "40 Weapon Cells". */
function itemPhrase(itemName: string, units: number): string {
	return units === 1 ? itemName : `${formatQuantity(units)} ${itemName}`;
}

// ── Sentences ────────────────────────────────────────────────────────────

/** What arrived, as the minute says it: "Foo Rifle, 5.00 TT, arrived at
 * 2:47 PM: no loot line names it". */
export function arrivalSentence(
	arrival: MinuteArrival,
	step: SessionMinute,
	clock: ReadingClock,
): string {
	const where = arrival.arrivedIn.kind === 'carried' ? '' : ` in ${placeName(arrival.arrivedIn)}`;
	const card = arrival.boughtAtTt ? ', and your PED Card fell by its TT' : '';
	return `${itemPhrase(arrival.itemName, arrival.units)}, ${formatTt(arrival.tt.ped)} TT, arrived${where} at ${clock(step.between.to)}: no loot line names it${card}`;
}

/** Loot that never arrived, as the minute says it. */
export function missingSentence(missing: MinuteMissingLoot, clock: ReadingClock): string {
	return `${itemPhrase(missing.itemName, missing.units)}, ${formatTt(missing.tt)} TT, looted at ${clock(missing.lootedIn.to)}: it never arrived`;
}

const NO_CORRECTION: Record<MinuteNoCorrection, string> = {
	harvestLoot: 'Harvested loot has no correction here.',
	otherLinesArrived:
		'Other loot of this name in the session did arrive, and removing it takes out every line of it.',
};

/** Why the loot correction is not offered, when it is not. */
export function missingNote(missing: MinuteMissingLoot): string | null {
	return missing.correction.kind === 'notOffered' ? NO_CORRECTION[missing.correction.reason] : null;
}

const LANE_COST: Record<MinuteLaneKind, string> = {
	weapon: 'weapon cost',
	healing: 'healing cost',
};

const LANE_ADVICE: Record<MinuteLaneKind, string> = {
	weapon: "check the weapon's cost settings",
	healing: "check the healing tools' cost settings",
};

/**
 * A stretch where the tracked cost drifted, as a sentence: "From 2:40 PM to
 * 2:50 PM the tracked weapon cost ran 18% further above what your account
 * measured than over the rest of the session: check the weapon's cost
 * settings". It is sized by the drift that flagged it, the move beyond the
 * session's usual gap, never by the raw gap (which a constant offset in the
 * cost settings fills on every minute alike): as a share of what was
 * measured over the stretch, or in PED when nothing was. Measured above
 * tracked is a positive drift, so the tracked cost ran below.
 */
export function stretchSentence(
	lane: MinuteLaneKind,
	stretch: MinuteStretch,
	steps: readonly SessionMinute[],
	clock: ReadingClock,
): string {
	const first = steps[stretch.firstStep];
	const last = steps[stretch.lastStep];
	const from = clock((first ?? last)?.between.from ?? stretch.between.from);
	const runsOn = stretch.lastStep >= steps.length - 1;
	const until = runsOn ? '' : ` to ${clock(last?.between.to ?? stretch.between.to)}`;
	const direction = stretch.drift < 0 ? 'above' : 'below';
	const measured = stretch.measured.ped;
	const share = measured > 0 ? Math.round((Math.abs(stretch.drift) / measured) * 100) : 0;
	const size = share >= 1 ? `${share}%` : `${formatPed(Math.abs(stretch.drift))} PED`;
	return `From ${from}${until} the tracked ${LANE_COST[lane]} ran ${size} further ${direction} what your account measured than over the rest of the session: ${LANE_ADVICE[lane]}`;
}

/** How many units the loot correction takes out for an item: every line of
 * it the readings never saw arrive, across the session, wherever the
 * correction is offered for it. The correction archives them all at once. */
export function archivedUnits(minutes: AccountSessionMinutes, itemName: string): number {
	const removes = (missing: MinuteMissingLoot) =>
		missing.correction.kind === 'offered' && missing.correction.itemName === itemName;
	let units = 0;
	for (const step of minutes.steps) {
		for (const missing of step.lootedNeverArrived) {
			if (removes(missing)) units += missing.units;
		}
	}
	return units;
}

// ── Figures ──────────────────────────────────────────────────────────────

const UNMEASURED: Record<MinuteUnmeasured, string> = {
	gap: 'The readings either side of this minute are too far apart to measure it alone',
	ammoUnsettled: 'How much of the ammo was fired in this minute could not be settled',
	ammoToppedUp: 'Universal Ammo was topped up in this minute, so what was fired is not known',
	limitedSetUnconfirmed: 'A limited piece lost TT; pricing it needs its set confirmed',
	noReadings: 'No reading covers this minute',
	partialCoverage: 'The readings do not cover this minute whole',
	stillRunning: 'This minute is still being read',
	trackedOnly: 'The readings do not measure this; its cost is the tracked one',
	playInGap: 'The chat log shows play outside the session across this gap',
	chatLogMissesGap: 'The chat log does not cover this gap',
	chatLogUnchecked: 'The chat log could not be checked over this gap',
	gapUnaccounted: 'Something across this gap is not accounted for',
	overheadInGap: 'This may be play after the session',
	gapCheckPending: 'This gap is checked on a later read',
	awaitingGapReading: 'Waiting for the reading after this gap',
	chatLogNotPastGap: 'The chat log has not been written past this gap yet',
};

const PRECISION_TITLE = {
	exact: 'Exact',
	wholePec: 'Read from values the site rounds down to the PEC',
};

/** A figure as a cell shows it, with what to say on hover. */
export interface CellFigure {
	text: string;
	title: string;
	measured: boolean;
}

export function measuredCell(figure: MinuteFigure): CellFigure {
	if (figure.kind === 'measured') {
		return {
			text: formatMeasured(figure.amount),
			title: PRECISION_TITLE[figure.amount.precision],
			measured: true,
		};
	}
	return { text: NO_DATA, title: UNMEASURED[figure.reason], measured: false };
}

/** A tracked figure, to the precision of the measured one beside it. */
export function trackedText(tracked: number, beside: MinuteFigure): string {
	return formatTracked(tracked, beside.kind === 'measured' ? beside.amount.precision : 'wholePec');
}

// ── Rows and flags ───────────────────────────────────────────────────────

/** Something in a minute worth a sentence. */
export type MinuteFlag =
	| {
			kind: 'arrival';
			key: string;
			arrival: MinuteArrival;
			text: string;
			/** Undecided: it waits for the player. */
			needsLook: boolean;
	  }
	| {
			kind: 'missing';
			key: string;
			missing: MinuteMissingLoot;
			text: string;
			note: string | null;
	  }
	| {
			kind: 'stretch';
			key: string;
			lane: MinuteLaneKind;
			stretch: MinuteStretch;
			text: string;
	  };

export interface MinuteRow {
	/** Stable across reads: the step's later reading. */
	key: string;
	index: number;
	step: SessionMinute;
	/** When the step ended, by the reading that closed it. */
	time: string;
	/** "Between 2:40 PM and 2:41 PM". */
	span: string;
	/** Said beside the time for a step across a gap, or one still open. */
	aside: string | null;
	/** Whether the step falls in a flagged stretch of each lane. */
	drifting: Record<MinuteLaneKind, boolean>;
	flags: MinuteFlag[];
	/** Whether the filtered list keeps it: anything worth seeing, whether or
	 * not there is something to do about it. */
	needsLook: boolean;
}

/** The key an arrival keeps whatever is decided of it: the reading it was
 * seen in and the item, as the backend keys its proposal. */
export function arrivalKey(step: SessionMinute, arrival: MinuteArrival): string {
	return `arrival:${step.readingId}:${arrival.itemName}`;
}

export function missingKey(step: SessionMinute, missing: MinuteMissingLoot): string {
	return `missing:${step.readingId}:${missing.itemName}`;
}

/** The stock of unknown origin an open arrival still stands behind, as the
 * read carries it: recorded, but the arrival never marked done. */
export function standingRecordOf(arrival: MinuteArrival): string | null {
	const disposition = arrival.disposition;
	return disposition.state === 'open' ? (disposition.unlinkedRecordId ?? null) : null;
}

/** Whether an arrival waits for the player: one offered and not decided.
 * One not offered has nothing to decide yet. */
export function arrivalWaits(arrival: MinuteArrival): boolean {
	return arrival.disposition.state === 'open';
}

const NOT_PROPOSED: Record<ArrivalNotProposed, string> = {
	beforeRecognition: 'Before the app recorded from your account.',
	outsideSession: "After the session: any purchase is offered with your account's purchases.",
	settling: 'May still be explained by the next reading.',
	awaiting: 'Offered on the next read.',
};

/** Why an arrival is not offered to decide. */
export function notProposedNote(reason: ArrivalNotProposed): string {
	return NOT_PROPOSED[reason];
}

function minutesAcross(step: SessionMinute): number {
	const ms = new Date(step.between.to).getTime() - new Date(step.between.from).getTime();
	return Math.max(1, Math.round(ms / 60000));
}

export function minuteRows(minutes: AccountSessionMinutes, clock: ReadingClock): MinuteRow[] {
	const stretches: Array<{ lane: MinuteLaneKind; stretch: MinuteStretch }> = [
		...minutes.weaponStretches.map((stretch) => ({ lane: 'weapon' as const, stretch })),
		...minutes.healingStretches.map((stretch) => ({ lane: 'healing' as const, stretch })),
	];
	return minutes.steps.map((step, index) => {
		const flags: MinuteFlag[] = [];
		for (const { lane, stretch } of stretches) {
			if (stretch.firstStep === index) {
				flags.push({
					kind: 'stretch',
					key: `${lane}:${step.readingId}`,
					lane,
					stretch,
					text: stretchSentence(lane, stretch, minutes.steps, clock),
				});
			}
		}
		for (const arrival of step.arrivedNotLooted) {
			flags.push({
				kind: 'arrival',
				key: arrivalKey(step, arrival),
				arrival,
				text: arrivalSentence(arrival, step, clock),
				needsLook: arrivalWaits(arrival),
			});
		}
		for (const missing of step.lootedNeverArrived) {
			flags.push({
				kind: 'missing',
				key: missingKey(step, missing),
				missing,
				text: missingSentence(missing, clock),
				note: missingNote(missing),
			});
		}
		const within = (lane: MinuteLaneKind) =>
			stretches.some(
				(entry) =>
					entry.lane === lane &&
					entry.stretch.firstStep <= index &&
					index <= entry.stretch.lastStep,
			);
		return {
			key: String(step.readingId),
			index,
			step,
			time: clock(step.between.to),
			span: `Between ${clock(step.between.from)} and ${clock(step.between.to)}`,
			aside: step.gap
				? `across a ${minutesAcross(step)} min gap`
				: step.settled
					? null
					: 'still settling',
			drifting: { weapon: within('weapon'), healing: within('healing') },
			flags,
			needsLook: flags.some((flag) => flag.kind !== 'arrival' || flag.needsLook),
		};
	});
}

/** Whether the player can act on a flag here: an arrival offered and not
 * decided, or loot the correction can take out. A drifting stretch, or loot
 * with no correction, is worth seeing but has nothing to do. */
export function actionable(flag: MinuteFlag): boolean {
	switch (flag.kind) {
		case 'arrival':
			return flag.needsLook;
		case 'missing':
			return flag.missing.correction.kind === 'offered';
		case 'stretch':
			return false;
	}
}

/** How many things across the session the player can act on. */
export function flagCount(rows: readonly MinuteRow[]): number {
	return rows.reduce((count, row) => count + row.flags.filter(actionable).length, 0);
}

/** The rows the filter keeps: those that need a look, and those the player
 * acted on here, so a decision just made stays in view with its undo. */
export function rowsToShow(
	rows: readonly MinuteRow[],
	onlyFlagged: boolean,
	keep: ReadonlySet<string>,
): MinuteRow[] {
	if (!onlyFlagged) return [...rows];
	return rows.filter((row) => row.needsLook || row.flags.some((flag) => keep.has(flag.key)));
}

/** The toggle's label: the count of what needs a look, when anything does. */
export function toggleLabel(count: number): string {
	if (count === 0) return 'Minute by minute';
	return `Minute by minute, ${count} ${count === 1 ? 'needs' : 'need'} a look`;
}

/** Loot the readings cannot confirm, as a line: `null` when there is none. */
export function unconfirmedLine(minutes: AccountSessionMinutes): string | null {
	if (minutes.lootUnconfirmed <= 0) return null;
	const amount = formatPed(minutes.lootUnconfirmed);
	return minutes.running
		? `${amount} PED of loot came after the last reading; the next one shows whether it arrived.`
		: `${amount} PED of loot came after the last reading, so no reading shows whether it arrived.`;
}
