/**
 * A session measure's details in plain words: wear item by item, repairs
 * (which settle wear and are never a cost), other consumption, the ammo by
 * type, what left with nothing to explain it, and the notable events in
 * time order.
 *
 * Times are those of the reading that saw the change, local, shown by the
 * app's time format, with the date as well when they span several days.
 */

import type {
	AccountPlace,
	AccountSessionMeasure,
	MeasuredAmmo,
	MeasuredAmount,
	MeasuredConsumption,
	MeasuredDecay,
	MeasuredEvent,
	MeasuredEventItem,
	MeasuredRepair,
	MeasuredRole,
	MeasuredSpan,
} from '$lib/api/commands.gen';
import { formatDate, formatTime, listNames } from '$lib/utils/format';
import { formatMeasured, formatUnits, sumAmounts } from './measure';

/** How a reading's time is shown: the time of day, with the date too
 * when the times shown span more than one calendar day. */
export type ReadingClock = (iso: string) => string;

function calendarDay(iso: string): string {
	const at = new Date(iso);
	return `${at.getFullYear()}-${at.getMonth()}-${at.getDate()}`;
}

/** The clock for a set of reading times: time alone within one day, date
 * and time across several. */
export function readingClock(isos: readonly string[]): ReadingClock {
	const days = new Set(isos.map(calendarDay));
	if (days.size <= 1) return formatTime;
	return (iso) => `${formatDate(iso)}, ${formatTime(iso)}`;
}

/** The clock for everything a measure's details list with a time. */
export function measureClock(measure: AccountSessionMeasure): ReadingClock {
	return readingClock([
		...measure.events.map((event) => event.between.to),
		...measure.repairs.map((repair) => repair.between.to),
	]);
}

/** When a change was seen: by the later of the two readings around it. */
export function seenAt(span: MeasuredSpan, clock: ReadingClock): string {
	return clock(span.to);
}

/** Where a holding sits, in words. */
export function placeName(place: AccountPlace): string {
	switch (place.kind) {
		case 'carried':
			return 'your inventory';
		case 'auction':
			return 'the auction';
		case 'storage':
			return `storage on ${place.planet}`;
		case 'attached':
			return `attached to ${place.parent}`;
		case 'other':
			return place.name;
	}
}

/** A quantity of something: the name alone for one, "580 Name" for more. */
export function quantityOf(name: string, units: number): string {
	return Math.abs(units) === 1 ? name : `${formatUnits(units)} ${name}`;
}

function itemList(items: readonly MeasuredEventItem[]): string {
	return listNames(items.map((line) => quantityOf(line.item.name, line.units)));
}

function ped(amount: MeasuredAmount): string {
	return `${formatMeasured(amount)} PED`;
}

/** One notable event as a plain sentence. */
export function eventSentence(event: MeasuredEvent): string {
	switch (event.kind) {
		case 'strongboxOpening': {
			const boxes = listNames(event.boxes.map((box) => `${formatUnits(box.count)} ${box.name}`));
			const parts: string[] = [];
			parts.push(
				event.settlement === 'settled'
					? `${ped(event.ammoFromRows)} ammo`
					: 'ammo that could not be settled',
			);
			for (const line of event.contents) parts.push(quantityOf(line.item.name, line.units));
			const keys = `${formatUnits(event.keysUsed)} ${Math.abs(event.keysUsed) === 1 ? 'key' : 'keys'}`;
			let sentence = `Opened ${boxes} with ${keys}: ${parts.join(', ')}`;
			if (event.possibleContents.length > 0) {
				sentence += `; possibly ${itemList(event.possibleContents)}`;
			}
			return sentence;
		}
		case 'sale': {
			const what = event.items.length > 0 ? `; ${itemList(event.items)}` : '';
			return `Sold to the Trade Terminal: ${ped(event.amount)}${what}`;
		}
		case 'purchase': {
			const what = event.items.length > 0 ? `; ${itemList(event.items)}` : '';
			return `Bought from the Trade Terminal: ${ped(event.amount)}${what}`;
		}
		case 'moved': {
			const what = `${quantityOf(event.name, event.units)}, ${ped(event.amount)}`;
			if (event.moveKind === 'listing') return `Listed on the auction: ${what}`;
			if (event.moveKind === 'listingReturned') return `Back from the auction: ${what}`;
			if (event.from.kind === 'carried') return `Moved to ${placeName(event.to)}: ${what}`;
			return `Moved from ${placeName(event.from)} to ${placeName(event.to)}: ${what}`;
		}
		case 'appeared':
			return `Arrived in ${placeName(event.item.place)}: ${quantityOf(event.item.name, event.units)}, ${ped(event.value)}`;
		case 'left':
			return `Left ${placeName(event.item.place)}: ${quantityOf(event.item.name, event.units)}, ${ped(event.value)}`;
		case 'shrapnelConverted':
			return `Converted ${formatUnits(event.shrapnelUnits)} Shrapnel (${ped(event.shrapnel)}) into ${formatUnits(event.ammoUnits)} ammo (${ped(event.ammo)})`;
		case 'listingSold':
			return `Sold on the auction: ${quantityOf(event.item.name, event.units)}, ${ped(event.amount)}`;
		case 'shrapnelPresumedConverted':
			return `${formatUnits(event.shrapnelUnits)} Shrapnel (${ped(event.shrapnel)}) left and is taken as converted into ammo`;
		case 'ammoToppedUp':
			return `${event.name} topped up: what was fired in that span is the tracked figure`;
	}
}

/** The notable events in time order, each with its reading's time. */
export function eventLines(
	events: readonly MeasuredEvent[],
	clock: ReadingClock,
): { at: string; text: string }[] {
	return events
		.map((event, index) => ({ event, index }))
		.sort(
			(a, b) =>
				Date.parse(a.event.between.to) - Date.parse(b.event.between.to) || a.index - b.index,
		)
		.map(({ event }) => ({ at: seenAt(event.between, clock), text: eventSentence(event) }));
}

// ── Wear ────────────────────────────────────────────────────────────────

const ROLE_ORDER: readonly MeasuredRole[] = [
	'weapon',
	'amplifier',
	'healing',
	'armour',
	'plating',
	'otherEquipment',
	'ammo',
	'material',
];

const ROLE_LABELS: Record<MeasuredRole, string> = {
	weapon: 'Weapons',
	amplifier: 'Amplifiers',
	healing: 'Healing tools',
	armour: 'Armour',
	plating: 'Plating',
	otherEquipment: 'Other equipment',
	ammo: 'Ammo',
	material: 'Materials',
};

export interface WearGroup {
	role: MeasuredRole;
	label: string;
	total: MeasuredAmount;
	items: { name: string; amount: MeasuredAmount }[];
}

/** Wear per item, grouped by role in a fixed order, heaviest first within
 * each group. */
export function wearGroups(decay: readonly MeasuredDecay[]): WearGroup[] {
	const groups: WearGroup[] = [];
	for (const role of ROLE_ORDER) {
		const items = decay
			.filter((item) => item.role === role)
			.map((item) => ({ name: item.item.name, amount: item.decay }))
			.sort((a, b) => b.amount.ped - a.amount.ped || a.name.localeCompare(b.name));
		if (items.length === 0) continue;
		groups.push({
			role,
			label: ROLE_LABELS[role],
			total: sumAmounts(items.map((item) => item.amount)),
			items,
		});
	}
	return groups;
}

// ── Repairs ─────────────────────────────────────────────────────────────

export interface RepairLine {
	at: string;
	what: string;
	restored: string;
	/** What the PED Card paid for it, as a cross-check against what it
	 * restored; `null` when the Card does not show it. */
	paid: string | null;
}

/** One repair: when, and what it restored. A repair settles wear the
 * session already counts, so it is never a cost; what the PED Card paid
 * for it is kept only as a cross-check. */
export function repairLine(repair: MeasuredRepair, clock: ReadingClock): RepairLine {
	const names = repair.items.map((item) => item.item.name);
	let paid: string | null = null;
	if (repair.cost.kind === 'paid') paid = `${ped(repair.cost.amount)} paid`;
	else if (repair.cost.kind === 'derived') paid = `about ${ped(repair.cost.amount)} paid`;
	return {
		at: seenAt(repair.between, clock),
		what: names.length > 0 ? listNames(names) : 'Repair',
		restored: ped(repair.restored),
		paid,
	};
}

/** Repairs in time order. */
export function repairLines(repairs: readonly MeasuredRepair[], clock: ReadingClock): RepairLine[] {
	return [...repairs]
		.sort((a, b) => Date.parse(a.between.to) - Date.parse(b.between.to))
		.map((repair) => repairLine(repair, clock));
}

// ── Consumption and ammo ────────────────────────────────────────────────

export interface AmountLine {
	name: string;
	detail: string;
	amount: MeasuredAmount;
}

/** Stacks consumed or gone, heaviest first. */
export function consumptionLines(items: readonly MeasuredConsumption[]): AmountLine[] {
	return [...items]
		.sort((a, b) => b.amount.ped - a.amount.ped || a.item.name.localeCompare(b.item.name))
		.map((item) => ({
			name: item.item.name,
			detail: `${formatUnits(item.units)} ${Math.abs(item.units) === 1 ? 'unit' : 'units'}`,
			amount: item.amount,
		}));
}

export interface AmmoLine {
	name: string;
	/** The burned figure, absent when the split could not be settled. */
	burned: MeasuredAmount | null;
	parts: string[];
}

/** Each ammo type: what burned and what else moved it, or, unsettled, its
 * net change and why no burned figure is shown. */
export function ammoLines(
	ammo: readonly MeasuredAmmo[],
	events: readonly MeasuredEvent[] = [],
): AmmoLine[] {
	const toppedUp = new Set(
		events.flatMap((event) => (event.kind === 'ammoToppedUp' ? [event.name] : [])),
	);
	return ammo.map((used) => {
		if (used.split.kind === 'unsettled') {
			return {
				name: used.name,
				burned: null,
				parts: [
					`net change ${formatUnits(used.netUnits, true)} units`,
					'the split between ammo fired and ammo paid out by strongboxes could not be settled',
				],
			};
		}
		const split = used.split;
		const parts = [
			toppedUp.has(used.name)
				? `${formatUnits(split.burnedUnits)} burned outside the span it was topped up in, which is not measured`
				: `${formatUnits(split.burnedUnits)} burned`,
		];
		if (split.strongboxUnits !== 0) {
			parts.push(
				`${formatUnits(split.strongboxUnits)} paid out by strongboxes (${ped(split.strongbox)})`,
			);
		}
		if (split.convertedUnits !== 0) {
			parts.push(`${formatUnits(split.convertedUnits)} from Shrapnel (${ped(split.converted)})`);
		}
		return { name: used.name, burned: split.burned, parts };
	});
}

/** Whether any amount the details list rests on values the site rounds
 * down to the PEC, so the list says each may be up to a PEC out. */
export function listsWholePec(measure: AccountSessionMeasure): boolean {
	const amounts: MeasuredAmount[] = [
		...measure.decay.map((item) => item.decay),
		...measure.otherConsumption.map((item) => item.amount),
		...measure.unexplainedDepartures.map((item) => item.amount),
		...measure.repairs.map((repair) => repair.restored),
		...measure.ammo.flatMap((used) => (used.split.kind === 'settled' ? [used.split.burned] : [])),
	];
	return amounts.some((amount) => amount.precision === 'wholePec');
}

/** Whether a measure has anything to list item by item. */
export function hasDetails(measure: AccountSessionMeasure): boolean {
	return (
		measure.ammo.length > 0 ||
		measure.decay.length > 0 ||
		measure.repairs.length > 0 ||
		measure.otherConsumption.length > 0 ||
		measure.unexplainedDepartures.length > 0 ||
		measure.events.length > 0
	);
}
