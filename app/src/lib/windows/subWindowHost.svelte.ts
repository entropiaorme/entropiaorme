/**
 * The host's half of the overlay's sub-windows: which is open, and every
 * rule for opening and closing one (the rules are stated in `subWindow.ts`).
 *
 * Sub-windows live in slots: a slot is one OS window, showing one sub-window
 * at a time. Opening a sub-window closes every other one, in whatever slot,
 * except one that stays (a task the player has left for the game).
 *
 * The host knows where the pointer is over its own control; the sub-window
 * reports where it is over itself. Away from both, and with nobody typing,
 * the sub-window closes after `awayMs`.
 */

import type { SatelliteWindow } from './satellite';
import type {
	ScreenRect,
	SubWindowAlign,
	SubWindowAnchor,
	SubWindowChannel,
	SubWindowCloseReason,
	SubWindowEnvelope,
	SubWindowReport,
} from './subWindow';

/** How long a sub-window stays once the pointer has left both it and its
 * control. */
export const SUB_WINDOW_AWAY_MS = 2000;

/** A click on a control just after its sub-window closed itself (the same
 * click took the keyboard from it) is that close, not a reopening. */
export const SUB_WINDOW_RECLICK_MS = 250;

/** Why a sub-window closed: by itself, or by the host. */
export type SubWindowClosedBy = SubWindowCloseReason | 'away' | 'toggle' | 'replaced' | 'host';

export interface SubWindowSlot {
	window: SatelliteWindow;
	channel: SubWindowChannel;
}

export interface SubWindowRequest {
	/** The slot (OS window) it opens in. */
	slot: string;
	/** Which sub-window it is. */
	id: string;
	/** The control it hangs from. */
	anchor: HTMLElement;
	/** What it shows. */
	state: unknown;
	/** Take the keyboard (default true). */
	focus?: boolean;
	/** A task the player leaves and returns to. */
	stays?: boolean;
	/** It belongs to a text field in the host, which decides when it goes. */
	attached?: boolean;
	align?: SubWindowAlign;
}

export interface SubWindowHostDeps {
	slots: Record<string, SubWindowSlot>;
	/** The gap between a control's surface and its sub-window. */
	gap: number;
	/** Where a sub-window hangs from a control. */
	anchorOf: (anchor: HTMLElement) => Promise<SubWindowAnchor>;
	/** The screen sub-windows must stay on. */
	bounds: () => Promise<ScreenRect | null>;
	/** Hear a slot's reports; answers the teardown. */
	listen: (event: string, handler: (report: SubWindowReport) => void) => Promise<() => void>;
	/** A sub-window could not be opened. */
	onError?: (id: string, error: unknown) => void;
	/** A sub-window closed, by whatever cause. */
	onClosed?: (id: string, by: SubWindowClosedBy) => void;
	/** The player did something in a sub-window. */
	onInteract?: (id: string) => void;
	awayMs?: number;
	now?: () => number;
}

interface OpenSubWindow {
	request: SubWindowRequest;
	pointerOnAnchor: boolean;
	pointerInside: boolean;
	held: boolean;
	timer: ReturnType<typeof setTimeout> | undefined;
	unwatch: () => void;
}

export function createSubWindowHost(deps: SubWindowHostDeps) {
	const awayMs = deps.awayMs ?? SUB_WINDOW_AWAY_MS;
	const now = deps.now ?? (() => Date.now());
	// What is open, by slot. The ids are reactive, for the controls' state.
	const open = new Map<string, OpenSubWindow>();
	let openIds = $state<Record<string, string>>({});
	const lastClosed = new Map<string, { id: string; at: number }>();
	// Each slot's shows run one after another, so a later request never
	// lands before an earlier one.
	const queues = new Map<string, Promise<void>>();

	function publish() {
		openIds = Object.fromEntries(
			[...open.entries()].map(([slot, held]) => [slot, held.request.id]),
		);
	}

	function slotOf(id: string): string | null {
		for (const [slot, held] of open) {
			if (held.request.id === id) return slot;
		}
		return null;
	}

	function clearTimer(held: OpenSubWindow) {
		if (held.timer === undefined) return;
		clearTimeout(held.timer);
		held.timer = undefined;
	}

	/** Start or stop the away timer for what `slot` shows. */
	function weigh(slot: string) {
		const held = open.get(slot);
		if (!held) return;
		clearTimer(held);
		const { stays, attached } = held.request;
		if (stays || attached || held.held || held.pointerOnAnchor || held.pointerInside) return;
		held.timer = setTimeout(() => {
			held.timer = undefined;
			void closeSlot(slot, 'away');
		}, awayMs);
	}

	/** Follow the pointer over `anchor` for what `slot` shows. */
	function watchAnchor(slot: string, anchor: HTMLElement): () => void {
		const set = (inside: boolean) => () => {
			const held = open.get(slot);
			if (!held || held.request.anchor !== anchor) return;
			held.pointerOnAnchor = inside;
			weigh(slot);
		};
		const enter = set(true);
		const leave = set(false);
		anchor.addEventListener('pointerenter', enter);
		anchor.addEventListener('pointerleave', leave);
		return () => {
			anchor.removeEventListener('pointerenter', enter);
			anchor.removeEventListener('pointerleave', leave);
		};
	}

	/** Forget what `slot` shows, without telling its window. */
	function forget(slot: string, by: SubWindowClosedBy): string | null {
		const held = open.get(slot);
		if (!held) return null;
		clearTimer(held);
		held.unwatch();
		open.delete(slot);
		publish();
		deps.onClosed?.(held.request.id, by);
		return held.request.id;
	}

	async function closeSlot(slot: string, by: SubWindowClosedBy): Promise<void> {
		if (forget(slot, by) === null) return;
		await deps.slots[slot]?.window.hide();
	}

	async function envelope(request: SubWindowRequest): Promise<SubWindowEnvelope> {
		const [anchor, bounds] = await Promise.all([deps.anchorOf(request.anchor), deps.bounds()]);
		return {
			id: request.id,
			state: request.state,
			anchor,
			bounds,
			gap: deps.gap,
			align: request.align ?? 'start',
			focus: request.focus ?? true,
			stays: request.stays === true,
		};
	}

	function queued(slot: string, work: () => Promise<void>): Promise<void> {
		const next = (queues.get(slot) ?? Promise.resolve()).then(work, work);
		queues.set(slot, next);
		return next;
	}

	/**
	 * Open `request`, or, when it is the one its slot already shows, bring
	 * what it shows up to date without revealing it again. Every other
	 * sub-window closes, except one that stays.
	 *
	 * With `opening` false nothing is opened: the request only brings an
	 * open sub-window up to date, and one that has closed since it was
	 * asked for stays closed.
	 */
	async function openRequest(request: SubWindowRequest, opening = true): Promise<boolean> {
		const slot = deps.slots[request.slot];
		if (!slot || !request.anchor.isConnected) return false;
		if (opening) {
			for (const [other, held] of [...open.entries()]) {
				if (other !== request.slot && !held.request.stays) await closeSlot(other, 'replaced');
			}
		}
		let shown = false;
		await queued(request.slot, async () => {
			const current = open.get(request.slot);
			try {
				if (current && current.request.id === request.id) {
					// The same sub-window: new state, perhaps a new control.
					// Only a new control changes where the pointer stands: an
					// update alone never restarts the wait to close.
					const moved = current.request.anchor !== request.anchor;
					if (moved) {
						current.unwatch();
						current.unwatch = watchAnchor(request.slot, request.anchor);
						current.pointerOnAnchor = request.anchor.matches(':hover');
					}
					current.request = request;
					await slot.window.emitTo(slot.channel.update, await envelope(request));
					if (moved) weigh(request.slot);
					shown = true;
					return;
				}
				if (!opening) return;
				if (current) forget(request.slot, 'replaced');
				await slot.window.ensure();
				const payload = await envelope(request);
				open.set(request.slot, {
					request,
					pointerOnAnchor: request.anchor.matches(':hover'),
					pointerInside: false,
					held: false,
					timer: undefined,
					unwatch: watchAnchor(request.slot, request.anchor),
				});
				publish();
				await slot.window.show(payload, undefined, {
					reveal: false,
					focus: payload.focus,
				});
				shown = true;
			} catch (error) {
				forget(request.slot, 'host');
				deps.onError?.(request.id, error);
			}
		});
		return shown;
	}

	function reclicked(id: string): boolean {
		for (const [slot, closed] of lastClosed) {
			if (closed.id !== id) continue;
			lastClosed.delete(slot);
			return now() - closed.at < SUB_WINDOW_RECLICK_MS;
		}
		return false;
	}

	function handleReport(slot: string, report: SubWindowReport) {
		const held = open.get(slot);
		if (!held || held.request.id !== report.id) return;
		switch (report.type) {
			case 'closed':
				lastClosed.set(slot, { id: report.id, at: now() });
				forget(slot, report.reason);
				return;
			case 'pointer':
				held.pointerInside = report.inside;
				weigh(slot);
				return;
			case 'held':
				held.held = report.held;
				weigh(slot);
				return;
			case 'interact':
				deps.onInteract?.(report.id);
				return;
		}
	}

	return {
		/** Whether the sub-window `id` is open. */
		isOpen(id: string): boolean {
			return Object.values(openIds).includes(id);
		},
		/** What `slot` shows, if anything. */
		shownIn(slot: string): string | null {
			return openIds[slot] ?? null;
		},
		open: (request: SubWindowRequest) => openRequest(request),
		/**
		 * Whether the click now landing on the control of `id` is the one
		 * that just closed it (it took the keyboard from the sub-window,
		 * which closed itself): asked before opening, and true once.
		 */
		reclicked,
		/** The control was clicked: close its sub-window when it is open,
		 * else open it. Answers whether it is open afterwards. */
		async toggle(request: SubWindowRequest): Promise<boolean> {
			const current = open.get(request.slot);
			if (current && current.request.id === request.id) {
				await closeSlot(request.slot, 'toggle');
				return false;
			}
			if (reclicked(request.id)) return false;
			return openRequest(request);
		},
		/** Bring what the open sub-window `id` shows up to date. */
		async update(id: string, state: unknown): Promise<void> {
			const slot = slotOf(id);
			const held = slot ? open.get(slot) : undefined;
			if (!slot || !held) return;
			await openRequest({ ...held.request, state }, false);
		},
		/** Close the sub-window `id`, when it is open. */
		async close(id: string): Promise<void> {
			const slot = slotOf(id);
			if (slot) await closeSlot(slot, 'host');
		},
		/** Close every sub-window, except those `keep` answers for. */
		async closeAll(keep: (id: string) => boolean = () => false): Promise<void> {
			for (const [slot, held] of [...open.entries()]) {
				if (!keep(held.request.id)) await closeSlot(slot, 'host');
			}
		},
		/** The host moved or changed size: every open sub-window follows its
		 * control, and one whose control is gone closes. */
		async reanchor(): Promise<void> {
			for (const [slot, held] of [...open.entries()]) {
				if (!held.request.anchor.isConnected) {
					await closeSlot(slot, 'host');
					continue;
				}
				await openRequest(held.request, false);
			}
		},
		/** Hear every slot's reports; answers the teardown. */
		connect(): () => void {
			let disposed = false;
			const detach: Array<() => void> = [];
			for (const [slot, { channel }] of Object.entries(deps.slots)) {
				void deps
					.listen(channel.report, (report) => {
						if (!disposed) handleReport(slot, report);
					})
					.then((stop) => {
						if (disposed) stop();
						else detach.push(stop);
					});
			}
			return () => {
				disposed = true;
				for (const stop of detach) stop();
				for (const held of open.values()) {
					clearTimer(held);
					held.unwatch();
				}
			};
		},
	};
}

export type SubWindowHost = ReturnType<typeof createSubWindowHost>;
