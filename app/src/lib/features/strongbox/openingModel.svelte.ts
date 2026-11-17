/**
 * The strongbox opening view's model: choose a box, open it in game, review
 * what came out, and record the run.
 *
 * Opening is a deliberate moment of its own, so the view will not begin a
 * run while a tracking session is live: the session's loot and ammo would
 * land in the same Inventory Changes export and be read as box contents.
 *
 * When the account is connected, the result can come from the account
 * itself: a run is read when it begins, every minute while the boxes are
 * opened, and once more when the player says it is done; the opening the
 * readings propose fills the review exactly as a pasted export would.
 * Openings the readings found on their own (during a tracked session, or
 * a run nobody reviewed) are offered at the start, to review or dismiss. A
 * proposal is never recorded here on its own: the player records the
 * reviewed opening through the usual command. Recording claims the
 * proposals first, records the opening, then links them to it as one; a
 * failed recording releases them. A claim left with no link (the app closed
 * between the two, or the link failed) is kept by the backend and shown as
 * possibly recorded already, so nothing here holds that state in memory.
 */
import {
	ACCOUNT_TOPIC,
	beginAccountRun,
	cancelAccountRun,
	claimProposals,
	dismissProposal,
	finishAccountRun,
	getProposals,
	readRunWhole,
	linkProposals,
	releaseProposals,
} from '$lib/api/account';
import type {
	AccountProposal,
	AccountRunReading,
	MarketHarvestData,
	StrongboxOpeningEntry,
} from '$lib/api/commands.gen';
import {
	getEquipmentInventory,
	getLootInventory,
	getStrongboxOpenings,
	readStrongboxExport,
	recordStrongboxOpening,
	undoStrongboxOpening,
} from '$lib/api/inventory';
import { getMarketItemMarkups } from '$lib/api/market';
import { createAccountStatus } from '$lib/features/account/accountStatus.svelte';
import type { ConfidenceMode } from '$lib/features/analytics/treeCuttingModel.svelte';
import { createSnapshotStore } from '$lib/realtime/snapshotStore.svelte';
import { trackingSnapshot } from '$lib/stores/trackingStore.svelte';
import { localDateOf, todayDate } from '$lib/utils/format';
import { describeError } from '$lib/view/errorState';
import { runProposals } from './detectedOpenings';
import {
	type BoxHolding,
	balanceRefusal,
	boxHoldings,
	defaultPlan,
	exportRefusal,
	isOpeningProposal,
	KEY_UNIT_TT,
	type ManualEntry,
	type ManualLine,
	manualRefusal,
	type OpeningProposal,
	type OpeningResult,
	openingInput,
	planRefusal,
	proposalRefusal,
	proposedBox,
	resultFromExport,
	resultFromManual,
	resultFromProposals,
	type SourcePlan,
	withPossible,
} from './openingDraft';

export type OpeningStage = 'choose' | 'open' | 'review' | 'done';
export type IntakeMode = 'account' | 'export' | 'manual';

/** Where reading a run from the account stands. */
export type RunState =
	| { phase: 'idle' }
	/** The reading from before the boxes is being taken. */
	| { phase: 'starting' }
	/** Open the boxes, then say so; `from` is the reading before them. */
	| { phase: 'open'; from: number }
	/** The reading after the boxes is being taken. */
	| { phase: 'reading'; from: number }
	/** The readings show no opening in the run yet: the next reading may. */
	| { phase: 'nothing'; from: number }
	| { phase: 'failed'; message: string };

const RUN_FAILED: Record<Exclude<AccountRunReading['outcome'], 'read'>, string> = {
	notConnected: 'Your account is not connected.',
	signedOut: 'The account site has signed you out. Sign in again in Settings.',
	unreadable: 'Your account could not be read just now.',
	interrupted: 'Reading your account was stopped.',
};

/** How many times linking proposals to their opening is tried at once. */
const LINK_ATTEMPTS = 2;

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

function proposedNames(proposals: OpeningProposal[]): string[] {
	return proposals.flatMap((proposal) =>
		[...proposal.outcome.contents, ...proposal.outcome.possibleContents].map(
			(item) => item.item.name,
		),
	);
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

	// ── The account ─────────────────────────────────────────────────
	const account = createAccountStatus();
	const proposals = createSnapshotStore<AccountProposal[]>(ACCOUNT_TOPIC, () =>
		getProposals('opening'),
	);
	let run = $state<RunState>({ phase: 'idle' });
	/** A result read from a run: the reading before its boxes, and how many
	 * of the box the latest reading still holds, so a later reading can be
	 * asked for while any are left. */
	let runFollowUp = $state<{ from: number; boxesLeft: number } | null>(null);
	/** Moves on whenever the run being read is left, so a reading that
	 * answers after the player moved on is dropped. */
	let runGeneration = 0;
	/** The proposals the review was filled from, and where it was opened. */
	let reviewing = $state<number[]>([]);
	let reviewFrom = $state<'choose' | 'open'>('open');
	/** Of those, the ones shown as possibly recorded already when the
	 * player chose to review them anyway: their old claim is let go. */
	let reviewingClaimed = $state<number[]>([]);
	let proposalError = $state<string | null>(null);
	let dismissing = $state<number | null>(null);

	const tracking = $derived(trackingSnapshot.current?.status === 'active');
	const accountConnected = $derived(account.current?.state === 'connected');
	const detected = $derived((proposals.current ?? []).filter(isOpeningProposal));
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

	/** Link proposals to the opening they became, as one; true when they
	 * are. A link that fails leaves them claimed, shown as possibly
	 * recorded. */
	async function link(ids: number[], openingId: string): Promise<boolean> {
		for (let attempt = 0; attempt < LINK_ATTEMPTS; attempt += 1) {
			try {
				await linkProposals({ ids, recordId: openingId });
				return true;
			} catch {
				// Tried once more; then the claim stands.
			}
		}
		return false;
	}

	async function load() {
		loading = true;
		error = null;
		try {
			await Promise.all([
				refresh(),
				trackingSnapshot.hydrate(),
				proposals.hydrate(),
				account.refresh(),
			]);
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

	function adoptProposals(found: OpeningProposal[], box: string, from: 'choose' | 'open') {
		boxItem = box;
		reviewing = found.map((proposal) => proposal.id);
		reviewingClaimed = found
			.filter((proposal) => proposal.possiblyRecorded)
			.map((proposal) => proposal.id);
		reviewFrom = from;
		adopt(resultFromProposals(found, box, { market, mode }));
		openedAt = localDateOf(found[found.length - 1].between.to);
	}

	/** Take the run's next reading and read the run whole, from the reading
	 * before its boxes to that one, then review what it holds: every
	 * arrival a content, the ammo as the Universal Ammo gained. */
	async function readRun(from: number, box: string) {
		runGeneration += 1;
		const generation = runGeneration;
		run = { phase: 'reading', from };
		try {
			const answer = await finishAccountRun();
			if (generation !== runGeneration) return;
			if (answer.outcome !== 'read') {
				run = { phase: 'failed', message: RUN_FAILED[answer.outcome] };
				return;
			}
			const whole = await readRunWhole({
				fromReadingId: from,
				toReadingId: answer.readingId,
				boxItem: box,
			});
			const found = runProposals(await getProposals('opening'), from, answer.readingId);
			if (generation !== runGeneration) return;
			if (found.length === 0) {
				run = { phase: 'nothing', from };
				return;
			}
			const refusal = proposalRefusal(found, box);
			if (refusal) {
				run = { phase: 'failed', message: refusal };
				return;
			}
			market = await marketFor(proposedNames(found));
			if (generation !== runGeneration) return;
			run = { phase: 'idle' };
			runFollowUp = { from, boxesLeft: whole.boxesLeft };
			adoptProposals(found, box, 'open');
		} catch (err) {
			if (generation !== runGeneration) return;
			run = { phase: 'failed', message: describeError(err, 'Could not read your account') };
		}
	}

	/** Stop reading for the run: the view, or the way the result comes in,
	 * has moved on. */
	/** Stop reading for the run; resolves once the backend has been told. */
	function closeRun(): Promise<void> {
		runGeneration += 1;
		runFollowUp = null;
		// Whatever state the run is in, failed included: closing a run the
		// backend already closed changes nothing.
		const cancelled =
			run.phase !== 'idle' ? cancelAccountRun().catch(() => undefined) : Promise.resolve();
		run = { phase: 'idle' };
		return cancelled;
	}

	async function startRun() {
		const cancelled = closeRun();
		const generation = runGeneration;
		run = { phase: 'starting' };
		// The run before this one is closed first, so its cancel can never
		// land on the run begun here.
		await cancelled;
		if (generation !== runGeneration) return;
		try {
			const answer = await beginAccountRun();
			if (generation !== runGeneration) return;
			run =
				answer.outcome === 'read'
					? { phase: 'open', from: answer.readingId }
					: { phase: 'failed', message: RUN_FAILED[answer.outcome] };
		} catch (err) {
			if (generation !== runGeneration) return;
			run = { phase: 'failed', message: describeError(err, 'Could not read your account') };
		}
	}

	function setIntake(value: IntakeMode) {
		if (value === intake) return;
		intake = value;
		readError = null;
		if (value === 'account') void startRun();
		else void closeRun();
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
			setIntake(value);
		},
		get accountConnected() {
			return accountConnected;
		},
		get run() {
			return run;
		},
		/** Openings the readings found that wait to be reviewed. */
		get detected() {
			return detected;
		},
		get proposalError() {
			return proposalError;
		},
		get dismissing() {
			return dismissing;
		},
		/** The review was filled from the account's readings. */
		get fromAccount() {
			return reviewing.length > 0;
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
		/** Follow the live session, so the gate lifts the moment tracking
		 * stops, and the account, so what it proposes stays current. */
		async subscribe() {
			const detachAccount = account.connect();
			const [stopTracking, stopProposals] = await Promise.all([
				trackingSnapshot.subscribe(),
				proposals.subscribe(),
			]);
			return () => {
				detachAccount();
				stopTracking();
				stopProposals();
			};
		},
		/** The view is going: stop reading for its run. */
		dispose() {
			void closeRun();
		},

		choose(name: string) {
			boxItem = name.trim() || null;
		},
		begin() {
			if (!boxItem || tracking) return;
			readError = null;
			manual = blankEntry();
			reviewing = [];
			stage = 'open';
			if (accountConnected) {
				intake = 'account';
				void startRun();
			} else if (intake === 'account') {
				intake = 'export';
			}
		},
		back() {
			if (stage === 'review') {
				const from = reviewFrom;
				reviewing = [];
				reviewFrom = 'open';
				if (from === 'choose') {
					stage = 'choose';
					return;
				}
				stage = 'open';
				if (intake === 'account') void startRun();
			} else if (stage === 'open') {
				void closeRun();
				stage = 'choose';
			}
		},

		/** Read the account afresh for the run, as when the stage opened. */
		readAccountAgain(): Promise<void> {
			if (stage === 'open' && intake === 'account') return startRun();
			return Promise.resolve();
		},
		/** The boxes are open: read the account once more and review what
		 * the readings found in the run. */
		async finishRun() {
			if (run.phase !== 'open' || !boxItem) return;
			await readRun(run.from, boxItem);
		},

		/** The site may not show every box opened yet: read the account once
		 * more and read the whole run again, from the reading before its
		 * boxes. The result replaces the one shown. */
		async checkNextReading() {
			const from = run.phase === 'nothing' ? run.from : runFollowUp?.from;
			if (from === undefined || !boxItem) return;
			await readRun(from, boxItem);
		},

		/** How many of the box the run's latest reading still holds, when the
		 * result shown was read from a run. */
		get runFollowUp() {
			return runFollowUp;
		},

		/** Review an opening the readings found, as a pasted export. */
		async reviewProposal(id: number) {
			const proposal = detected.find((candidate) => candidate.id === id);
			if (!proposal || reading) return;
			proposalError = null;
			const box = proposedBox(proposal);
			if (box === null) {
				proposalError = proposalRefusal([proposal], '');
				return;
			}
			void closeRun();
			const generation = runGeneration;
			reading = true;
			try {
				const found = await marketFor(proposedNames([proposal]));
				// The player moved on meanwhile: drop it.
				if (generation !== runGeneration || stage !== 'choose') return;
				market = found;
				adoptProposals([proposal], box, 'choose');
			} finally {
				reading = false;
			}
		},

		async dismissProposal(id: number) {
			if (dismissing !== null) return;
			dismissing = id;
			proposalError = null;
			try {
				await dismissProposal({ id });
				await proposals.hydrate();
			} catch (err) {
				proposalError = describeError(err, 'Failed to dismiss it');
			} finally {
				dismissing = null;
			}
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
				reviewing = [];
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
				reviewing = [];
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
			result.possible = result.possible.map((line) =>
				line.itemName === itemName ? { ...line, recycle } : line,
			);
		},
		/** Add an item that may be the boxes' to the run, or leave it out. */
		setPossible(itemName: string, included: boolean) {
			if (!result) return;
			result = withPossible($state.snapshot(result) as OpeningResult, itemName, included);
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
			const from = $state.snapshot(reviewing);
			// Claims standing before this attempt: shown as possibly recorded
			// and reviewed anyway, or left claimed by an attempt that failed
			// to release them. They are taken over in the same claim, and
			// never released by this attempt.
			const stale = $state.snapshot(reviewingClaimed).filter((id) => from.includes(id));
			const fresh = from.filter((id) => !stale.includes(id));
			try {
				if (from.length > 0) await claimProposals({ ids: from, retake: stale });
			} catch (err) {
				recordError = describeError(err, 'Could not set the opening aside to record');
				recording = false;
				void proposals.hydrate();
				return;
			}
			let id: string;
			try {
				({ id } = await recordStrongboxOpening(
					openingInput($state.snapshot(result), $state.snapshot(plan), openedAt),
				));
			} catch (err) {
				recordError = describeError(err, 'Failed to record the opening');
				// Not recorded: release what this attempt claimed. If that
				// fails, those stay claimed too, and the next attempt takes
				// them over rather than being refused.
				if (fresh.length > 0) {
					try {
						await releaseProposals({ ids: fresh });
					} catch {
						reviewingClaimed = [...stale, ...fresh];
					}
				}
				recording = false;
				void proposals.hydrate();
				return;
			}
			// Recorded: whatever follows, it is never recorded again.
			reviewing = [];
			reviewingClaimed = [];
			stage = 'done';
			if (from.length > 0) await link(from, id);
			await refresh().catch(() => undefined);
			recorded = openings.find((entry) => entry.id === id) ?? null;
			void proposals.hydrate();
			recording = false;
		},

		/** Back to the start, for another box type or another sitting. */
		startOver() {
			void closeRun();
			stage = 'choose';
			result = null;
			recorded = null;
			reviewing = [];
			readError = null;
			recordError = null;
			manual = blankEntry();
			openedAt = todayDate();
			void proposals.hydrate();
		},

		async undo(id: string) {
			if (undoing) return;
			undoing = id;
			error = null;
			try {
				await undoStrongboxOpening({ id });
				await refresh();
				if (recorded?.id === id) recorded = openings.find((entry) => entry.id === id) ?? null;
				void proposals.hydrate();
			} catch (err) {
				error = describeError(err, 'Failed to undo the opening');
			} finally {
				undoing = null;
			}
		},
	};
}

export type OpeningModel = ReturnType<typeof createOpeningModel>;
