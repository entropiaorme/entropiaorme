/**
 * The drafts captures of the sell queue offer, and what the player has done
 * with each.
 *
 * A capture that read the queue replaces whatever an earlier one offered:
 * drafts from two captures never stand side by side. A capture that read
 * nothing leaves the drafts already offered in place (each is dated to the
 * moment its own screen was read) and says why beside them. Captures are
 * ordered by the identity each carries from when it was started, so an older
 * one answering late, from this form's button or from the floating one, never
 * replaces a newer one. Closing the form drops whatever is still out.
 *
 * The game keeps a queued row until its own list button is pressed, so a
 * recapture brings back rows already recorded here. Every row recorded is
 * remembered, by its name and every figure as printed, for as long as this
 * model lives: a row a later capture brings back matching one of them comes
 * back recorded, and cannot be recorded again. Two identical rows queued and
 * one recorded bring back one recorded and one to review.
 *
 * Each draft is accepted or discarded on its own, and accepting one names
 * the draft it accepts. Saving is a phase: while one draft is being saved,
 * every other action is refused at once with a reason, and a capture arriving
 * meanwhile waits. It is applied once the save has succeeded, or, after a
 * failed save, at the player's next action, so the failure is never left
 * standing over a draft it is not about.
 */

import type {
	QueuedCount,
	QueuedListingDraft,
	SaleQueueCapture,
	SaleQueueRefusal,
	UnreadQueuedItem,
} from '$lib/api/commands.gen';
import { type QueueRead, rowKey } from './saleQueueCapture';

export type DraftState = 'pending' | 'saving' | 'recorded' | 'discarded';

export interface DraftEntry {
	draft: QueuedListingDraft;
	state: DraftState;
}

export interface SaleQueueSource {
	/** One capture, taken now for this form. */
	capture: () => Promise<SaleQueueCapture>;
	/** A capture the floating button took for this form, or null. Collected
	 * once. */
	collect: () => Promise<SaleQueueCapture | null>;
}

const SAVING = 'A listing is being saved; wait for it to finish';
const EARLIER = 'That draft is from an earlier capture';
const RECORDED = 'That draft is already recorded';

export function createSaleQueueDrafts(source: SaleQueueSource) {
	/** The newest capture applied: nothing older replaces it. */
	let applied = 0;
	/** Bumped when the form closes, so what is still out is dropped. */
	let epoch = 0;
	/** Bumped per capture this form starts, so only the last one lands. */
	let request = 0;
	/** A capture that arrived while a draft was being saved, or after a save
	 * failed, waiting for the player's next action. */
	let deferred: SaleQueueCapture | null = null;
	/** Every row recorded while this model lives, by its key, counted. */
	const recorded = new Map<string, number>();

	let capturing = $state(false);
	let failure = $state<string | null>(null);
	let refusal = $state<SaleQueueRefusal | null>(null);
	let read = $state<QueueRead | null>(null);
	let entries = $state<DraftEntry[]>([]);
	let selectedId = $state<string | null>(null);

	const saving = $derived(entries.some((entry) => entry.state === 'saving'));
	const visible = $derived(entries.filter((entry) => entry.state !== 'discarded'));
	const selected = $derived(entries.find((entry) => entry.draft.draftId === selectedId) ?? null);

	function find(id: string): DraftEntry | undefined {
		return entries.find((entry) => entry.draft.draftId === id);
	}

	function firstPending(): string | null {
		return entries.find((entry) => entry.state === 'pending')?.draft.draftId ?? null;
	}

	/** Apply a capture newer than any applied yet; an older one is ignored. */
	function apply(capture: SaleQueueCapture): void {
		if (capture.captureId <= applied) return;
		applied = capture.captureId;
		failure = null;
		if (capture.kind === 'refused') {
			refusal = capture.reason;
			return;
		}
		refusal = null;
		read = capture;
		const left = new Map(recorded);
		entries = capture.drafts.map((draft) => {
			const key = rowKey(draft);
			const count = left.get(key) ?? 0;
			if (count === 0) return { draft, state: 'pending' as const };
			left.set(key, count - 1);
			return { draft, state: 'recorded' as const };
		});
		selectedId = firstPending();
	}

	/** Hold a capture until no save is in flight or failed. */
	function defer(capture: SaleQueueCapture): void {
		if (!deferred || deferred.captureId < capture.captureId) deferred = capture;
	}

	function settleDeferred(): void {
		const waiting = deferred;
		deferred = null;
		if (waiting) apply(waiting);
	}

	return {
		get capturing() {
			return capturing;
		},
		/** The capture itself failed to answer (not a refusal from the reader). */
		get failure() {
			return failure;
		},
		/** Why the newest capture read no queue. */
		get refusal() {
			return refusal;
		},
		/** The newest capture that read the queue. */
		get read() {
			return read;
		},
		get queued(): QueuedCount | null {
			return read?.queued ?? null;
		},
		get unread(): UnreadQueuedItem[] {
			return read?.unread ?? [];
		},
		/** The drafts still shown: pending, being saved, or recorded. */
		get entries() {
			return visible;
		},
		get selectedId() {
			return selectedId;
		},
		get selected() {
			return selected;
		},
		get saving() {
			return saving;
		},
		get hasPending() {
			return entries.some((entry) => entry.state === 'pending');
		},

		/** Capture the sell queue for this form. Refused while saving. */
		async capture(): Promise<string | null> {
			if (saving) return SAVING;
			if (capturing) return null;
			settleDeferred();
			const mine = ++request;
			const at = epoch;
			capturing = true;
			failure = null;
			try {
				const capture = await source.capture();
				if (at !== epoch || mine !== request) return null;
				if (saving) defer(capture);
				else apply(capture);
			} catch (cause) {
				if (at === epoch && mine === request) {
					failure = cause instanceof Error ? cause.message : 'Could not read the sell queue';
				}
			} finally {
				if (mine === request) capturing = false;
			}
			return null;
		},

		/** Collect a capture the floating button took, if one is waiting. On
		 * opening the form, a refusal waiting from before is stale and is not
		 * shown. */
		async collect(opening = false): Promise<void> {
			const at = epoch;
			let capture: SaleQueueCapture | null;
			try {
				capture = await source.collect();
			} catch {
				// Nothing could be collected; the form stands as it is.
				return;
			}
			if (!capture || at !== epoch) return;
			if (opening && capture.kind === 'refused') return;
			if (saving || deferred) defer(capture);
			else apply(capture);
		},

		/** Review a draft in the form, or `null` to enter a listing by hand. */
		select(id: string | null): string | null {
			if (saving) return SAVING;
			settleDeferred();
			if (id === null) {
				selectedId = null;
				return null;
			}
			const entry = find(id);
			if (!entry) return EARLIER;
			if (entry.state !== 'pending') return RECORDED;
			selectedId = id;
			return null;
		},

		/** Review the next draft still to be dealt with, if there is one. */
		next(): void {
			if (saving) return;
			settleDeferred();
			selectedId = firstPending();
		},

		/** Put a draft aside without recording it. */
		discard(id: string): string | null {
			if (saving) return SAVING;
			settleDeferred();
			const entry = find(id);
			if (!entry) return EARLIER;
			if (entry.state !== 'pending') return RECORDED;
			entry.state = 'discarded';
			if (selectedId === id) selectedId = firstPending();
			return null;
		},

		/** Record draft `id` by `save`. Answers why it was refused, or null
		 * once saved; a failed save is thrown, and the draft stays to try
		 * again. */
		async accept(id: string, save: () => Promise<void>): Promise<string | null> {
			if (saving) return SAVING;
			const entry = find(id);
			if (!entry) return EARLIER;
			if (entry.state !== 'pending') return RECORDED;
			entry.state = 'saving';
			try {
				await save();
			} catch (cause) {
				entry.state = 'pending';
				throw cause;
			}
			entry.state = 'recorded';
			const key = rowKey(entry.draft);
			recorded.set(key, (recorded.get(key) ?? 0) + 1);
			if (selectedId === id) selectedId = firstPending();
			settleDeferred();
			return null;
		},

		/** The form closed: drop every draft and whatever is still out. What
		 * was recorded is remembered. */
		reset(): void {
			epoch++;
			request++;
			deferred = null;
			capturing = false;
			failure = null;
			refusal = null;
			read = null;
			entries = [];
			selectedId = null;
		},
	};
}

export type SaleQueueDrafts = ReturnType<typeof createSaleQueueDrafts>;
