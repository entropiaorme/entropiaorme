/**
 * The strongbox opening view's model: choose a box, open it in game, review
 * what came out, and record the run.
 *
 * Opening is a deliberate moment of its own, so the view will not begin a
 * run while a tracking session is live: the session's loot and ammo would
 * land in the same Inventory Changes export and be read as box contents.
 */
import type { MarketHarvestData, StrongboxOpeningEntry } from '$lib/api/commands.gen';
import {
	getEquipmentInventory,
	getLootInventory,
	getStrongboxOpenings,
	readStrongboxExport,
	recordStrongboxOpening,
	undoStrongboxOpening,
} from '$lib/api/inventory';
import { getMarketItemMarkups } from '$lib/api/market';
import type { ConfidenceMode } from '$lib/features/analytics/treeCuttingModel.svelte';
import { trackingSnapshot } from '$lib/stores/trackingStore.svelte';
import { todayDate } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';
import {
	type BoxHolding,
	balanceRefusal,
	boxHoldings,
	defaultPlan,
	exportRefusal,
	KEY_UNIT_TT,
	type ManualEntry,
	type ManualLine,
	manualRefusal,
	type OpeningResult,
	openingInput,
	planRefusal,
	resultFromExport,
	resultFromManual,
	type SourcePlan,
} from './openingDraft';

export type OpeningStage = 'choose' | 'open' | 'review' | 'done';
export type IntakeMode = 'export' | 'manual';

let nextLineId = 1;
function blankLine(): ManualLine {
	return { id: nextLineId++, itemName: '', quantity: null, ttValue: null };
}

function blankEntry(): ManualEntry {
	return { boxes: null, keyUnitTt: KEY_UNIT_TT, lines: [blankLine()] };
}

async function marketFor(names: string[]): Promise<MarketHarvestData | null> {
	if (names.length === 0) return null;
	return getMarketItemMarkups(names).catch(() => null);
}

export function createOpeningModel() {
	let stage = $state<OpeningStage>('choose');
	let holdings = $state<BoxHolding[]>([]);
	let openings = $state<StrongboxOpeningEntry[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let boxItem = $state<string | null>(null);
	let intake = $state<IntakeMode>('export');
	let reading = $state(false);
	let readError = $state<string | null>(null);
	let manual = $state<ManualEntry>(blankEntry());

	let result = $state<OpeningResult | null>(null);
	let plan = $state<SourcePlan>(defaultPlan(0, null));
	let openedAt = $state(todayDate());
	let market = $state<MarketHarvestData | null>(null);
	const mode: ConfidenceMode = 'liquidMiddling';

	let recording = $state(false);
	let recordError = $state<string | null>(null);
	let recorded = $state<StrongboxOpeningEntry | null>(null);
	let undoing = $state<string | null>(null);

	const tracking = $derived(trackingSnapshot.current?.status === 'active');
	const holding = $derived(
		holdings.find(
			(row) => row.name.toLocaleLowerCase() === (boxItem ?? '').trim().toLocaleLowerCase(),
		) ?? null,
	);

	async function refresh() {
		const [loot, assets, runs] = await Promise.all([
			getLootInventory(),
			getEquipmentInventory(),
			getStrongboxOpenings(),
		]);
		holdings = boxHoldings(loot, assets);
		openings = runs;
	}

	async function load() {
		loading = true;
		error = null;
		try {
			await Promise.all([refresh(), trackingSnapshot.hydrate()]);
		} catch (err) {
			error = describeError(err, 'Failed to read your strongboxes');
		} finally {
			loading = false;
		}
	}

	function adopt(next: OpeningResult) {
		result = next;
		plan = defaultPlan(next.boxes, holding);
		recordError = null;
		stage = 'review';
	}

	return {
		get stage() {
			return stage;
		},
		get holdings() {
			return holdings;
		},
		get openings() {
			return openings;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		get tracking() {
			return tracking;
		},
		get boxItem() {
			return boxItem;
		},
		get holding() {
			return holding;
		},
		get intake() {
			return intake;
		},
		set intake(value: IntakeMode) {
			intake = value;
			readError = null;
		},
		get reading() {
			return reading;
		},
		get readError() {
			return readError;
		},
		get manual() {
			return manual;
		},
		get manualRefusal() {
			return manualRefusal(manual);
		},
		get result() {
			return result;
		},
		get plan() {
			return plan;
		},
		set plan(value: SourcePlan) {
			plan = value;
		},
		get planRefusal() {
			return result ? planRefusal(plan, result.boxes) : null;
		},
		get balanceRefusal() {
			return result ? balanceRefusal(result) : null;
		},
		get openedAt() {
			return openedAt;
		},
		set openedAt(value: string) {
			openedAt = value;
		},
		get recording() {
			return recording;
		},
		get recordError() {
			return recordError;
		},
		get recorded() {
			return recorded;
		},
		get undoing() {
			return undoing;
		},
		load,
		/** Follow the live session, so the gate lifts the moment tracking stops. */
		subscribe: () => trackingSnapshot.subscribe(),

		choose(name: string) {
			boxItem = name.trim() || null;
		},
		begin() {
			if (!boxItem || tracking) return;
			readError = null;
			manual = blankEntry();
			stage = 'open';
		},
		back() {
			if (stage === 'review') stage = 'open';
			else if (stage === 'open') stage = 'choose';
		},

		async readExport(text: string) {
			if (!boxItem || reading) return;
			reading = true;
			readError = null;
			try {
				const read = await readStrongboxExport({ text, boxItem });
				const refusal = exportRefusal(read, boxItem);
				if (refusal) {
					readError = refusal;
					return;
				}
				market = await marketFor(read.contents.map((row) => row.itemName));
				adopt(resultFromExport(read, boxItem, { market, mode }));
			} catch (err) {
				readError = describeError(err, 'Failed to read the export');
			} finally {
				reading = false;
			}
		},

		addManualLine() {
			manual.lines.push(blankLine());
		},
		removeManualLine(id: number) {
			manual.lines = manual.lines.filter((line) => line.id !== id);
			if (manual.lines.length === 0) manual.lines.push(blankLine());
		},
		async reviewManual() {
			if (!boxItem || manualRefusal(manual)) return;
			reading = true;
			try {
				market = await marketFor(manual.lines.map((line) => line.itemName.trim()).filter(Boolean));
				adopt(resultFromManual($state.snapshot(manual), boxItem, { market, mode }));
			} finally {
				reading = false;
			}
		},

		setRecycle(itemName: string, recycle: boolean) {
			if (!result) return;
			result.lines = result.lines.map((line) =>
				line.itemName === itemName ? { ...line, recycle } : line,
			);
		},
		/** Keep every item, or cube everything the market says sells as cubes. */
		applyDefaults(kind: 'keep' | 'suggested') {
			if (!result) return;
			result.lines = result.lines.map((line) => ({
				...line,
				recycle: kind === 'suggested' ? line.estimate.sellsAsCubes : false,
			}));
		},

		async record() {
			if (!result || recording) return;
			if (balanceRefusal(result) || planRefusal(plan, result.boxes)) return;
			recording = true;
			recordError = null;
			try {
				const { id } = await recordStrongboxOpening(
					openingInput($state.snapshot(result), $state.snapshot(plan), openedAt),
				);
				await refresh();
				recorded = openings.find((run) => run.id === id) ?? null;
				stage = 'done';
			} catch (err) {
				recordError = describeError(err, 'Failed to record the opening');
			} finally {
				recording = false;
			}
		},

		/** Back to the start, for another box type or another sitting. */
		startOver() {
			stage = 'choose';
			result = null;
			recorded = null;
			readError = null;
			recordError = null;
			manual = blankEntry();
			openedAt = todayDate();
		},

		async undo(id: string) {
			if (undoing) return;
			undoing = id;
			error = null;
			try {
				await undoStrongboxOpening({ id });
				await refresh();
				if (recorded?.id === id) recorded = openings.find((run) => run.id === id) ?? null;
			} catch (err) {
				error = describeError(err, 'Failed to undo the opening');
			} finally {
				undoing = null;
			}
		},
	};
}

export type OpeningModel = ReturnType<typeof createOpeningModel>;
