/**
 * A strongbox opening as the player reviews it before recording: which box,
 * what came out, what each item might be worth, and where the boxes came
 * from.
 *
 * Deliberately pure. A key pays back exactly its TT in whatever the box held,
 * mostly Universal Ammo, so an opening never gains or loses TT and the only
 * question worth asking is the markup on what came out. Every rule about how
 * a result is read, what is cubed by default, and what the run is worth lives
 * here where it can be tested on its own.
 */
import type {
	AccountProposal,
	InventoryItem,
	MarketHarvestData,
	StockPosition,
	StrongboxExportRead,
	StrongboxIntake,
	StrongboxOpeningInput,
	StrongboxSource,
	StrongboxUnexpectedReason,
} from '$lib/api/commands.gen';
import {
	type ConfidenceMode,
	effectiveItemMarkup,
	type MarkupBasis,
	manualValuation,
	marketOpportunity,
	NANOCUBE_FALLBACK_MARKUP,
} from '$lib/features/analytics/treeCuttingModel.svelte';
import { formatPed, formatQuantity } from '$lib/utils/format';

/** Every strongbox seen so far opens with a key worth this much TT. */
export const KEY_UNIT_TT = 10;
/** What one Nanocube is worth in TT. */
export const NANOCUBE_UNIT_TT = 0.01;
/** The export truncates each value to the whole PEC. */
const PEC_PER_ROW = 0.01;

export function isStrongbox(name: string): boolean {
	return /strongbox\s*$/i.test(name.trim());
}

// ── The boxes the player holds ──────────────────────────────────────

/** An asset position of boxes, which carries what it cost. */
export interface BoxAsset {
	id: string;
	quantity: number;
	markupPaid: number;
	/** False when the position goes in only whole. */
	divisible: boolean;
}

/** One box type the player holds, across loot and assets. */
export interface BoxHolding {
	name: string;
	/** Tracked loot: these carry the hunts that looted them. */
	lootQty: number;
	assets: BoxAsset[];
	total: number;
}

/** The box types held, most first. */
export function boxHoldings(loot: StockPosition[], assets: InventoryItem[]): BoxHolding[] {
	const byName = new Map<string, BoxHolding>();
	const entry = (name: string) => {
		const key = name.trim().toLocaleLowerCase();
		let found = byName.get(key);
		if (!found) {
			found = { name: name.trim(), lootQty: 0, assets: [], total: 0 };
			byName.set(key, found);
		}
		return found;
	};
	for (const row of loot) {
		if (!isStrongbox(row.itemName) || row.quantity <= 0) continue;
		const holding = entry(row.itemName);
		holding.lootQty += row.quantity;
		holding.total += row.quantity;
	}
	for (const item of assets) {
		if (!isStrongbox(item.name) || item.quantity <= 0) continue;
		const holding = entry(item.name);
		holding.assets.push({
			id: item.id,
			quantity: item.quantity,
			markupPaid: item.markupPaid,
			divisible: item.divisible,
		});
		holding.total += item.quantity;
	}
	return [...byName.values()].sort((a, b) => b.total - a.total || a.name.localeCompare(b.name));
}

// ── What came out ───────────────────────────────────────────────────

/** What the market says an item might fetch above TT. Informational only. */
export interface ItemEstimate {
	/** Markup above TT in PED, or null when nothing values the item. */
	markupPed: number | null;
	markupPct: number | null;
	basis: MarkupBasis | null;
	/** What the same TT would fetch as Nanocubes. */
	cubeMarkupPed: number;
	/** The item's own reading says it sells for no more than Nanocubes do. */
	sellsAsCubes: boolean;
}

export interface ContentLine {
	itemName: string;
	quantity: number;
	ttValue: number;
	recycle: boolean;
	estimate: ItemEstimate;
}

export interface MarketContext {
	market: MarketHarvestData | null;
	mode: ConfidenceMode;
}

export function nanocubeMarkupPct(context: MarketContext): number {
	return context.market?.nanocubeMarkupPct ?? NANOCUBE_FALLBACK_MARKUP;
}

export function estimateItem(
	itemName: string,
	quantity: number,
	ttValue: number,
	context: MarketContext,
): ItemEstimate {
	const cubePct = nanocubeMarkupPct(context);
	const cubeMarkupPed = ttValue * (cubePct / 100 - 1);
	const marketItem = context.market?.items.find(
		(item) => item.itemName.toLocaleLowerCase() === itemName.trim().toLocaleLowerCase(),
	);
	const manual = manualValuation(marketItem, quantity, ttValue);
	const ownReading = marketItem?.markupPct ?? null;
	// Nothing at all to go on: say so rather than value it as Nanocubes,
	// which is what the floor would otherwise quietly do.
	if (!manual && ownReading === null) {
		return { markupPed: null, markupPct: null, basis: null, cubeMarkupPed, sellsAsCubes: false };
	}
	const applied = effectiveItemMarkup(
		itemName,
		context.market ? marketOpportunity(marketItem, cubePct) : null,
		cubePct,
		context.mode,
		{ manual },
	);
	const markupPed = manual
		? manual.valuePed - ttValue
		: applied
			? ttValue * (applied.markupPct / 100 - 1)
			: null;
	return {
		markupPed,
		markupPct: applied?.markupPct ?? null,
		basis: applied?.basis ?? null,
		cubeMarkupPed,
		sellsAsCubes: !manual && ownReading !== null && ownReading <= cubePct,
	};
}

/** The markup a line is expected to carry as decided: as itself, or as cubes. */
export function lineMarkup(line: ContentLine): number | null {
	if (line.recycle) return line.estimate.cubeMarkupPed;
	return line.estimate.markupPed;
}

/** A kept item the market values above what Nanocubes would fetch. */
export function isHit(line: ContentLine): boolean {
	return (
		!line.recycle &&
		line.estimate.markupPed !== null &&
		line.estimate.markupPed > line.estimate.cubeMarkupPed + 0.005
	);
}

export function contentLine(
	itemName: string,
	quantity: number,
	ttValue: number,
	context: MarketContext,
): ContentLine {
	const estimate = estimateItem(itemName, quantity, ttValue, context);
	return {
		itemName: itemName.trim(),
		quantity,
		ttValue,
		// Every content is kept: recycling is done in game, and the
		// account readings record it when it happens.
		recycle: false,
		estimate,
	};
}

/** Re-value every line against fresh market data, keeping the decisions. */
export function revalue(lines: ContentLine[], context: MarketContext): ContentLine[] {
	return lines.map((line) => ({
		...line,
		estimate: estimateItem(line.itemName, line.quantity, line.ttValue, context),
	}));
}

/** Hits first, then by what each is expected to fetch, then by TT. */
export function orderLines(lines: ContentLine[]): ContentLine[] {
	return [...lines].sort((a, b) => {
		const hit = Number(isHit(b)) - Number(isHit(a));
		if (hit) return hit;
		const markup = (b.estimate.markupPed ?? -Infinity) - (a.estimate.markupPed ?? -Infinity);
		if (markup) return markup;
		return b.ttValue - a.ttValue || a.itemName.localeCompare(b.itemName);
	});
}

// ── A result, read or entered ───────────────────────────────────────

export interface Unexpected {
	itemName: string;
	quantity: number;
	valuePed: number;
	reason: StrongboxUnexpectedReason;
}

/** Something that arrived beside an opening and may or may not be from
 * the boxes: left out unless the player adds it. */
export interface PossibleLine extends ContentLine {
	included: boolean;
}

/** A run's result, ready to review. */
export interface OpeningResult {
	boxItem: string;
	boxes: number;
	keyUnitTt: number;
	ammoPed: number;
	lines: ContentLine[];
	/** Arrivals that may be the boxes' too, off until the player adds them. */
	possible: PossibleLine[];
	/** The ammo is whatever the keys did not pay back in items, so it moves
	 * as possible items are added; otherwise it was read as it is. */
	ammoFromKeys: boolean;
	/** The ammo the account's readings showed, when they settled it; the
	 * review says so when it differs from the ammo recorded. */
	readAmmoPed: number | null;
	/** How many of the boxes paid ammo alone, when the readings show it
	 * exactly. */
	paidAmmoAlone: number | null;
	intake: StrongboxIntake;
	/** Export rows left out of the opening, with why. */
	unexpected: Unexpected[];
	/** Pasted lines that were not rows. */
	skipped: { line: number; text: string; reason: string }[];
	/** Something about the result the player should look at; nothing blocks. */
	notes: string[];
}

/** Every line the run records: what came out, and the possible items the
 * player added. */
export function contentLines(result: OpeningResult): ContentLine[] {
	return [...result.lines, ...result.possible.filter((line) => line.included)];
}

export const UNEXPECTED_LABEL: Record<StrongboxUnexpectedReason, string> = {
	other_box: 'another kind of box',
	removed: 'left the inventory',
	gained: 'arrived rather than left',
	worn: 'changed in TT only',
};

/** Why an export cannot be read as an opening of this box, or null. */
export function exportRefusal(read: StrongboxExportRead, boxItem: string): string | null {
	if (read.boxesOpened > 0) return null;
	if (read.contents.length === 0 && read.unexpected.length === 0 && read.ammoPed === 0) {
		return read.skipped.length > 0
			? 'That does not look like the Inventory Changes export: no line read as an item.'
			: 'There is nothing to read.';
	}
	const other = read.unexpected.find((row) => row.reason === 'other_box');
	if (other) {
		return `The export shows ${other.row.itemName} opened, not ${boxItem}. Choose that box instead.`;
	}
	return `The export shows no ${boxItem} opened. Reset the page before opening, then copy it again.`;
}

export function resultFromExport(
	read: StrongboxExportRead,
	boxItem: string,
	context: MarketContext,
): OpeningResult {
	const notes: string[] = [];
	let keyUnitTt = KEY_UNIT_TT;
	if (read.keys && read.keys.count > 0) {
		keyUnitTt = read.keys.ttValue / read.keys.count;
		if (Math.abs(read.keys.count - read.boxesOpened) > 1e-9) {
			notes.push(
				`${formatQuantity(read.keys.count)} keys were used for ${formatQuantity(read.boxesOpened)} boxes.`,
			);
		}
	} else {
		notes.push(`The export shows no keys used; each is taken at ${formatPed(KEY_UNIT_TT)} PED.`);
	}
	return {
		boxItem,
		boxes: read.boxesOpened,
		keyUnitTt,
		ammoPed: read.ammoPed,
		lines: orderLines(
			read.contents.map((row) => contentLine(row.itemName, row.quantity, row.valuePed, context)),
		),
		possible: [],
		ammoFromKeys: false,
		readAmmoPed: null,
		paidAmmoAlone: null,
		intake: 'export',
		unexpected: read.unexpected.map(({ row, reason }) => ({
			itemName: row.itemName,
			quantity: row.quantity,
			valuePed: row.valuePed,
			reason,
		})),
		skipped: read.skipped,
		notes,
	};
}

/** An item the player enters by hand. */
export interface ManualLine {
	id: number;
	itemName: string;
	quantity: number | null;
	ttValue: number | null;
}

export interface ManualEntry {
	boxes: number | null;
	keyUnitTt: number;
	lines: ManualLine[];
}

/** The lines that say something: a name and a positive quantity. */
function filledLines(entry: ManualEntry): ManualLine[] {
	return entry.lines.filter((line) => line.itemName.trim() && (line.quantity ?? 0) > 0);
}

export function manualItemsTt(entry: ManualEntry): number {
	return filledLines(entry).reduce((sum, line) => sum + (line.ttValue ?? 0), 0);
}

/** The ammo a hand-entered run implies: whatever the keys did not pay back in items. */
export function manualAmmo(entry: ManualEntry): number {
	return Math.max(0, (entry.boxes ?? 0) * entry.keyUnitTt - manualItemsTt(entry));
}

/** Why a hand-entered result cannot be reviewed yet, or null. */
export function manualRefusal(entry: ManualEntry): string | null {
	const boxes = entry.boxes ?? 0;
	if (!(boxes > 0) || !Number.isInteger(boxes)) return 'Say how many boxes you opened.';
	if (!(entry.keyUnitTt > 0)) return 'A key needs its TT.';
	for (const line of entry.lines) {
		const named = line.itemName.trim() !== '';
		const counted = (line.quantity ?? 0) > 0;
		if (named !== counted) return 'Every item needs a name and a quantity.';
		if (named && /^universal ammo$/i.test(line.itemName.trim())) {
			return 'Leave the Universal Ammo out: it is whatever the keys did not pay back in items.';
		}
		if (named && line.ttValue === null) return `Give ${line.itemName.trim()} its TT.`;
	}
	const items = manualItemsTt(entry);
	const keys = boxes * entry.keyUnitTt;
	if (items > keys + PEC_PER_ROW) {
		return `The items come to ${formatPed(items)} PED TT, more than the ${formatPed(keys)} PED the keys pay back.`;
	}
	return null;
}

export function resultFromManual(
	entry: ManualEntry,
	boxItem: string,
	context: MarketContext,
): OpeningResult {
	return {
		boxItem,
		boxes: entry.boxes ?? 0,
		keyUnitTt: entry.keyUnitTt,
		ammoPed: manualAmmo(entry),
		lines: orderLines(
			filledLines(entry).map((line) =>
				contentLine(line.itemName, line.quantity ?? 0, line.ttValue ?? 0, context),
			),
		),
		possible: [],
		ammoFromKeys: false,
		readAmmoPed: null,
		paidAmmoAlone: null,
		intake: 'manual',
		unexpected: [],
		skipped: [],
		notes: [],
	};
}

// ── A result read from the account ──────────────────────────────────

/** An opening the account readings propose. */
export type OpeningProposal = AccountProposal & {
	outcome: Extract<AccountProposal['outcome'], { kind: 'opening' }>;
};

export function isOpeningProposal(proposal: AccountProposal): proposal is OpeningProposal {
	return proposal.outcome.kind === 'opening';
}

/** The one box type a proposal opened, or null when it was more than one. */
export function proposedBox(proposal: OpeningProposal): string | null {
	return proposal.outcome.boxes.length === 1 ? proposal.outcome.boxes[0].name : null;
}

/** Why proposals cannot be reviewed as an opening of this box, or null. */
export function proposalRefusal(proposals: OpeningProposal[], boxItem: string): string | null {
	if (proposals.length === 0) return `Nothing was read from your account for ${boxItem}.`;
	for (const proposal of proposals) {
		const box = proposedBox(proposal);
		if (box === null) {
			return 'Your account shows more than one kind of box opened together. Paste the export or enter it by hand.';
		}
		if (box.toLocaleLowerCase() !== boxItem.trim().toLocaleLowerCase()) {
			return `Your account shows ${box} opened, not ${boxItem}. Choose that box instead.`;
		}
	}
	return null;
}

interface ProposedItem {
	name: string;
	units: number;
	ped: number;
}

/** One line per item name: the readings may see the same item arrive in two
 * steps of one run. */
function mergeItems(items: ProposedItem[]): ProposedItem[] {
	const merged = new Map<string, ProposedItem>();
	for (const item of items) {
		const key = item.name.trim().toLocaleLowerCase();
		const held = merged.get(key);
		if (held) {
			held.units += item.units;
			held.ped += item.ped;
		} else {
			merged.set(key, { ...item });
		}
	}
	return [...merged.values()];
}

function itemsTt(lines: ContentLine[]): number {
	return lines.reduce((sum, line) => sum + line.ttValue, 0);
}

type Doubt = NonNullable<OpeningProposal['outcome']['doubt']>;

/** What each doubt means, in words; more than one kind of box is refused
 * before a result is built. */
const DOUBT_NOTE: Record<Doubt, ((keys: number, boxes: number) => string) | null> = {
	severalKinds: null,
	noKeys: () => `Your account shows no keys used; each is taken at ${formatPed(KEY_UNIT_TT)} PED.`,
	countsDiffer: (keys, boxes) =>
		`${formatQuantity(keys)} keys were used for ${formatQuantity(boxes)} boxes.`,
	unbalanced: () =>
		'What arrived does not pay the keys back in one clear way, so the ammo is taken as what the keys did not pay back in items.',
};

/**
 * The result the account readings propose for a run, built as a pasted
 * export would be: one box type, its keys, what came out, and the ammo.
 * Several proposals (a run read across more than one minute) add up to one
 * result. The ammo is what the keys did not pay back in items, as an
 * entry by hand takes it (a settled reading's own figure, when it differs,
 * is noted beside it). An opening that did not
 * settle keeps its doubt in the notes; what arrived that
 * may or may not be the boxes' is listed apart, left out until the player
 * adds it. Call only once `proposalRefusal` has passed.
 */
export function resultFromProposals(
	proposals: OpeningProposal[],
	boxItem: string,
	context: MarketContext,
): OpeningResult {
	const outcomes = proposals.map((proposal) => proposal.outcome);
	const boxes = outcomes.reduce((sum, outcome) => sum + outcome.boxes[0].count, 0);
	const keysUsed = outcomes.reduce((sum, outcome) => sum + outcome.keysUsed, 0);
	const keysTt = outcomes.reduce((sum, outcome) => sum + outcome.keys.ped, 0);
	const settled = outcomes.every((outcome) => outcome.ammo !== null);
	const notes: string[] = [];
	for (const outcome of outcomes) {
		const note = outcome.doubt ? DOUBT_NOTE[outcome.doubt] : null;
		if (note) {
			const text = note(outcome.keysUsed, outcome.boxes[0].count);
			if (!notes.includes(text)) notes.push(text);
		}
	}
	const keyUnitTt = keysUsed > 0 ? keysTt / keysUsed : KEY_UNIT_TT;
	const items = (
		pick: (outcome: OpeningProposal['outcome']) => OpeningProposal['outcome']['contents'],
	) =>
		mergeItems(
			outcomes.flatMap((outcome) =>
				pick(outcome).map((item) => ({
					name: item.item.name,
					units: item.units,
					ped: item.amount.ped,
				})),
			),
		);
	const lines = orderLines(
		items((outcome) => outcome.contents).map((item) =>
			contentLine(item.name, item.units, item.ped, context),
		),
	);
	const possible: PossibleLine[] = items((outcome) => outcome.possibleContents).map((item) => ({
		...contentLine(item.name, item.units, item.ped, context),
		recycle: false,
		included: false,
	}));
	if (possible.length > 0) {
		notes.push(
			possible.length === 1
				? `${possible[0].itemName} arrived at the same time and may not be from the boxes, so it is left out unless you add it.`
				: `${possible.length} items arrived at the same time and may not be from the boxes, so they are left out unless you add them.`,
		);
	}
	const paid = outcomes.map((outcome) => outcome.paidAmmoAlone);
	// The ammo is the keys less what else came out, settled or not: a
	// settled reading already showed the rows agree with that within the
	// site's truncation, and the keys are what the boxes pay back whole.
	const ammoPed = Math.max(0, boxes * keyUnitTt - itemsTt(lines));
	const readAmmoPed = settled
		? outcomes.reduce((sum, outcome) => sum + (outcome.ammo?.amount.ped ?? 0), 0)
		: null;
	return {
		boxItem,
		boxes,
		keyUnitTt,
		ammoPed,
		lines,
		possible,
		ammoFromKeys: true,
		readAmmoPed,
		paidAmmoAlone: paid.every((count) => count !== null)
			? paid.reduce((sum: number, count) => sum + (count ?? 0), 0)
			: null,
		// Read from the site's own change list: the same delta the export
		// carries.
		intake: 'export',
		unexpected: [],
		skipped: [],
		notes,
	};
}

/** Add a possible item to the run, or leave it out again. An ammo figure
 * that is what the keys did not pay back in items moves with it. */
export function withPossible(
	result: OpeningResult,
	itemName: string,
	included: boolean,
): OpeningResult {
	const possible = result.possible.map((line) =>
		line.itemName === itemName ? { ...line, included } : line,
	);
	const next = { ...result, possible };
	if (result.ammoFromKeys) {
		next.ammoPed = Math.max(0, result.boxes * result.keyUnitTt - itemsTt(contentLines(next)));
	}
	return next;
}

/** What the review says of the ammo when the account's readings showed a
 * figure other than the one recorded (the keys less what else came out).
 * Worked out from the result as it stands, so it follows the possible items
 * the player adds or leaves out. */
export function ammoNote(result: OpeningResult): string | null {
	const read = result.readAmmoPed;
	if (read === null || Math.abs(read - result.ammoPed) <= 1e-6) return null;
	return `Your account shows ${formatPed(read)} PED of ammo; the keys less what else came out are ${formatPed(result.ammoPed)} PED, and that is the figure recorded.`;
}

/** The quiet line on how the boxes paid, when the readings show it. */
export function paidAloneSentence(result: OpeningResult): string | null {
	const alone = result.paidAmmoAlone;
	if (alone === null || alone <= 0 || alone > result.boxes) return null;
	if (alone === result.boxes) return `All ${formatQuantity(result.boxes)} paid ammo alone`;
	return `${formatQuantity(alone)} of ${formatQuantity(result.boxes)} paid ammo alone`;
}

// ── The run's figures ───────────────────────────────────────────────

export interface OpeningSummary {
	keysTt: number;
	ammoPed: number;
	keptTt: number;
	cubedTt: number;
	nanocubes: number;
	/** TT of what came out minus the keys': zero for a clean run. */
	gap: number;
	/** Expected markup of the run as decided, over the lines that have one. */
	estimatedMarkup: number;
	/** Lines nothing values: their markup is left out of the estimate. */
	unvalued: number;
	hits: number;
}

export function summarise(result: OpeningResult): OpeningSummary {
	const keysTt = result.boxes * result.keyUnitTt;
	let keptTt = 0;
	let cubedTt = 0;
	let estimatedMarkup = 0;
	let unvalued = 0;
	let hits = 0;
	for (const line of contentLines(result)) {
		if (line.recycle) cubedTt += line.ttValue;
		else keptTt += line.ttValue;
		const markup = lineMarkup(line);
		if (markup === null) unvalued += 1;
		else estimatedMarkup += markup;
		if (isHit(line)) hits += 1;
	}
	return {
		keysTt,
		ammoPed: result.ammoPed,
		keptTt,
		cubedTt,
		nanocubes: Math.round(cubedTt / NANOCUBE_UNIT_TT),
		gap: keptTt + cubedTt + result.ammoPed - keysTt,
		estimatedMarkup,
		unvalued,
		hits,
	};
}

/** Why the run cannot be recorded as it stands, or null. */
export function balanceRefusal(result: OpeningResult): string | null {
	const { gap, keysTt } = summarise(result);
	const tolerance = PEC_PER_ROW * (contentLines(result).length + 1) + 1e-9;
	if (Math.abs(gap) <= tolerance) return null;
	return `What came out comes to ${formatPed(keysTt + gap)} PED TT, but the keys are ${formatPed(keysTt)} PED. A key always pays back its TT, so something is missing from the result.`;
}

// ── Where the boxes came from ───────────────────────────────────────

export interface AssetShare {
	id: string;
	quantity: number;
	held: number;
	markupPerBox: number;
	divisible: boolean;
}

export interface SourcePlan {
	/** From loot: tracked, or held from before tracking began. */
	loot: number;
	assets: AssetShare[];
	bought: number;
	/** What the bought boxes cost above TT, as the player enters it. */
	boughtMarkup: number | null;
	boughtMarkupPer: 'total' | 'box';
}

/** Loot first, then assets, and anything beyond what is held as loot again:
 * boxes held from before tracking began, which credit no activity. Bought
 * boxes are the player's to say. */
export function defaultPlan(boxes: number, holding: BoxHolding | null): SourcePlan {
	let left = boxes;
	const loot = Math.min(left, holding?.lootQty ?? 0);
	left -= loot;
	const assets: AssetShare[] = (holding?.assets ?? []).map((asset) => {
		const take = asset.divisible
			? Math.min(left, asset.quantity)
			: left >= asset.quantity
				? asset.quantity
				: 0;
		left -= take;
		return {
			id: asset.id,
			quantity: take,
			held: asset.quantity,
			markupPerBox: asset.quantity > 0 ? asset.markupPaid / asset.quantity : 0,
			divisible: asset.divisible,
		};
	});
	return { loot: loot + left, assets, bought: 0, boughtMarkup: null, boughtMarkupPer: 'total' };
}

export function planTotal(plan: SourcePlan): number {
	return plan.loot + plan.bought + plan.assets.reduce((sum, asset) => sum + asset.quantity, 0);
}

export function boughtMarkupTotal(plan: SourcePlan): number {
	const entered = plan.boughtMarkup ?? 0;
	return plan.boughtMarkupPer === 'box' ? entered * plan.bought : entered;
}

/** What the opened boxes cost above TT: bought for the run, or held as assets. */
export function boxCost(plan: SourcePlan): number {
	return (
		boughtMarkupTotal(plan) +
		plan.assets.reduce((sum, asset) => sum + asset.quantity * asset.markupPerBox, 0)
	);
}

/** Boxes the plan draws from loot beyond what the app tracked. */
export function untrackedBoxes(plan: SourcePlan, holding: BoxHolding | null): number {
	return Math.max(0, plan.loot - (holding?.lootQty ?? 0));
}

export function planRefusal(plan: SourcePlan, boxes: number): string | null {
	const counts = [plan.loot, plan.bought, ...plan.assets.map((asset) => asset.quantity)];
	if (counts.some((count) => count < 0 || !Number.isInteger(count))) {
		return 'Box counts are whole numbers.';
	}
	for (const asset of plan.assets) {
		if (asset.quantity > asset.held) return 'More boxes are drawn from an asset than it holds.';
		if (!asset.divisible && asset.quantity !== 0 && asset.quantity !== asset.held) {
			return 'That asset goes in only whole.';
		}
	}
	const total = planTotal(plan);
	if (total !== boxes) {
		return `The sources come to ${formatQuantity(total)} boxes, but ${formatQuantity(boxes)} were opened.`;
	}
	if (plan.bought > 0 && plan.boughtMarkup === null) {
		return 'Say what the bought boxes cost above TT (0 if nothing).';
	}
	if ((plan.boughtMarkup ?? 0) < 0) return 'A markup paid cannot be negative.';
	return null;
}

// ── The request ─────────────────────────────────────────────────────

export function openingInput(
	result: OpeningResult,
	plan: SourcePlan,
	openedAt: string | null,
): StrongboxOpeningInput {
	const sources: StrongboxSource[] = [];
	if (plan.loot > 0) sources.push({ kind: 'held', quantity: plan.loot });
	for (const asset of plan.assets) {
		if (asset.quantity > 0) {
			sources.push({ kind: 'asset', itemId: asset.id, quantity: asset.quantity });
		}
	}
	if (plan.bought > 0) {
		sources.push({ kind: 'bought', quantity: plan.bought, markupPaid: boughtMarkupTotal(plan) });
	}
	return {
		boxItem: result.boxItem,
		sources,
		keyUnitTt: result.keyUnitTt,
		keyMarkupPaid: null,
		contents: contentLines(result).map((line) => ({
			itemName: line.itemName,
			quantity: line.quantity,
			ttValue: line.ttValue,
			recycle: line.recycle,
		})),
		ammoPed: result.ammoPed,
		intake: result.intake,
		openedAt: openedAt || null,
	};
}
