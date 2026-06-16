<script lang="ts">
	import { settleTween } from '$lib/motion/testMotion';
	import { cubicOut } from 'svelte/easing';
	import { formatMultiplier, formatPed } from '$lib/utils/format';

	let {
		history,
		netHistory,
	}: { history: number[] | null; netHistory: number[] | null } = $props();

	const MIN_Y_MAX = 2; // multiplier floor so a flat ~1x session still renders cleanly
	const Y_HEAD_ROOM = 1.15; // extra padding above the highest point
	const NET_MIN_RANGE = 1.0; // PED — minimum total y-span for the P&L chart so a tiny session still renders cleanly

	// Pixel-space chart geometry. Width follows the container; height is fixed
	// so the chart never grows disproportionately tall on wide screens. We
	// drive the SVG viewBox from the live container width so 1 SVG unit = 1
	// pixel, which keeps dots round at any width without distortion.
	const CHART_HEIGHT = 170;
	const PAD_L = 38; // room for y-axis labels (current max + 1×)
	const PAD_R = 10;
	const PAD_T = 14;
	const PAD_B = 14;
	const PLOT_H = CHART_HEIGHT - PAD_T - PAD_B;

	// Width-driven dot density. The visible window adapts to whatever fits at
	// roughly TARGET_PITCH px/dot, clamped so the chart stays readable in a
	// narrow panel and doesn't blow out into a sparse mess on huge screens.
	const TARGET_PITCH = 24;
	const MIN_VISIBLE = 12;
	const MAX_VISIBLE = 80;
	const FALLBACK_WIDTH = 600; // until the bind:clientWidth lands

	// Dot colour bands (per spec). Multipliers below 1× use the same orange
	// the dashboard's Net stat uses for negative values (color-negative).
	function dotColour(m: number): string {
		if (m < 1) return 'var(--color-negative)';
		if (m < 3) return 'var(--color-accent)';
		if (m < 10) return 'var(--color-positive)';
		return 'var(--color-warning)';
	}

	let containerWidth = $state(FALLBACK_WIDTH);
	let chartWidth = $derived(Math.max(FALLBACK_WIDTH / 2, containerWidth));
	let plotWidth = $derived(chartWidth - PAD_L - PAD_R);

	let visibleCount = $derived(
		Math.min(
			MAX_VISIBLE,
			Math.max(MIN_VISIBLE, Math.round(plotWidth / TARGET_PITCH) + 1)
		)
	);

	let trimmed = $derived((history ?? []).slice(-visibleCount));

	let targetYMax = $derived(
		Math.max(MIN_Y_MAX, ...(trimmed.length > 0 ? trimmed : [0])) * Y_HEAD_ROOM
	);

	// Smooth y-axis rescale. The chart "breathes" when a big multiplier
	// lands or scrolls off the trailing edge.
	const yMax = settleTween(MIN_Y_MAX * Y_HEAD_ROOM, {
		duration: 600,
		easing: cubicOut,
	});
	$effect(() => {
		void yMax.set(targetYMax);
	});

	function xAt(i: number, n: number, slots: number): number {
		// Right-anchored from tick one: newest sits at the rightmost slot
		// regardless of how many points exist. With n=1 that puts the dot at
		// the right edge immediately rather than the centre.
		const slot = slots - n + i;
		return PAD_L + (slot / (slots - 1)) * plotWidth;
	}

	function yAt(m: number, scale: number): number {
		const clamped = Math.min(m, scale);
		return PAD_T + PLOT_H - (clamped / scale) * PLOT_H;
	}

	let pathD = $derived.by(() => {
		const n = trimmed.length;
		if (n === 0) return '';
		const scale = $yMax;
		const slots = visibleCount;
		const pts = trimmed.map(
			(m, i) => `${xAt(i, n, slots).toFixed(2)},${yAt(m, scale).toFixed(2)}`
		);
		return `M ${pts.join(' L ')}`;
	});

	let oneLineY = $derived(yAt(1, $yMax));
	let plotRight = $derived(chartWidth - PAD_R);
	let plotBottom = PAD_T + PLOT_H;

	// Placeholder shape for the multiplier chart's empty state — a believable
	// run dominated by dots near 0.85–0.95× with two outlier spikes. Drawn
	// muted so it blends into the panel and reads as a hint at the chart's
	// shape, not as live data.
	const PLACEHOLDER_MULT: number[] = [
		0.85, 0.92, 0.88, 0.95, 0.79, 0.91, 0.94, 1.72,
		0.83, 0.88, 0.92, 0.86, 0.94, 3.18, 0.82, 0.89,
	];
	const PLACEHOLDER_MULT_YMAX = 3.6; // fits the 3.18 spike with head-room

	// Shared X-position helper for both placeholders — spreads N points
	// evenly across the full plot width. The placeholder ignores the
	// right-anchored / per-kill window of the real chart, and uses the same
	// N (16) for both charts so spike columns line up vertically.
	function placeholderX(i: number, n: number): number {
		if (n <= 1) return PAD_L + plotWidth / 2;
		return PAD_L + (i / (n - 1)) * plotWidth;
	}

	function placeholderMultY(m: number): number {
		const clamped = Math.min(m, PLACEHOLDER_MULT_YMAX);
		return PAD_T + PLOT_H - (clamped / PLACEHOLDER_MULT_YMAX) * PLOT_H;
	}

	let placeholderMultPath = $derived.by(() => {
		const n = PLACEHOLDER_MULT.length;
		const pts = PLACEHOLDER_MULT.map(
			(m, i) => `${placeholderX(i, n).toFixed(2)},${placeholderMultY(m).toFixed(2)}`
		);
		return `M ${pts.join(' L ')}`;
	});

	let placeholderMultOneLineY = $derived(
		PAD_T + PLOT_H - (1 / PLACEHOLDER_MULT_YMAX) * PLOT_H
	);

	// Placeholder for the cumulative session P&L chart. Hand-tuned so its
	// spikes land at indices 7 and 13 — exactly where PLACEHOLDER_MULT
	// spikes — and so the curve tells the same story: a slow drift down,
	// a sudden jump up at the first multiplier spike, drift down again,
	// then a bigger jump up at the second spike before easing back.
	const PLACEHOLDER_NET: number[] = [
		-0.4, -0.7, -1.0, -1.2, -1.7, -2.0, -2.3, // slow decline …
		-0.2,                                      // … 1st spike up (idx 7)
		-0.6, -0.9, -1.2, -1.7, -2.1,              // resume decline
		 4.8,                                      // … 2nd spike up (idx 13)
		 4.4,  4.1,                                // ease back down
	];
	const PLACEHOLDER_NET_MAX = 5.4;
	const PLACEHOLDER_NET_MIN = -2.7;

	function placeholderNetY(value: number): number {
		const span = PLACEHOLDER_NET_MAX - PLACEHOLDER_NET_MIN;
		const clamped = Math.min(Math.max(value, PLACEHOLDER_NET_MIN), PLACEHOLDER_NET_MAX);
		const norm = (clamped - PLACEHOLDER_NET_MIN) / span;
		return PAD_T + PLOT_H - norm * PLOT_H;
	}

	let placeholderNetPath = $derived.by(() => {
		const n = PLACEHOLDER_NET.length;
		const pts = PLACEHOLDER_NET.map(
			(v, i) => `${placeholderX(i, n).toFixed(2)},${placeholderNetY(v).toFixed(2)}`
		);
		return `M ${pts.join(' L ')}`;
	});

	let placeholderNetZeroY = $derived(placeholderNetY(0));

	// ── Chart 2: cumulative session P&L (per kill) ──────────────────────
	// Mirrors chart 1's geometry/density; uses a signed y-axis around 0.
	let netTrimmed = $derived((netHistory ?? []).slice(-visibleCount));

	let netRawMax = $derived(
		netTrimmed.length > 0 ? Math.max(...netTrimmed) : 0
	);
	let netRawMin = $derived(
		netTrimmed.length > 0 ? Math.min(...netTrimmed) : 0
	);
	// 0 is always inside the range (so the dashed reference is visible);
	// each side gets head-room so the curve doesn't graze the chart edges.
	let netTargetMax = $derived(Math.max(0, netRawMax) * Y_HEAD_ROOM);
	let netTargetMin = $derived(Math.min(0, netRawMin) * Y_HEAD_ROOM);
	// Enforce a minimum total span so a near-zero session still renders.
	let netMaxFinal = $derived.by(() => {
		const span = netTargetMax - netTargetMin;
		if (span >= NET_MIN_RANGE) return netTargetMax;
		const pad = (NET_MIN_RANGE - span) / 2;
		return netTargetMax + pad;
	});
	let netMinFinal = $derived.by(() => {
		const span = netTargetMax - netTargetMin;
		if (span >= NET_MIN_RANGE) return netTargetMin;
		const pad = (NET_MIN_RANGE - span) / 2;
		return netTargetMin - pad;
	});

	const netMaxTween = settleTween(NET_MIN_RANGE / 2, { duration: 600, easing: cubicOut });
	const netMinTween = settleTween(-NET_MIN_RANGE / 2, { duration: 600, easing: cubicOut });
	$effect(() => {
		void netMaxTween.set(netMaxFinal);
		void netMinTween.set(netMinFinal);
	});

	function netYAt(value: number, lo: number, hi: number): number {
		const span = hi - lo;
		if (span <= 0) return PAD_T + PLOT_H / 2;
		const clamped = Math.min(Math.max(value, lo), hi);
		const norm = (clamped - lo) / span;
		return PAD_T + PLOT_H - norm * PLOT_H;
	}

	let netZeroY = $derived(netYAt(0, $netMinTween, $netMaxTween));

	let netPathD = $derived.by(() => {
		const n = netTrimmed.length;
		if (n === 0) return '';
		const lo = $netMinTween;
		const hi = $netMaxTween;
		const slots = visibleCount;
		const pts = netTrimmed.map(
			(v, i) => `${xAt(i, n, slots).toFixed(2)},${netYAt(v, lo, hi).toFixed(2)}`
		);
		return `M ${pts.join(' L ')}`;
	});

	function netDotColour(value: number): string {
		return value >= 0 ? 'var(--color-positive)' : 'var(--color-negative)';
	}

	// Sign-aware PED label — explicit + for profit, minus is intrinsic.
	function formatPedSigned(value: number): string {
		return (value > 0 ? '+' : '') + formatPed(value);
	}
</script>

<div class="flex-1 min-h-0 flex flex-col gap-5 overflow-y-auto -mr-2 pr-2">
	<!-- Chart 1: per-kill multiplier (rolling window) -->
	<section class="flex flex-col gap-2">
		<div class="flex items-center justify-between gap-3">
			<span class="eyebrow">Per-kill multiplier</span>
			<div class="flex flex-wrap items-center justify-end gap-x-2.5 gap-y-1 text-[10px] tracking-[0.12em] uppercase font-medium text-text-tertiary">
				<span class="flex items-center gap-1.5">
					<span class="block h-1.5 w-1.5 rounded-full" style="background:var(--color-negative);"></span>
					&lt;1×
				</span>
				<span class="flex items-center gap-1.5">
					<span class="block h-1.5 w-1.5 rounded-full" style="background:var(--color-accent);"></span>
					1–3×
				</span>
				<span class="flex items-center gap-1.5">
					<span class="block h-1.5 w-1.5 rounded-full" style="background:var(--color-positive);"></span>
					3–10×
				</span>
				<span class="flex items-center gap-1.5">
					<span class="block h-1.5 w-1.5 rounded-full" style="background:var(--color-warning);"></span>
					10×+
				</span>
			</div>
		</div>

		<div
			class="relative rounded-md border border-border/60 bg-base/40 p-2"
			bind:clientWidth={containerWidth}
		>
			<div class="relative" style="height: {CHART_HEIGHT}px;">
				{#if trimmed.length === 0}
					<svg
						viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
						width={chartWidth}
						height={CHART_HEIGHT}
						class="block"
						role="img"
						aria-hidden="true"
					>
						<!-- Muted axes -->
						<line
							x1={PAD_L} x2={PAD_L} y1={PAD_T} y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 35%, transparent)"
							stroke-width="1"
						/>
						<line
							x1={PAD_L} x2={plotRight} y1={plotBottom} y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 35%, transparent)"
							stroke-width="1"
						/>
						<!-- Muted dashed 1× -->
						<line
							x1={PAD_L} x2={plotRight}
							y1={placeholderMultOneLineY} y2={placeholderMultOneLineY}
							stroke="color-mix(in oklab, var(--color-border-bright) 30%, transparent)"
							stroke-width="1"
							stroke-dasharray="3 4"
						/>
						<!-- Muted connecting line -->
						<path
							d={placeholderMultPath}
							fill="none"
							stroke="color-mix(in oklab, var(--color-text-tertiary) 28%, transparent)"
							stroke-width="1.25"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
						<!-- Muted dots — single faint tone, no colour bands -->
						{#each PLACEHOLDER_MULT as m, i (i)}
							<circle
								cx={placeholderX(i, PLACEHOLDER_MULT.length)}
								cy={placeholderMultY(m)}
								r={2.5}
								fill="color-mix(in oklab, var(--color-text-tertiary) 45%, transparent)"
							/>
						{/each}
					</svg>
					<div class="absolute inset-x-0 top-[18%] flex items-center justify-center pointer-events-none">
						<p class="text-xs text-text-tertiary px-2.5 py-1 rounded-sm bg-base/60 backdrop-blur-[2px]">
							No active session
						</p>
					</div>
				{:else}
					<svg
						viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
						width={chartWidth}
						height={CHART_HEIGHT}
						class="block"
						role="img"
						aria-label="Per-kill multiplier sparkline, last {trimmed.length} kills"
					>
						<!-- Y axis line -->
						<line
							x1={PAD_L}
							x2={PAD_L}
							y1={PAD_T}
							y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 90%, transparent)"
							stroke-width="1"
						/>
						<!-- X axis line -->
						<line
							x1={PAD_L}
							x2={plotRight}
							y1={plotBottom}
							y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 90%, transparent)"
							stroke-width="1"
						/>

						<!-- Dashed reference line at 1× -->
						<line
							x1={PAD_L}
							x2={plotRight}
							y1={oneLineY}
							y2={oneLineY}
							stroke="color-mix(in oklab, var(--color-border-bright) 70%, transparent)"
							stroke-width="1"
							stroke-dasharray="3 4"
						/>

						<!-- Y axis labels: top (current chart ceiling) and 1× -->
						<text
							x={PAD_L - 6}
							y={PAD_T + 3}
							text-anchor="end"
							font-size="10"
							font-weight="500"
							letter-spacing="0.06em"
							fill="var(--color-text-secondary)"
							font-family="inherit"
						>{formatMultiplier($yMax)}</text>
						<text
							x={PAD_L - 6}
							y={oneLineY + 3}
							text-anchor="end"
							font-size="10"
							font-weight="500"
							letter-spacing="0.06em"
							fill="var(--color-text-tertiary)"
							font-family="inherit"
						>1×</text>

						<!-- Connecting line — neutral, lets the dot colours carry the signal -->
						{#if pathD}
							<path
								d={pathD}
								fill="none"
								stroke="color-mix(in oklab, var(--color-text-secondary) 55%, transparent)"
								stroke-width="1.25"
								stroke-linecap="round"
								stroke-linejoin="round"
							/>
						{/if}

						<!-- Dots (drawn after line so they sit on top) -->
						{#each trimmed as m, i (i + ':' + m)}
							{@const cx = xAt(i, trimmed.length, visibleCount)}
							{@const cy = yAt(m, $yMax)}
							{@const colour = dotColour(m)}
							{@const isLast = i === trimmed.length - 1}
							<g>
								{#if isLast}
									<circle {cx} {cy} r="6.5" fill={colour} opacity="0.22" />
								{/if}
								<circle
									{cx}
									{cy}
									r={isLast ? 3.5 : 2.75}
									fill={colour}
									stroke="var(--color-base)"
									stroke-width="0.75"
								>
									<title>{formatMultiplier(m)}</title>
								</circle>
							</g>
						{/each}
					</svg>
				{/if}
			</div>
		</div>

	</section>

	<div class="h-px bg-border/60" aria-hidden="true"></div>

	<!-- Chart 2: cumulative session P&L (per kill) -->
	<section class="flex flex-col gap-2">
		<span class="eyebrow">Cumulative session P&L</span>

		<div class="relative rounded-md border border-border/60 bg-base/40 p-2">
			<div class="relative" style="height: {CHART_HEIGHT}px;">
				{#if netTrimmed.length === 0}
					<svg
						viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
						width={chartWidth}
						height={CHART_HEIGHT}
						class="block"
						role="img"
						aria-hidden="true"
					>
						<!-- Muted axes -->
						<line
							x1={PAD_L} x2={PAD_L} y1={PAD_T} y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 35%, transparent)"
							stroke-width="1"
						/>
						<line
							x1={PAD_L} x2={plotRight} y1={plotBottom} y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 35%, transparent)"
							stroke-width="1"
						/>
						<!-- Muted dashed zero line -->
						<line
							x1={PAD_L} x2={plotRight}
							y1={placeholderNetZeroY} y2={placeholderNetZeroY}
							stroke="color-mix(in oklab, var(--color-border-bright) 30%, transparent)"
							stroke-width="1"
							stroke-dasharray="3 4"
						/>
						<!-- Muted connecting line -->
						<path
							d={placeholderNetPath}
							fill="none"
							stroke="color-mix(in oklab, var(--color-text-tertiary) 28%, transparent)"
							stroke-width="1.25"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
						<!-- Muted dots — single faint tone, no profit/loss split -->
						{#each PLACEHOLDER_NET as v, i (i)}
							<circle
								cx={placeholderX(i, PLACEHOLDER_NET.length)}
								cy={placeholderNetY(v)}
								r={2.5}
								fill="color-mix(in oklab, var(--color-text-tertiary) 45%, transparent)"
							/>
						{/each}
					</svg>
					<div class="absolute inset-x-0 top-[18%] flex items-center justify-center pointer-events-none">
						<p class="text-xs text-text-tertiary px-2.5 py-1 rounded-sm bg-base/60 backdrop-blur-[2px]">
							No active session
						</p>
					</div>
				{:else}
					<svg
						viewBox="0 0 {chartWidth} {CHART_HEIGHT}"
						width={chartWidth}
						height={CHART_HEIGHT}
						class="block"
						role="img"
						aria-label="Cumulative session profit and loss curve, last {netTrimmed.length} kills"
					>
						<!-- Y axis line -->
						<line
							x1={PAD_L}
							x2={PAD_L}
							y1={PAD_T}
							y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 90%, transparent)"
							stroke-width="1"
						/>
						<!-- X axis line -->
						<line
							x1={PAD_L}
							x2={plotRight}
							y1={plotBottom}
							y2={plotBottom}
							stroke="color-mix(in oklab, var(--color-border-bright) 90%, transparent)"
							stroke-width="1"
						/>

						<!-- Dashed reference line at 0 PED -->
						<line
							x1={PAD_L}
							x2={plotRight}
							y1={netZeroY}
							y2={netZeroY}
							stroke="color-mix(in oklab, var(--color-border-bright) 70%, transparent)"
							stroke-width="1"
							stroke-dasharray="3 4"
						/>

						<!-- Y axis labels: top, zero, bottom (only when bottom is truly negative) -->
						<text
							x={PAD_L - 6}
							y={PAD_T + 3}
							text-anchor="end"
							font-size="10"
							font-weight="500"
							letter-spacing="0.06em"
							fill="var(--color-text-secondary)"
							font-family="inherit"
						>{formatPedSigned($netMaxTween)}</text>
						<text
							x={PAD_L - 6}
							y={netZeroY + 3}
							text-anchor="end"
							font-size="10"
							font-weight="500"
							letter-spacing="0.06em"
							fill="var(--color-text-tertiary)"
							font-family="inherit"
						>0</text>
						{#if $netMinTween < -0.01}
							<text
								x={PAD_L - 6}
								y={plotBottom + 3}
								text-anchor="end"
								font-size="10"
								font-weight="500"
								letter-spacing="0.06em"
								fill="var(--color-text-secondary)"
								font-family="inherit"
							>{formatPed($netMinTween)}</text>
						{/if}

						<!-- Connecting line — neutral, lets dot colours carry the sign signal -->
						{#if netPathD}
							<path
								d={netPathD}
								fill="none"
								stroke="color-mix(in oklab, var(--color-text-secondary) 55%, transparent)"
								stroke-width="1.25"
								stroke-linecap="round"
								stroke-linejoin="round"
							/>
						{/if}

						<!-- Dots -->
						{#each netTrimmed as v, i (i + ':' + v)}
							{@const cx = xAt(i, netTrimmed.length, visibleCount)}
							{@const cy = netYAt(v, $netMinTween, $netMaxTween)}
							{@const colour = netDotColour(v)}
							{@const isLast = i === netTrimmed.length - 1}
							<g>
								{#if isLast}
									<circle {cx} {cy} r="6.5" fill={colour} opacity="0.22" />
								{/if}
								<circle
									{cx}
									{cy}
									r={isLast ? 3.5 : 2.75}
									fill={colour}
									stroke="var(--color-base)"
									stroke-width="0.75"
								>
									<title>{formatPedSigned(v)} PED</title>
								</circle>
							</g>
						{/each}
					</svg>
				{/if}
			</div>
		</div>

	</section>
</div>
