/**
 * Transient notices for the tracking overlay: the one channel through which
 * the strip reports that something happened. A session raising a tracking
 * warning, an overlay action failing, and the backend refusing a start are
 * all events, not standing state, so each shows once, in full, and clears
 * itself.
 *
 * Standing conditions do not belong here. The harvest guardrail, for
 * instance, keeps its own persistent cue on the strip (the questioned tool
 * in red, the recorded tool beneath) for as long as the disagreement
 * stands; the notice only announces that it began.
 *
 * Tracking warnings arrive as the session's cumulative list on every
 * frame, so they are observed rather than pushed: each distinct message
 * notifies once per session, and a new session starts with a clean slate.
 * A feature model's error channel is followed the same way: each message
 * it reports notifies as it is set, and an attempt that clears and fails
 * again notifies again.
 */

import { untrack } from 'svelte';

export type OverlayNoticeTone = 'warning' | 'error';

export interface OverlayNotice {
	id: number;
	text: string;
	tone: OverlayNoticeTone;
}

/** The shortest a notice stays up: long enough to register a short line. */
export const NOTICE_MIN_MS = 2000;
/** The longest a notice stays up unattended; hovering holds it for longer. */
export const NOTICE_MAX_MS = 5000;
/** Notices shown at once; a burst beyond this drops the oldest. */
export const NOTICE_MAX_VISIBLE = 3;

/** How long `text` stays up: a short line clears at the floor, a long one
 * gets roughly the time it takes to read, capped at the ceiling. */
export function noticeDurationMs(text: string): number {
	return Math.min(NOTICE_MAX_MS, Math.max(NOTICE_MIN_MS, 1500 + text.length * 25));
}

export function createOverlayNotices() {
	let notices = $state<OverlayNotice[]>([]);
	const timers = new Map<number, ReturnType<typeof setTimeout>>();
	let held = false;
	let nextId = 1;
	let observedSession: string | null = null;
	const observed = new Set<string>();

	function disarm(id: number) {
		const timer = timers.get(id);
		if (timer !== undefined) clearTimeout(timer);
		timers.delete(id);
	}

	function arm(notice: OverlayNotice) {
		disarm(notice.id);
		if (held) return;
		timers.set(
			notice.id,
			setTimeout(() => dismiss(notice.id), noticeDurationMs(notice.text)),
		);
	}

	function dismiss(id: number) {
		disarm(id);
		notices = notices.filter((notice) => notice.id !== id);
	}

	/** Show `text` as a notice. A message already on screen restarts its
	 * clock rather than stacking a duplicate beneath itself. */
	function push(text: string, tone: OverlayNoticeTone = 'error') {
		const message = text.trim();
		if (!message) return;
		const existing = notices.find((notice) => notice.text === message);
		if (existing) {
			arm(existing);
			return;
		}
		const notice: OverlayNotice = { id: nextId++, text: message, tone };
		const overflow = notices.length + 1 - NOTICE_MAX_VISIBLE;
		for (const dropped of notices.slice(0, Math.max(0, overflow))) disarm(dropped.id);
		notices = [...notices.slice(Math.max(0, overflow)), notice];
		arm(notice);
	}

	/** Notify each warning in the session's cumulative list that has not
	 * notified yet this session. */
	function observeWarnings(sessionId: string | null, warnings: readonly string[]) {
		if (sessionId !== observedSession) {
			observedSession = sessionId;
			observed.clear();
		}
		for (const warning of warnings) {
			if (observed.has(warning)) continue;
			observed.add(warning);
			push(warning, 'warning');
		}
	}

	/** Notify each message `read` reports as it is set. Registers an
	 * effect, so call it during component initialisation. */
	function follow(read: () => string | null) {
		$effect(() => {
			const message = read();
			if (message) untrack(() => push(message));
		});
	}

	/** Pause every notice's clock (the pointer is over them). */
	function hold() {
		held = true;
		for (const id of [...timers.keys()]) disarm(id);
	}

	/** Resume after a hold, giving each notice its full time again. */
	function release() {
		held = false;
		for (const notice of notices) arm(notice);
	}

	function destroy() {
		for (const id of [...timers.keys()]) disarm(id);
		notices = [];
	}

	return {
		get current(): readonly OverlayNotice[] {
			return notices;
		},
		push,
		dismiss,
		observeWarnings,
		follow,
		hold,
		release,
		destroy,
	};
}

export type OverlayNotices = ReturnType<typeof createOverlayNotices>;
