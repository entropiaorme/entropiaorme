/** Public types for the interactive guide module. */

export interface CursorAPI {
	/**
	 * Move the virtual cursor to the centre of an element or rect. Does not toggle visibility.
	 *
	 * `from` forces Motion to tween from explicit keyframes instead of its cached x/y. Required
	 * on looped animations where iter 2+ would otherwise pop directly to the target: the prior
	 * iter's animate() pinned Motion's internal cache at the end position, and the inter-iter
	 * `duration: 0` snap path bypasses Motion (writes style.transform directly) so the cache
	 * stays stale. Passing `from` overrides the cache and gives a visible slide on every iter.
	 */
	moveTo(
		target: HTMLElement | DOMRect,
		opts?: {
			duration?: number;
			offset?: { x: number; y: number };
			from?: { x: number; y: number };
		},
	): Promise<void>;
	/** Play a one-shot click-ripple animation at the cursor's current position. */
	clickRipple(): Promise<void>;
	/** Visual state of the cursor (used by the SVG via data-state attribute). */
	setState(state: 'idle' | 'hover' | 'grab'): void;
	/** Show the cursor. Called automatically by `click()` and `hover()`; call manually for custom flows (e.g. drag). */
	show(): void;
	/** Hide the cursor. Called automatically at step boundaries and after `click()` completes. */
	hide(): void;
}

/** Imperative shim a surface exposes for guide-driven state mutations. */
export type DemoApi = Record<string, (...args: any[]) => any>;

/** Inline span in a `p` block. `href` upgrades the span to an external link (target=_blank). */
export type ProseInline = { text: string; href?: string };

/** Rich prose body block. Cards opt in by passing `body: ProseBlock[]` instead of a single string. */
export type ProseBlock =
	| { kind: 'p'; text: string | ProseInline[] }
	| { kind: 'ul'; items: string[] }
	| { kind: 'svg'; svg: string };

export interface PlayCtx {
	cursor: CursorAPI;
	demoApi: DemoApi;
	/** Move cursor to element + ripple + dispatch a real `click` event. */
	click: (target: HTMLElement | (() => HTMLElement | null)) => Promise<void>;
	/** Move cursor to element + dispatch mouseenter+mousemove so hover handlers fire. */
	hover: (target: HTMLElement | (() => HTMLElement | null)) => Promise<void>;
	/** Sleep. */
	wait: (ms: number) => Promise<void>;
}

export interface GuideStep {
	id: string;
	/**
	 * Lazy anchor resolver; element may only mount after a prior step's play().
	 * Omit for narrative steps (uniform-dim overlay, centred prose card, no cursor).
	 * Return a DOMRect to synthesise a single cutout region from multiple elements,
	 * or DOMRect[] for multiple separate cutout holes (each pierces the dim layer
	 * independently via the SVG path's even-odd fill rule).
	 */
	anchor?: () => HTMLElement | DOMRect | DOMRect[] | null;
	prose: { title: string; body: string | ProseBlock[]; note?: string | ProseInline[] };
	/** Animation + action script that runs on step entry and on replay. Omit for pure-narrative steps. */
	play?: (ctx: PlayCtx) => Promise<void>;
	/** Reverses demo-substrate mutations; called on replay and on step exit. */
	resetDemo?: () => void;
	/**
	 * Set true when the step demonstrates a visible action (cursor click, hover, drag)
	 * the user might want to re-watch. The Replay button renders only when this is true;
	 * cards that just point at a region rely on Back/Next alone.
	 */
	replayable?: boolean;
	/**
	 * Override the prose card's auto-placement (right-of-anchor → bottom → left → top → side-clamp).
	 * `'top-right'` pins to the viewport top-right corner (use for large anchors that fill the page).
	 * `'top'` places anchor-relative above the highlighted region, viewport-clamped if cramped.
	 * `'top-centre'` pins to the viewport top edge, horizontally centred, cleared from titlebar.
	 * `'bottom-left'` places below + horizontally aligned to `placementAnchor` when one is set;
	 * with no `placementAnchor`, pins to the viewport bottom-left corner (mirror of `'top-right'`).
	 * `'bottom-centre'` places below + horizontally centred to `placementAnchor` when one is set;
	 * with no `placementAnchor`, pins to the viewport bottom edge, horizontally centred.
	 */
	placement?: 'top-right' | 'top' | 'top-centre' | 'bottom-left' | 'bottom-centre' | 'middle-right';
	/**
	 * Secondary anchor used only for positioning the prose card (the primary `anchor` drives the
	 * cutout highlights, which may be elsewhere on the page). Used with `placement: 'bottom-left'`
	 * (card lands flush below + left-aligned with this anchor, width matched) or
	 * `placement: 'bottom-centre'` (card lands flush below + horizontally centred to this anchor).
	 */
	placementAnchor?: () => HTMLElement | null;
	/**
	 * Optional pixel offset applied to the computed placement (positive x shifts right,
	 * positive y shifts down). Useful for fine-tuning viewport-pinned placements around
	 * surface-specific chrome (e.g. tab bars, in-page controls) without forking the variant.
	 */
	placementOffset?: { x?: number; y?: number };
}

export interface GuideSurface {
	id: string;
	title: string;
	/**
	 * Optional hook called after the demo data swap, before step 0's play().
	 * Normalises per-surface UI state (active tab, modal flags, expanded rows, mutex toggles)
	 * so the guide always opens from a canonical view regardless of where the user clicked `?`.
	 */
	beforeStart?: (demoApi: DemoApi) => void | Promise<void>;
	steps: GuideStep[];
}
