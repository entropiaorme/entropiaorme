/**
 * Runes-native state for recording one armour cost from the overlay's Cost
 * popup: which stream is being recorded, the amount read or typed, the
 * sessions it will be spread over, and the confirmation.
 *
 * Unlimited protection is one pooled repair: the amount is the Repair
 * Terminal total. A limited set is measured by its Trade Terminal value; the
 * first reading is a baseline, a later lower one books the TT lost at the
 * set's markup, and a higher one needs a reason to reset the baseline.
 */

import {
	confirmProtectionObservation,
	confirmProtectionRepair,
	getProtectionOverview,
	getProtectionRecordingCandidates,
	type ProtectionRecordingCandidates,
	type ProtectionSet,
	type ProtectionStream,
	scanRepairCost,
	scanTradeTerminalValue,
} from '$lib/api';
import { describeError } from '$lib/view/errorState';
import {
	chosenSessions,
	EMPTY_SELECTION,
	groupBySessionType,
	lookBack,
	parseAmount,
	type RecordingSelection,
	recordingToken,
	type SessionGroup,
	startAt,
	toggleEarlier,
	toggleGroup,
	toggleSession,
	totalHits,
} from './armourRecording';

export type RecordingPhase = 'ready' | 'scanning' | 'review' | 'saved';

/** What a confirmed recording did, in the player's terms. */
export type RecordingResult =
	| { kind: 'spread'; costPed: number; sessions: number }
	| { kind: 'unattributed'; costPed: number }
	| { kind: 'baseline'; ttValuePed: number }
	| { kind: 'reset'; ttValuePed: number };

export interface ArmourRecordingOptions {
	/** Whether the Repair Terminal reader is switched on. */
	repairOcrEnabled: () => boolean;
	/** Whether limited sets are offered (an in-development surface). */
	limitedEnabled: () => boolean;
}

const INCREASE_TOLERANCE = 0.0000001;

function sameStream(a: ProtectionStream, b: ProtectionStream): boolean {
	if (a.kind === 'unlimited' || b.kind === 'unlimited') return a.kind === b.kind;
	return a.setId === b.setId;
}

export function createArmourRecording(options: ArmourRecordingOptions) {
	let sets = $state<ProtectionSet[]>([]);
	let stream = $state<ProtectionStream>({ kind: 'unlimited' });
	let phase = $state<RecordingPhase>('ready');
	let value = $state('');
	let source = $state<'ocr' | 'manual'>('manual');
	let rawText = $state<string | null>(null);
	let calibrated = $state(true);
	let resetReason = $state('');
	let notice = $state<string | null>(null);
	let saving = $state(false);
	let candidates = $state<ProtectionRecordingCandidates | null>(null);
	let candidatesLoading = $state(false);
	let selection = $state<RecordingSelection>(EMPTY_SELECTION);
	let earlierOpen = $state(false);
	let result = $state<RecordingResult | null>(null);
	let token = recordingToken();
	// Each stream change starts a fresh candidate read; a slower read for a
	// stream the player has already left must not land over the current one.
	let candidatesRequest = 0;

	const set = $derived.by(() => {
		const current = stream;
		if (current.kind !== 'limited') return null;
		return sets.find((entry) => Number(entry.id) === current.setId) ?? null;
	});
	const amount = $derived(parseAmount(value));
	const baselineTt = $derived(
		stream.kind === 'limited'
			? (candidates?.baselineTtPed ?? set?.latestObservation?.ttValuePed ?? null)
			: null,
	);
	const isBaseline = $derived(stream.kind === 'limited' && baselineTt === null);
	const increased = $derived(
		baselineTt !== null && amount !== null && amount > baselineTt + INCREASE_TOLERANCE,
	);
	/** The cost this recording will book, before it is confirmed. */
	const costPed = $derived.by((): number | null => {
		if (amount === null) return null;
		if (stream.kind === 'unlimited') return amount;
		if (baselineTt === null || increased || !set) return null;
		return ((baselineTt - amount) * set.markupPercent) / 100;
	});
	const sessions = $derived(candidates?.sessions ?? []);
	const earlier = $derived(candidates?.earlier ?? []);
	const chosen = $derived(chosenSessions(sessions, earlier, selection));
	const chosenIds = $derived(new Set(chosen.map((session) => session.sessionId)));
	const groups = $derived<SessionGroup[]>(groupBySessionType(lookBack(sessions, selection)));
	const earlierGroups = $derived<SessionGroup[]>(groupBySessionType(earlier));
	const hits = $derived(totalHits(chosen));
	/** Whether this recording spreads over sessions at all. */
	const spreads = $derived(!isBaseline && !increased);
	/** The picker only earns its space when there is a choice to make. */
	const pickerVisible = $derived(spreads && (sessions.length > 1 || earlier.length > 0));
	const canConfirm = $derived(
		phase === 'review' &&
			amount !== null &&
			!saving &&
			!candidatesLoading &&
			(!increased || resetReason.trim().length > 0) &&
			(stream.kind === 'unlimited' || set !== null),
	);

	function resetEntry(): void {
		phase = 'ready';
		value = '';
		source = 'manual';
		rawText = null;
		calibrated = true;
		resetReason = '';
		notice = null;
		result = null;
		token = recordingToken();
	}

	async function loadCandidates(): Promise<void> {
		const request = ++candidatesRequest;
		candidatesLoading = true;
		try {
			const next = await getProtectionRecordingCandidates(stream);
			if (request !== candidatesRequest) return;
			candidates = next;
			selection = EMPTY_SELECTION;
			earlierOpen = false;
		} catch (cause) {
			if (request !== candidatesRequest) return;
			candidates = null;
			notice = describeError(cause, 'The sessions to record against could not be read');
		} finally {
			if (request === candidatesRequest) candidatesLoading = false;
		}
	}

	async function loadSets(): Promise<void> {
		if (!options.limitedEnabled()) {
			sets = [];
			return;
		}
		try {
			sets = (await getProtectionOverview()).sets;
		} catch (cause) {
			notice = describeError(cause, 'Limited armour sets could not be read');
		}
		if (stream.kind === 'limited' && !set) stream = { kind: 'unlimited' };
	}

	async function load(): Promise<void> {
		resetEntry();
		await loadSets();
		await loadCandidates();
	}

	async function chooseStream(next: ProtectionStream): Promise<void> {
		if (saving || sameStream(stream, next)) return;
		stream = next;
		resetEntry();
		candidates = null;
		await loadCandidates();
	}

	async function scan(): Promise<void> {
		phase = 'scanning';
		notice = null;
		try {
			if (stream.kind === 'limited') {
				const read = await scanTradeTerminalValue();
				calibrated = read.calibrated;
				rawText = read.rawText;
				if (read.error || read.valuePed === null) {
					notice = read.error ?? 'No number was recognised';
					source = 'manual';
				} else {
					value = read.valuePed.toFixed(2);
					source = 'ocr';
				}
			} else {
				// The repair reader reads the terminal, not a session; the id it
				// takes is unused, so none is named here.
				const read = await scanRepairCost('');
				rawText = read.raw_text ?? null;
				if (read.error || read.cost_ped == null) {
					notice = read.error ?? 'No number was recognised';
					source = 'manual';
				} else {
					value = read.cost_ped.toFixed(2);
					source = 'ocr';
				}
			}
		} catch {
			notice = `${stream.kind === 'limited' ? 'Trade Terminal' : 'Repair Terminal'} scan failed`;
			source = 'manual';
		}
		phase = 'review';
	}

	function enterManually(): void {
		source = 'manual';
		rawText = null;
		notice = null;
		phase = 'review';
	}

	async function confirm(): Promise<void> {
		if (!canConfirm || amount === null) return;
		saving = true;
		notice = null;
		const sessionIds = spreads ? chosen.map((session) => session.sessionId) : [];
		try {
			if (stream.kind === 'unlimited') {
				const outcome = await confirmProtectionRepair({
					clientToken: token,
					costPed: amount,
					sessionIds,
				});
				const window = outcome.costWindow;
				result =
					window.allocations.length > 0
						? { kind: 'spread', costPed: window.costPed, sessions: window.allocations.length }
						: { kind: 'unattributed', costPed: window.costPed };
			} else {
				const outcome = await confirmProtectionObservation({
					setId: stream.setId,
					clientToken: token,
					ttValuePed: amount,
					source,
					rawText,
					resetReason: increased ? resetReason.trim() : null,
					sessionIds,
				});
				const window = outcome.costWindow;
				if (window) {
					result =
						window.allocations.length > 0
							? { kind: 'spread', costPed: window.costPed, sessions: window.allocations.length }
							: { kind: 'unattributed', costPed: window.costPed };
				} else {
					result = { kind: increased ? 'reset' : 'baseline', ttValuePed: amount };
				}
				await loadSets();
			}
			phase = 'saved';
		} catch (cause) {
			notice = describeError(cause, 'The armour cost could not be recorded');
		} finally {
			saving = false;
		}
	}

	return {
		get sets() {
			return sets;
		},
		get stream() {
			return stream;
		},
		get set() {
			return set;
		},
		get phase() {
			return phase;
		},
		get value() {
			return value;
		},
		set value(next: string) {
			value = next;
		},
		get resetReason() {
			return resetReason;
		},
		set resetReason(next: string) {
			resetReason = next;
		},
		get rawText() {
			return rawText;
		},
		get calibrated() {
			return calibrated;
		},
		get notice() {
			return notice;
		},
		get saving() {
			return saving;
		},
		get candidates() {
			return candidates;
		},
		get candidatesLoading() {
			return candidatesLoading;
		},
		get selection() {
			return selection;
		},
		get sessions() {
			return sessions;
		},
		get earlier() {
			return earlier;
		},
		get groups() {
			return groups;
		},
		get earlierGroups() {
			return earlierGroups;
		},
		get chosen() {
			return chosen;
		},
		get chosenIds() {
			return chosenIds;
		},
		get hits() {
			return hits;
		},
		get baselineTt() {
			return baselineTt;
		},
		get isBaseline() {
			return isBaseline;
		},
		get increased() {
			return increased;
		},
		get costPed() {
			return costPed;
		},
		get spreads() {
			return spreads;
		},
		get pickerVisible() {
			return pickerVisible;
		},
		get earlierOpen() {
			return earlierOpen;
		},
		get canConfirm() {
			return canConfirm;
		},
		get result() {
			return result;
		},
		get repairOcrEnabled() {
			return options.repairOcrEnabled();
		},
		load,
		chooseStream,
		scan,
		enterManually,
		confirm,
		toggleSession(sessionId: string) {
			selection = toggleSession(selection, sessionId);
		},
		toggleGroup(group: SessionGroup) {
			selection = toggleGroup(selection, group, chosenIds);
		},
		toggleEarlier(sessionId: string) {
			selection = toggleEarlier(selection, sessionId);
		},
		/** Start the look-back at the session with this id. */
		startFrom(sessionId: string) {
			const index = sessions.findIndex((session) => session.sessionId === sessionId);
			if (index >= 0) selection = startAt(selection, index);
		},
		/** Show or hide the earlier sessions. Hiding them drops any that were
		 * ticked back in, so nothing hidden is still being recorded against. */
		toggleEarlierOpen() {
			earlierOpen = !earlierOpen;
			if (!earlierOpen) selection = { ...selection, reincluded: new Set() };
		},
	};
}

export type ArmourRecording = ReturnType<typeof createArmourRecording>;
