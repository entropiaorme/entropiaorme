<script lang="ts">
	import { onMount } from 'svelte';
	import { externalLinks } from '$lib/utils/openExternal';
	import { guideState } from './state.svelte';
	import { closeGuide, nextStep, previousStep, replayCurrentStep, setCursorElement } from './engine';

	let cursorEl = $state<HTMLDivElement>();
	let rippleEl = $state<HTMLDivElement>();
	let anchorRects = $state<DOMRect[] | null>(null);
	let placementAnchorRect = $state<DOMRect | null>(null);
	// Convenience: primary rect is the first when present; used for anchor-relative placement.
	let anchorRect = $derived(anchorRects?.[0] ?? null);
	let viewport = $state({ w: 0, h: 0 });

	let currentStep = $derived(
		guideState.currentSurface?.steps[guideState.currentStepIndex] ?? null
	);
	let totalSteps = $derived(guideState.currentSurface?.steps.length ?? 0);
	let isLast = $derived(guideState.currentStepIndex + 1 >= totalSteps);
	/** Narrative steps have no anchor: uniform-dim overlay, centred prose card. */
	let isNarrative = $derived(currentStep != null && !currentStep.anchor);

	onMount(() => {
		const updateViewport = () => {
			viewport = { w: window.innerWidth, h: window.innerHeight };
		};
		updateViewport();
		window.addEventListener('resize', updateViewport);
		return () => {
			window.removeEventListener('resize', updateViewport);
			setCursorElement(null, null);
		};
	});

	// Re-register the cursor handle whenever the {#if guideState.isActive} block
	// mounts or unmounts. onMount fires once at layout time before the conditional
	// block exists, so it would otherwise see a null binding for the lifetime of the app.
	$effect(() => {
		setCursorElement(cursorEl ?? null, rippleEl ?? null);
	});

	// Resolve the anchor rect(s) with polling: anchors may mount mid-step as play() drives the surface.
	// Single-rect and multi-rect anchors normalise into the same DOMRect[] shape; the cutout path
	// renders one inner hole per rect via the SVG even-odd fill rule.
	$effect(() => {
		if (!guideState.isActive || !currentStep) {
			anchorRects = null;
			placementAnchorRect = null;
			return;
		}
		const step = currentStep;
		if (!step.anchor) {
			anchorRects = null;
			placementAnchorRect = null;
			return;
		}
		const anchorFn = step.anchor;
		const placementAnchorFn = step.placementAnchor;
		let cancelled = false;
		const tick = () => {
			if (cancelled) return;
			const result = anchorFn();
			if (!result) {
				if (anchorRects) anchorRects = null;
				return;
			}
			let next: DOMRect[];
			if (result instanceof HTMLElement) {
				next = [result.getBoundingClientRect()];
			} else if (Array.isArray(result)) {
				next = result;
			} else {
				next = [result];
			}
			const prev = anchorRects;
			const same =
				prev != null &&
				prev.length === next.length &&
				prev.every((p, i) => {
					const n = next[i];
					return p.top === n.top && p.left === n.left && p.width === n.width && p.height === n.height;
				});
			if (!same) {
				anchorRects = next;
			}
			const placementEl = placementAnchorFn?.();
			const nextPlacementRect = placementEl?.getBoundingClientRect() ?? null;
			const prevPlacementRect = placementAnchorRect;
			const samePlacement =
				(prevPlacementRect == null && nextPlacementRect == null) ||
				(prevPlacementRect != null &&
					nextPlacementRect != null &&
					prevPlacementRect.top === nextPlacementRect.top &&
					prevPlacementRect.left === nextPlacementRect.left &&
					prevPlacementRect.width === nextPlacementRect.width &&
					prevPlacementRect.height === nextPlacementRect.height);
			if (!samePlacement) {
				placementAnchorRect = nextPlacementRect;
			}
		};
		tick();
		const interval = setInterval(tick, 120);
		return () => {
			cancelled = true;
			clearInterval(interval);
		};
	});

	// SVG cutout path: outer viewport rect + one inner rounded-rect hole per anchor rect,
	// joined into a single path string under the even-odd fill rule so each hole pierces
	// the dim independently. Narrative steps (or anchored-but-not-yet-resolved) collapse
	// the cutout to a 1×1 dot at viewport centre so the CSS transition animates from centre.
	function innerRectPath(rx: number, ry: number, rw: number, rh: number, radius: number): string {
		return (
			`M${rx + radius} ${ry} ` +
			`H${rx + rw - radius} ` +
			`Q${rx + rw} ${ry} ${rx + rw} ${ry + radius} ` +
			`V${ry + rh - radius} ` +
			`Q${rx + rw} ${ry + rh} ${rx + rw - radius} ${ry + rh} ` +
			`H${rx + radius} ` +
			`Q${rx} ${ry + rh} ${rx} ${ry + rh - radius} ` +
			`V${ry + radius} ` +
			`Q${rx} ${ry} ${rx + radius} ${ry} Z`
		);
	}
	let cutoutPath = $derived.by(() => {
		if (viewport.w === 0) return '';
		const W = viewport.w;
		const H = viewport.h;
		const outer = `M0 0 H${W} V${H} H0 Z`;
		if (!anchorRects || anchorRects.length === 0) {
			return `${outer} ${innerRectPath(W / 2 - 0.5, H / 2 - 0.5, 1, 1, 0.5)}`;
		}
		const pad = 6;
		const radius = 8;
		const inners = anchorRects
			.map((rect) =>
				innerRectPath(
					rect.left - pad,
					rect.top - pad,
					rect.width + pad * 2,
					rect.height + pad * 2,
					radius
				)
			)
			.join(' ');
		return `${outer} ${inners}`;
	});

	let proseStyle = $derived.by(() => {
		if (viewport.w === 0) return 'opacity: 0; pointer-events: none;';
		const cardW = 360;
		const cardH = 200;
		const margin = 16;
		// Narrative steps centre the card on screen.
		if (isNarrative) {
			return `left: ${(viewport.w - cardW) / 2}px; top: ${(viewport.h - cardH) / 2}px;`;
		}
		// Explicit placement override (for large anchors that fill the page).
		// Pushed off the absolute top-right corner so the card clears (a) the custom
		// titlebar's min/max/close controls (~138px wide, top 32px) and (b) the in-page
		// X exit button (~56px in from the right, sits below the titlebar).
		if (currentStep?.placement === 'top-right') {
			return `left: ${viewport.w - cardW - 160}px; top: ${44}px;`;
		}
		// Place the card flush below + left-aligned with a secondary anchor (placementAnchor),
		// and match its width so the card fits cleanly into the same column. Used for layouts
		// where the cutout highlights one region but the card belongs in an empty quadrant
		// elsewhere (e.g. codex: cutout on the right detail panel, card under the mobs list
		// in the lower-left empty area, width matching the mobs list sidebar).
		if (currentStep?.placement === 'bottom-left' && currentStep?.placementAnchor) {
			if (placementAnchorRect) {
				const x = Math.max(
					margin,
					Math.min(placementAnchorRect.left, viewport.w - placementAnchorRect.width - margin)
				);
				const y = Math.max(margin, placementAnchorRect.bottom + 12);
				return `left: ${x}px; top: ${y}px; width: ${placementAnchorRect.width}px;`;
			}
		}
		// Viewport-pinned bottom-left: when no placementAnchor is supplied, pin to the
		// viewport bottom-left corner with sensible offsets. Mirror of the 'top-right'
		// branch above (analytics ledger-add-entry card lives here, where the cutout
		// fills the main ledger column and the card needs a permanent off-content seat).
		if (currentStep?.placement === 'bottom-left' && !currentStep?.placementAnchor) {
			return `left: ${margin + 8}px; bottom: ${margin + 8}px; top: auto;`;
		}
		// Anchor-relative bottom-centre: when a placementAnchor is supplied, place the card
		// flush below + horizontally centred to that anchor (mirror of 'bottom-left'+anchor,
		// but centred instead of left-aligned). placementOffset nudges the centred base
		// (positive x shifts right, positive y shifts down). Used when the cutout sits on a
		// fixed-positioned demo element (e.g. dashboard overlay screenshot) and the card
		// should hang directly underneath rather than at a viewport corner.
		if (currentStep?.placement === 'bottom-centre' && currentStep?.placementAnchor) {
			if (placementAnchorRect) {
				const offX = currentStep?.placementOffset?.x ?? 0;
				const offY = currentStep?.placementOffset?.y ?? 0;
				const cx = placementAnchorRect.left + placementAnchorRect.width / 2;
				const x = Math.max(
					margin,
					Math.min(cx - cardW / 2 + offX, viewport.w - cardW - margin)
				);
				const y = Math.max(margin, placementAnchorRect.bottom + 12 + offY);
				return `left: ${x}px; top: ${y}px;`;
			}
			// Falls through to viewport-pinned 'bottom-centre' below until placementAnchor
			// resolves (e.g. dashboard overlay-spawn step's phase-1 sweep before the
			// screenshot mounts).
		}
		// Viewport-pinned bottom-centre: horizontally centred at the bottom edge.
		// placementOffset semantics mirror 'top-centre': positive x shifts right,
		// positive y shifts the card AWAY from the bottom edge (upward). Lets a
		// card pad off the bottom (e.g. dashboard-intro lifts ~26px to land 50px
		// from the viewport bottom instead of the default 24px).
		if (currentStep?.placement === 'bottom-centre') {
			const offX = currentStep?.placementOffset?.x ?? 0;
			const offY = currentStep?.placementOffset?.y ?? 0;
			return `left: ${(viewport.w - cardW) / 2 + offX}px; bottom: ${margin + 8 + offY}px; top: auto;`;
		}
		// Viewport-pinned top-centre: horizontally centred at the top edge, with the same
		// 44px top offset as 'top-right' so the card clears the custom titlebar
		// (32px) + a small gap. Naturally clear of the ?-button (which sits at the
		// right edge of the page header) since it's centre-horizontal. placementOffset
		// applies after the centred base so cards can nudge around tab bars or other
		// surface-specific chrome.
		if (currentStep?.placement === 'top-centre') {
			const offX = currentStep?.placementOffset?.x ?? 0;
			const offY = currentStep?.placementOffset?.y ?? 0;
			return `left: ${(viewport.w - cardW) / 2 + offX}px; top: ${44 + offY}px;`;
		}
		// Middle-right placement, dual mode (mirrors bottom-centre / bottom-left
		// shape): with a placementAnchor, top-align with the anchor's top and
		// pin to the viewport's right edge. Without one, vertically centre via
		// translateY(-50%); safe on the text-only prose card (the transform-
		// blur quirk applies to bitmap content, not pure text).
		if (currentStep?.placement === 'middle-right') {
			const offX = currentStep?.placementOffset?.x ?? 0;
			const offY = currentStep?.placementOffset?.y ?? 0;
			if (placementAnchorRect) {
				const top = Math.max(margin, placementAnchorRect.top + offY);
				return `right: ${margin + offX}px; top: ${top}px;`;
			}
			return `right: ${margin + offX}px; top: calc(50% + ${offY}px); transform: translateY(-50%);`;
		}
		if (!anchorRect) return 'opacity: 0; pointer-events: none;';
		// Anchor-relative top: pin the card's BOTTOM 12px above the anchor and let the card
		// grow upwards by its actual content height. Avoids the cardH-constant mismatch where
		// a content-rich card (3 bullets + 2 paragraphs ≈ 280px) would overflow past the
		// estimated cardH=200 and bleed back into the highlighted region.
		if (currentStep?.placement === 'top') {
			const x = Math.max(margin, Math.min(anchorRect.left, viewport.w - cardW - margin));
			const bottomGap = viewport.h - anchorRect.top + 12;
			return `left: ${x}px; bottom: ${bottomGap}px; top: auto;`;
		}
		// Try right, then bottom, then left, then top; pick the first that fits.
		const candidates = [
			{ x: anchorRect.right + 24, y: anchorRect.top },
			{ x: anchorRect.left, y: anchorRect.bottom + 16 },
			{ x: anchorRect.left - cardW - 24, y: anchorRect.top },
			{ x: anchorRect.left, y: anchorRect.top - cardH - 16 }
		];
		for (const c of candidates) {
			if (
				c.x >= margin &&
				c.x + cardW <= viewport.w - margin &&
				c.y >= margin &&
				c.y + cardH <= viewport.h - margin
			) {
				return `left: ${c.x}px; top: ${c.y}px;`;
			}
		}
		// Fallback: slide to whichever side of the anchor has more room, clamp to viewport.
		// Avoids centring on top of large anchors (e.g. modals) that fail all four strict candidates.
		const rightRoom = viewport.w - anchorRect.right - margin;
		const leftRoom = anchorRect.left - margin;
		const x = rightRoom >= leftRoom
			? Math.min(anchorRect.right + 24, viewport.w - cardW - margin)
			: Math.max(anchorRect.left - cardW - 24, margin);
		const y = Math.max(margin, Math.min(anchorRect.top, viewport.h - cardH - margin));
		return `left: ${x}px; top: ${y}px;`;
	});
</script>

{#if guideState.isActive}
	<!-- Click blocker: starts at y=32 to keep the custom titlebar (drag region + min/max/close)
	     usable as a safeguard. The titlebar covers the top 32px of the layout; everything below
	     gets dimmed + blocked. The 32px constant tracks Titlebar.svelte's .titlebar height. -->
	<div
		class="fixed left-0 right-0 bottom-0 z-[8999] pointer-events-auto"
		style="top: 32px;"
		role="presentation"
	></div>
	<!-- Visual layer: cutout dim + virtual cursor + prose card. Full-viewport but pointer-events:
	     none so the titlebar shows through; prose card opts back in for its own clicks. The guide
	     engine drives real DOM clicks via el.click(), which bypasses pointer-events entirely. -->
	<div
		class="fixed inset-0 z-[9000] pointer-events-none"
		role="presentation"
	>
		<!-- Cutout SVG: dimmed surround + transparent hole, even-odd fill, single path animates between steps. -->
		<svg
			class="absolute inset-0 w-full h-full pointer-events-none"
			width={viewport.w}
			height={viewport.h}
			aria-hidden="true"
		>
			<path
				d={cutoutPath}
				fill="rgba(0, 0, 0, 0.62)"
				fill-rule="evenodd"
				style="transition: d 0.35s cubic-bezier(0.4, 0, 0.2, 1);"
			/>
		</svg>

		<!-- Virtual cursor: app-stylised SVG, brand-tinted, click ripple, deliberately distinct from system pointer. -->
		<div
			bind:this={cursorEl}
			class="guide-cursor absolute top-0 left-0 pointer-events-none transition-opacity duration-200"
			class:opacity-0={!guideState.cursorVisible}
			data-state="idle"
			style="transform: translate(0px, 0px);"
		>
			<svg width="28" height="28" viewBox="0 0 28 28" fill="none" aria-hidden="true">
				<path
					d="M5 3 L5 22 L11 17 L15 25 L18 23.5 L13.6 16 L21.5 16 Z"
					fill="rgb(99, 179, 237)"
					stroke="rgba(15, 23, 42, 0.55)"
					stroke-width="1"
					stroke-linejoin="round"
				/>
			</svg>
			<div
				bind:this={rippleEl}
				class="absolute top-[3px] left-[3px] w-6 h-6 rounded-full border-2 opacity-0"
				style="border-color: rgb(99, 179, 237);"
			></div>
		</div>

		<!-- Prose card with Replay + Next. -->
		{#if currentStep}
			<div
				class="absolute w-[360px] bg-surface-raised border border-border rounded-lg shadow-2xl p-4 pointer-events-auto"
				style={proseStyle}
			>
				<div class="flex items-start justify-between mb-2 gap-2">
					<h3 class="text-sm font-semibold text-text leading-snug">
						{currentStep.prose.title}
					</h3>
					<button
						class="shrink-0 text-text-tertiary hover:text-text transition-colors"
						onclick={closeGuide}
						aria-label="Close guide"
					>
						<svg
							xmlns="http://www.w3.org/2000/svg"
							viewBox="0 0 16 16"
							fill="currentColor"
							class="w-4 h-4"
						>
							<path
								d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z"
							/>
						</svg>
					</button>
				</div>
				{#if typeof currentStep.prose.body === 'string'}
					<p class="text-sm text-text-secondary leading-relaxed">
						{currentStep.prose.body}
					</p>
				{:else}
					<div class="space-y-2 text-sm text-text-secondary leading-relaxed">
						{#each currentStep.prose.body as block}
							{#if block.kind === 'p'}
								<p>
									{#if typeof block.text === 'string'}
										{block.text}
									{:else}
										{#each block.text as span}
											{#if span.href}
												<a
													href={span.href}
													target="_blank"
													rel="noopener noreferrer"
													class="text-accent hover:underline"
													use:externalLinks
												>{span.text}</a>
											{:else}
												{span.text}
											{/if}
										{/each}
									{/if}
								</p>
							{:else if block.kind === 'ul'}
								<ul class="list-disc pl-5 space-y-1">
									{#each block.items as item}
										<li>{item}</li>
									{/each}
								</ul>
							{:else}
								<div class="flex justify-center pt-1">{@html block.svg}</div>
							{/if}
						{/each}
					</div>
				{/if}
				{#if currentStep.prose.note}
					<p class="mt-2 text-xs text-text-tertiary leading-relaxed">
						{#if typeof currentStep.prose.note === 'string'}
							{currentStep.prose.note}
						{:else}
							{#each currentStep.prose.note as span}
								{#if span.href}
									<a
										href={span.href}
										target="_blank"
										rel="noopener noreferrer"
										class="text-accent hover:underline"
										use:externalLinks
									>{span.text}</a>
								{:else}
									{span.text}
								{/if}
							{/each}
						{/if}
					</p>
				{/if}
				<div class="flex items-center justify-between mt-4">
					<span class="text-xs text-text-tertiary tabular-nums">
						{guideState.currentStepIndex + 1} / {totalSteps}
					</span>
					<div class="flex items-center gap-2">
						{#if guideState.currentStepIndex > 0}
							<button
								type="button"
								class="px-2.5 py-1 text-xs rounded-md text-text-secondary hover:bg-surface-hover transition-colors"
								onclick={previousStep}
							>
								Back
							</button>
						{/if}
						{#if currentStep.replayable}
							<button
								type="button"
								class="px-2.5 py-1 text-xs rounded-md text-text-secondary hover:bg-surface-hover disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
								onclick={replayCurrentStep}
								disabled={guideState.isPlaying}
							>
								Replay
							</button>
						{/if}
						<button
							type="button"
							class="px-3 py-1 text-xs font-medium rounded-md bg-accent text-white hover:bg-accent-hover transition-colors"
							onclick={nextStep}
						>
							{isLast ? 'Done' : 'Next'}
						</button>
					</div>
				</div>
			</div>
		{/if}
	</div>
{/if}

<style>
	.guide-cursor {
		will-change: transform;
		filter: drop-shadow(0 2px 6px rgba(15, 23, 42, 0.45));
	}
</style>
