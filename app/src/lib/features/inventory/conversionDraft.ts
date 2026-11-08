/**
 * The conversion form's draft: what the player has said went in and came out,
 * and what the app can work out from that so they do not have to.
 *
 * Deliberately pure. The form asks only for what cannot be derived, so every
 * rule about which field appears, what a TT fills in with, and what the
 * outcome sentence says lives here where it can be tested on its own.
 */
import type {
	ActivityHistoryEntry,
	ConversionHolding,
	ConversionInputLine,
	DerivedSource,
	HoldingOrigin,
	ItemConversionInput,
} from '$lib/api/commands.gen';
import { formatPed } from '$lib/utils/format';

/** Something the player holds, or has held, that a line can name. */
export interface ConvertibleItem {
	name: string;
	/** Set for a whole asset. Loot is identified by its name. */
	assetId: string | null;
	heldQty: number;
	unitTt: number;
	/** Markup already paid per unit, which rides along with the item. */
	paidPerUnit: number;
	origins: HoldingOrigin[];
}

export interface InputDraft {
	id: number;
	itemName: string;
	assetId: string | null;
	quantity: number | null;
	/** The player says these particular ones were bought, though some are held. */
	bought: boolean;
	ttValue: number | null;
	/** Once typed, a TT stops following the quantity. */
	ttEdited: boolean;
	/** Markup paid on a bought line, or worth on one of the player's own. */
	markup: number | null;
}

export interface OutputDraft {
	id: number;
	itemName: string;
	quantity: number | null;
	ttValue: number | null;
	ttEdited: boolean;
	holding: ConversionHolding;
}

export interface ConversionDraft {
	inputs: InputDraft[];
	outputs: OutputDraft[];
	/** Null while the PED outcome field is not showing. */
	pedAmount: number | null;
	showPed: boolean;
}

export type InputKind = 'held' | 'bought' | 'asset';

let nextLineId = 1;

export function emptyInput(fields: Partial<InputDraft> = {}): InputDraft {
	return {
		id: nextLineId++,
		itemName: '',
		assetId: null,
		quantity: null,
		bought: false,
		ttValue: null,
		ttEdited: false,
		markup: null,
		...fields,
	};
}

export function emptyOutput(fields: Partial<OutputDraft> = {}): OutputDraft {
	return {
		id: nextLineId++,
		itemName: '',
		quantity: 1,
		ttValue: null,
		ttEdited: false,
		holding: 'loot',
		...fields,
	};
}

export function emptyDraft(
	start?: { name: string; assetId: string | null } | null,
): ConversionDraft {
	return {
		inputs: [emptyInput(start ? { itemName: start.name, assetId: start.assetId } : {})],
		outputs: [emptyOutput()],
		pedAmount: null,
		showPed: false,
	};
}

function findItem(line: InputDraft, items: ConvertibleItem[]): ConvertibleItem | undefined {
	const name = line.itemName.trim();
	return line.assetId
		? items.find((item) => item.assetId === line.assetId)
		: items.find((item) => item.assetId === null && item.name === name);
}

/** Whether the player holds any of this line's item at all. */
export function isHeldItem(line: InputDraft, items: ConvertibleItem[]): boolean {
	return (findItem(line, items)?.heldQty ?? 0) > 0;
}

/** What the player holds of this line's item. */
export function heldQuantity(line: InputDraft, items: ConvertibleItem[]): number {
	return findItem(line, items)?.heldQty ?? 0;
}

/** Where a line comes from, which decides what it asks for. */
export function inputKind(line: InputDraft, items: ConvertibleItem[]): InputKind {
	if (line.assetId && findItem(line, items)) return 'asset';
	return isHeldItem(line, items) && !line.bought ? 'held' : 'bought';
}

function isBlankInput(line: InputDraft): boolean {
	return line.itemName.trim() === '' && line.quantity === null;
}

function isBlankOutput(line: OutputDraft): boolean {
	return line.itemName.trim() === '' && line.ttValue === null;
}

function knownUnitTt(name: string, items: ConvertibleItem[]): number | null {
	const unit = items.find((item) => item.assetId === null && item.name === name.trim())?.unitTt;
	return unit && unit > 0 ? unit : null;
}

/** The TT a known item fills in with for this quantity, when there is one. */
export function suggestedTt(
	itemName: string,
	quantity: number | null,
	items: ConvertibleItem[],
): number | null {
	const unit = knownUnitTt(itemName, items);
	if (unit === null || !quantity || quantity <= 0) return null;
	return Math.round(unit * quantity * 10000) / 10000;
}

function lineTt(
	line: { itemName: string; quantity: number | null; ttValue: number | null; ttEdited: boolean },
	items: ConvertibleItem[],
): number | null {
	return line.ttEdited ? line.ttValue : suggestedTt(line.itemName, line.quantity, items);
}

/** The TT a bought line should currently show. */
export function shownInputTt(line: InputDraft, items: ConvertibleItem[]): number | null {
	return lineTt(line, items);
}

/** The TT an output line should currently show. */
export function shownOutputTt(line: OutputDraft, items: ConvertibleItem[]): number | null {
	return lineTt(line, items);
}

function hasOrigin(line: InputDraft, items: ConvertibleItem[]): boolean {
	const kind = inputKind(line, items);
	return kind !== 'bought' && (findItem(line, items)?.origins.length ?? 0) > 0;
}

/**
 * Whether a worth field can change anything. With a single source there is
 * nothing to divide, so the question is never asked.
 */
export function worthIsShown(draft: ConversionDraft, items: ConvertibleItem[]): boolean {
	return draft.inputs.filter((line) => !isBlankInput(line) && hasOrigin(line, items)).length >= 2;
}

/** How much a held line's quantity runs past what is held, if it does. */
export function excessOverHeld(line: InputDraft, items: ConvertibleItem[]): number {
	if (inputKind(line, items) !== 'held' || !line.quantity) return 0;
	return Math.max(0, line.quantity - heldQuantity(line, items));
}

function inputTt(line: InputDraft, items: ConvertibleItem[]): number | null {
	const kind = inputKind(line, items);
	if (kind === 'bought') return lineTt(line, items);
	const item = findItem(line, items);
	if (!item) return 0;
	return kind === 'asset' ? item.unitTt : (line.quantity ?? 0) * item.unitTt;
}

function inputPaid(line: InputDraft, items: ConvertibleItem[]): number {
	const kind = inputKind(line, items);
	if (kind === 'bought') return line.markup ?? 0;
	const item = findItem(line, items);
	if (!item) return 0;
	return kind === 'asset' ? item.paidPerUnit : (line.quantity ?? 0) * item.paidPerUnit;
}

function isInputComplete(line: InputDraft, items: ConvertibleItem[]): boolean {
	if (line.itemName.trim() === '') return false;
	if (inputKind(line, items) === 'asset') return true;
	if (!line.quantity || line.quantity <= 0) return false;
	return inputKind(line, items) === 'held' || lineTt(line, items) !== null;
}

function isOutputComplete(line: OutputDraft, items: ConvertibleItem[]): boolean {
	if (line.itemName.trim() === '') return false;
	if (line.holding === 'loot' && (!line.quantity || line.quantity <= 0)) return false;
	return lineTt(line, items) !== null;
}

function plural(count: number, noun: string): string {
	return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

function listNames(names: string[]): string {
	if (names.length <= 1) return names[0] ?? '';
	if (names.length === 2) return `${names[0]} and ${names[1]}`;
	return `${names[0]}, ${names[1]} and ${plural(names.length - 2, 'other')}`;
}

function originsOf(lines: InputDraft[], items: ConvertibleItem[]): string[] {
	const totals = new Map<string, number>();
	for (const line of lines) {
		for (const origin of findItem(line, items)?.origins ?? []) {
			totals.set(origin.label, (totals.get(origin.label) ?? 0) + origin.quantity);
		}
	}
	return [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([label]) => label);
}

export interface ConversionPreview {
	/** Everything needed to record it has been given. */
	ready: boolean;
	/** The outcome in plain words, or null until there is something to say. */
	sentence: string | null;
}

function signed(value: number): string {
	return `${value >= 0 ? '+' : ''}${formatPed(value)}`;
}

/** Say what recording this conversion will do, in the player's terms. */
export function previewConversion(
	draft: ConversionDraft,
	items: ConvertibleItem[],
): ConversionPreview {
	const inputs = draft.inputs.filter((line) => !isBlankInput(line));
	const outputs = draft.outputs.filter((line) => !isBlankOutput(line));
	const ped = draft.showPed ? (draft.pedAmount ?? 0) : 0;
	const hasResult = outputs.length > 0 || ped > 0;
	const ready =
		inputs.length > 0 &&
		inputs.every((line) => isInputComplete(line, items)) &&
		outputs.every((line) => isOutputComplete(line, items)) &&
		hasResult;
	if (!ready) return { ready: false, sentence: null };

	const sourced = inputs.filter((line) => hasOrigin(line, items));
	const destination = listNames(originsOf(sourced, items));
	const ttIn = inputs.reduce((sum, line) => sum + (inputTt(line, items) ?? 0), 0);
	const paid = inputs.reduce((sum, line) => sum + inputPaid(line, items), 0);
	const ttOut = outputs.reduce((sum, line) => sum + (lineTt(line, items) ?? 0), 0);
	const difference = ttOut + ped - ttIn;
	let recognised: number;
	let carried: number;
	if (outputs.length === 0) {
		recognised = difference - paid;
		carried = 0;
	} else if (difference > 0) {
		const netted = Math.min(difference, paid);
		recognised = difference - netted;
		carried = paid - netted;
	} else {
		recognised = difference;
		carried = paid;
	}

	const parts: string[] = [];
	const hasDestination = destination !== '';
	if (Math.abs(recognised) > 1e-9) {
		const isPedOnly = outputs.length === 0;
		if (recognised > 0) {
			const what = isPedOnly ? `${signed(recognised)} PED` : `${signed(recognised)} PED of TT`;
			parts.push(
				hasDestination
					? `${what} ${isPedOnly ? 'goes to' : 'counts now, for'} ${destination}.`
					: `${what} goes to your ledger, with no activity behind it.`,
			);
		} else {
			parts.push(
				hasDestination
					? `${formatPed(-recognised)} PED is lost here, charged to ${destination}.`
					: `${formatPed(-recognised)} PED is lost here.`,
			);
		}
	} else if (outputs.length === 0) {
		parts.push('This nets to nothing.');
	}

	if (outputs.length > 0) {
		const single = outputs.length === 1;
		const only = outputs[0];
		const count = only.holding === 'asset' ? 1 : (only.quantity ?? 1);
		parts.push(
			single
				? `You'll hold ${count} ${only.itemName.trim()}.`
				: `You'll hold ${outputs.length} kinds of item.`,
		);
		if (!hasDestination) {
			parts.push("None of this came from tracked play, so selling it won't credit an activity.");
		} else {
			const makes = single && count === 1 ? 'it makes' : 'they make';
			const threshold =
				carried > 1e-9
					? `above ${formatPed(ttOut + carried)}, after fees,`
					: ttOut > 1e-9
						? `above ${single && count === 1 ? 'its' : 'their'} ${formatPed(ttOut)} TT, after fees,`
						: 'after fees';
			const worthLines = worthIsShown(draft, items)
				? sourced.filter((line) => (line.markup ?? 0) > 0)
				: [];
			if (worthLines.length > 0) {
				const owed = worthLines.reduce((sum, line) => sum + (line.markup ?? 0), 0);
				const blanks = sourced.filter((line) => !worthLines.includes(line));
				parts.push(
					blanks.length > 0
						? `Anything ${makes} ${threshold} pays ${formatPed(owed)} to ${listNames(originsOf(worthLines, items))} first, then the rest goes to ${listNames(originsOf(blanks, items))}.`
						: `Anything ${makes} ${threshold} is shared by what you said each is worth, across ${destination}.`,
				);
			} else {
				parts.push(`Anything ${makes} ${threshold} goes to ${destination}.`);
			}
		}
	}

	return { ready: true, sentence: parts.join(' ') };
}

function positive(value: number | null): number | null {
	return value && value > 0 ? value : null;
}

/** The recorded form of a finished draft. */
export function toConversionRequest(
	draft: ConversionDraft,
	items: ConvertibleItem[],
	convertedAt: string | null,
): ItemConversionInput {
	const showWorth = worthIsShown(draft, items);
	const worthOf = (line: InputDraft) => (showWorth ? positive(line.markup) : null);
	const inputs = draft.inputs
		.filter((line) => !isBlankInput(line))
		.map((line): ConversionInputLine => {
			const kind = inputKind(line, items);
			const itemName = line.itemName.trim();
			if (kind === 'asset') {
				return { kind: 'asset', itemId: line.assetId ?? '', worth: worthOf(line) };
			}
			if (kind === 'held') {
				return { kind: 'held', itemName, quantity: line.quantity ?? 0, worth: worthOf(line) };
			}
			return {
				kind: 'bought',
				itemName,
				quantity: line.quantity ?? 0,
				ttValue: lineTt(line, items) ?? 0,
				markupPaid: positive(line.markup),
			};
		});
	const outputs = draft.outputs
		.filter((line) => !isBlankOutput(line))
		.map((line) => ({
			itemName: line.itemName.trim(),
			quantity: line.holding === 'asset' ? 1 : (line.quantity ?? 0),
			ttValue: lineTt(line, items) ?? 0,
			holding: line.holding,
		}));
	return {
		inputs,
		outputs,
		pedAmount: draft.showPed ? positive(draft.pedAmount) : null,
		convertedAt,
	};
}

/** The newest conversion that used this item, for "Same as last time". */
export function lastConversionFor(
	itemName: string,
	history: ActivityHistoryEntry[],
): ActivityHistoryEntry | null {
	const name = itemName.trim();
	if (name === '') return null;
	return (
		history.find(
			(entry) =>
				entry.kind === 'conversion' &&
				entry.itemised &&
				!entry.undone &&
				entry.conversionLines.some((line) => line.side === 'input' && line.itemName === name),
		) ?? null
	);
}

/** A fresh draft repeating an earlier conversion. */
export function draftFromEntry(
	entry: ActivityHistoryEntry,
	items: ConvertibleItem[],
): ConversionDraft {
	const inputs = entry.conversionLines
		.filter((line) => line.side === 'input')
		.map((line) =>
			emptyInput({
				itemName: line.itemName,
				quantity: line.quantity,
				markup: line.markup,
				bought:
					line.acquisition === 'bought' &&
					items.some((item) => item.assetId === null && item.name === line.itemName),
				ttValue: line.acquisition === 'bought' ? line.ttValue : null,
				ttEdited: line.acquisition === 'bought',
			}),
		);
	const outputs = entry.conversionLines
		.filter((line) => line.side === 'output')
		.map((line) =>
			emptyOutput({
				itemName: line.itemName,
				quantity: line.quantity,
				ttValue: line.ttValue,
				ttEdited: true,
				holding: line.holding ?? 'loot',
			}),
		);
	return {
		inputs: inputs.length > 0 ? inputs : [emptyInput()],
		outputs: outputs.length > 0 || entry.pedAmount === null ? outputs : [],
		pedAmount: entry.pedAmount,
		showPed: entry.pedAmount !== null,
	};
}

/** The quiet line under a holding that came out of a conversion. */
export function derivedFromLabel(sources: DerivedSource[] | undefined): string | null {
	if (!sources || sources.length === 0) return null;
	const names = sources.slice(0, 2).map((source) => {
		const whole = Math.round(source.quantity);
		return Math.abs(source.quantity - whole) < 1e-6 && whole > 0
			? `${whole} ${source.itemName}`
			: source.itemName;
	});
	const more = sources.length > 2 ? ' and more' : '';
	return `From ${names.join(' and ')}${more}`;
}
