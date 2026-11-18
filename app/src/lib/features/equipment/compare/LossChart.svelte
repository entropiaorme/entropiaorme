<script lang="ts">
	import {
		type ChartModel,
		formatHp,
		formatKillPec,
		formatSeconds,
		niceRangeTicks,
		niceTicks,
	} from './comparisonView';

	let {
		chart,
		emptyMessage,
		pinnedHp = null,
		onPin,
	}: {
		chart: ChartModel;
		emptyMessage?: string;
		/** The mob HP the table answers for, marked on the plot. */
		pinnedHp?: number | null;
		/** A click on the plot pins its HP. */
		onPin?: (hp: number) => void;
	} = $props();

	const HEIGHT = 260;
	const PAD_L = 48;
	const PAD_R = 16;
	const PAD_T = 18;
	const PAD_B = 28;
	const PLOT_H = HEIGHT - PAD_T - PAD_B;
	/** A drag shorter than this, in pixels, is a click, not a zoom box. */
	const MIN_DRAG = 8;
	const clipId = `compare-plot-${Math.random().toString(36).slice(2, 10)}`;

	let width = $state(640);
	const plotW = $derived(Math.max(1, width - PAD_L - PAD_R));

	// The visible window: the whole curve, or the box the player dragged.
	type Domain = { x0: number; x1: number; y0: number; y1: number };
	let zoom = $state<Domain | null>(null);
	const fullTicks = $derived(niceTicks(chart.lossMax));
	const domain = $derived<Domain>(
		zoom ?? { x0: 0, x1: chart.hpMax, y0: 0, y1: fullTicks[fullTicks.length - 1] || 1 },
	);
	const yTicks = $derived(zoom ? niceRangeTicks(zoom.y0, zoom.y1) : fullTicks);
	const xTicks = $derived(
		zoom
			? niceRangeTicks(zoom.x0, zoom.x1, 5)
			: [0, 0.25, 0.5, 0.75, 1].map((fraction) => fraction * chart.hpMax),
	);
	const hps = $derived(chart.series[0]?.points.map((point) => point.hp) ?? []);

	const xAt = (hp: number) => PAD_L + ((hp - domain.x0) / (domain.x1 - domain.x0)) * plotW;
	const yAt = (loss: number) =>
		PAD_T + PLOT_H - ((loss - domain.y0) / (domain.y1 - domain.y0)) * PLOT_H;
	const hpAt = (x: number) => domain.x0 + ((x - PAD_L) / plotW) * (domain.x1 - domain.x0);
	const lossAt = (y: number) => domain.y0 + ((PAD_T + PLOT_H - y) / PLOT_H) * (domain.y1 - domain.y0);
	const visible = (hp: number) => hp >= domain.x0 && hp <= domain.x1;

	function pathOf(points: { hp: number; loss: number }[]): string {
		// One point either side of the window keeps the line running to the
		// clip edge.
		const first = Math.max(0, points.findIndex((point) => point.hp >= domain.x0) - 1);
		let last = points.findIndex((point) => point.hp > domain.x1);
		last = last === -1 ? points.length - 1 : last;
		return points
			.slice(first, last + 1)
			.map(
				(point, index) =>
					`${index === 0 ? 'M' : 'L'}${xAt(point.hp).toFixed(1)},${yAt(point.loss).toFixed(1)}`,
			)
			.join(' ');
	}

	function svgPoint(event: PointerEvent, svg: SVGSVGElement): { x: number; y: number } {
		const rect = svg.getBoundingClientRect();
		const x = ((event.clientX - rect.left) / rect.width) * width;
		const y = ((event.clientY - rect.top) / rect.height) * HEIGHT;
		return {
			x: Math.min(Math.max(x, PAD_L), PAD_L + plotW),
			y: Math.min(Math.max(y, PAD_T), PAD_T + PLOT_H),
		};
	}

	// Hover and keyboard: the crosshair snaps to the nearest sampled HP in
	// view and the readout ranks every setup there.
	let active = $state<number | null>(null);
	function nearestIndex(x: number): number | null {
		if (hps.length === 0) return null;
		const hp = hpAt(x);
		let best = 0;
		for (let index = 1; index < hps.length; index += 1) {
			if (Math.abs(hps[index] - hp) < Math.abs(hps[best] - hp)) best = index;
		}
		return visible(hps[best]) ? best : null;
	}

	// Drag a box to zoom into it; a mostly horizontal drag keeps the
	// current loss range.
	let drag = $state<{ from: { x: number; y: number }; to: { x: number; y: number } } | null>(null);
	function onPointerDown(event: PointerEvent & { currentTarget: SVGSVGElement }) {
		if (event.button !== 0) return;
		const point = svgPoint(event, event.currentTarget);
		drag = { from: point, to: point };
		event.currentTarget.setPointerCapture(event.pointerId);
	}
	function onPointerMove(event: PointerEvent & { currentTarget: SVGSVGElement }) {
		const point = svgPoint(event, event.currentTarget);
		if (drag) drag = { ...drag, to: point };
		active = nearestIndex(point.x);
	}
	function onPointerUp() {
		if (!drag) return;
		const { from, to } = drag;
		drag = null;
		if (Math.abs(to.x - from.x) < MIN_DRAG) {
			const index = nearestIndex(to.x);
			if (index !== null) onPin?.(hps[index]);
			return;
		}
		const [left, right] = [Math.min(from.x, to.x), Math.max(from.x, to.x)];
		const tall = Math.abs(to.y - from.y) >= MIN_DRAG;
		const [top, bottom] = [Math.min(from.y, to.y), Math.max(from.y, to.y)];
		zoom = {
			x0: hpAt(left),
			x1: hpAt(right),
			y0: tall ? lossAt(bottom) : domain.y0,
			y1: tall ? lossAt(top) : domain.y1,
		};
		active = null;
	}
	const dragBox = $derived(
		drag && Math.abs(drag.to.x - drag.from.x) >= MIN_DRAG
			? {
					x: Math.min(drag.from.x, drag.to.x),
					w: Math.abs(drag.to.x - drag.from.x),
					y: Math.abs(drag.to.y - drag.from.y) >= MIN_DRAG ? Math.min(drag.from.y, drag.to.y) : PAD_T,
					h:
						Math.abs(drag.to.y - drag.from.y) >= MIN_DRAG
							? Math.abs(drag.to.y - drag.from.y)
							: PLOT_H,
				}
			: null,
	);

	function onKey(event: KeyboardEvent) {
		if (hps.length === 0) return;
		const inView = hps.flatMap((hp, index) => (visible(hp) ? [index] : []));
		if (inView.length === 0) return;
		if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
			event.preventDefault();
			const stride = event.shiftKey ? 10 : 1;
			const step = event.key === 'ArrowRight' ? stride : -stride;
			const [low, high] = [inView[0], inView[inView.length - 1]];
			const start = active ?? (step > 0 ? low - 1 : high + 1);
			active = Math.min(high, Math.max(low, start + step));
		} else if (event.key === 'Escape') {
			if (active !== null) active = null;
			else zoom = null;
		}
	}
	const readout = $derived.by(() => {
		if (active === null || !visible(hps[active])) return null;
		const index = active;
		return {
			hp: hps[index],
			// Cheapest first: the top row is the best setup at this HP.
			rows: chart.series
				.map((series) => ({ series, point: series.points[index] }))
				.filter((row) => row.point !== undefined)
				.sort((left, right) => left.point.loss - right.point.loss),
		};
	});
	const readoutLeft = $derived(readout ? xAt(readout.hp) : 0);
</script>

<div class="w-full">
	<div class="relative" bind:clientWidth={width}>
		<!-- The plot is a focusable readout: arrow keys step the crosshair,
		     as the pointer does; dragging a box zooms into it. -->
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<svg
			class="block w-full cursor-crosshair select-none touch-none outline-none focus-visible:ring-1 focus-visible:ring-accent/60 rounded"
			height={HEIGHT}
			viewBox="0 0 {width} {HEIGHT}"
			role="img"
			tabindex="0"
			aria-label="Expected loss per kill, PEC, against mob HP from {formatHp(domain.x0)} to {formatHp(domain.x1)}. Click to pin a mob HP; drag a box to zoom; use the arrow keys to read each HP; Escape resets the zoom."
			onpointerdown={onPointerDown}
			onpointermove={onPointerMove}
			onpointerup={onPointerUp}
			onpointercancel={() => (drag = null)}
			onpointerleave={() => {
				if (!drag) active = null;
			}}
			onkeydown={onKey}
			onblur={() => (active = null)}
		>
			<defs>
				<clipPath id={clipId}>
					<rect x={PAD_L} y={PAD_T - 6} width={plotW} height={PLOT_H + 6} />
				</clipPath>
			</defs>

			{#each yTicks as tick}
				<line
					x1={PAD_L}
					x2={width - PAD_R}
					y1={yAt(tick)}
					y2={yAt(tick)}
					stroke="var(--color-border)"
					stroke-width="1"
				/>
				<text x={PAD_L - 8} y={yAt(tick) + 3} text-anchor="end" class="fill-text-tertiary text-[10px] tabular-nums">
					{formatKillPec(tick)}
				</text>
			{/each}
			{#each xTicks as tick}
				<text x={xAt(tick)} y={HEIGHT - 8} text-anchor="middle" class="fill-text-tertiary text-[10px] tabular-nums">
					{formatHp(tick)}
				</text>
			{/each}
			<text x={PAD_L} y={10} class="fill-text-tertiary text-[10px]">PEC lost per kill</text>
			<text x={width - PAD_R} y={HEIGHT - 18} text-anchor="end" class="fill-text-tertiary text-[10px]">mob HP</text>

			<g clip-path="url(#{clipId})">
				{#each chart.series as series (series.index)}
					<path
						d={pathOf(series.points)}
						fill="none"
						stroke="var(--compare-series-{series.slot})"
						stroke-width="2"
						stroke-linejoin="round"
						stroke-linecap="round"
					/>
				{/each}

				<!-- The HP the table answers for -->
				{#if pinnedHp !== null && visible(pinnedHp)}
					<line
						x1={xAt(pinnedHp)}
						x2={xAt(pinnedHp)}
						y1={PAD_T}
						y2={PAD_T + PLOT_H}
						stroke="var(--color-accent)"
						stroke-width="1"
						stroke-dasharray="3 3"
					/>
				{/if}

				<!-- Where the cheaper setup changes -->
				{#each chart.markers.filter((marker) => visible(marker.hp)) as marker}
					<line
						x1={xAt(marker.hp)}
						x2={xAt(marker.hp)}
						y1={yAt(marker.loss) + 6}
						y2={PAD_T + PLOT_H}
						stroke="var(--color-text-tertiary)"
						stroke-width="1"
						opacity="0.5"
					/>
					<circle
						cx={xAt(marker.hp)}
						cy={yAt(marker.loss)}
						r="4"
						fill="var(--color-text)"
						stroke="var(--color-surface)"
						stroke-width="2"
					/>
					<text
						x={xAt(marker.hp)}
						y={yAt(marker.loss) - 9}
						text-anchor="middle"
						class="fill-text-secondary text-[10px] tabular-nums"
					>
						{formatHp(marker.hp)} HP
					</text>
				{/each}

				{#if readout && !drag}
					<line
						x1={readoutLeft}
						x2={readoutLeft}
						y1={PAD_T}
						y2={PAD_T + PLOT_H}
						stroke="var(--color-text-tertiary)"
						stroke-width="1"
					/>
					{#each readout.rows as row (row.series.index)}
						<circle
							cx={readoutLeft}
							cy={yAt(row.point.loss)}
							r="4"
							fill="var(--compare-series-{row.series.slot})"
							stroke="var(--color-surface)"
							stroke-width="2"
						/>
					{/each}
				{/if}
			</g>

			{#if pinnedHp !== null && visible(pinnedHp)}
				<text
					x={xAt(pinnedHp)}
					y={PAD_T - 5}
					text-anchor="middle"
					class="fill-accent text-[10px] font-medium tabular-nums">{formatHp(pinnedHp)}</text
				>
			{/if}

			{#if dragBox}
				<rect
					x={dragBox.x}
					y={dragBox.y}
					width={dragBox.w}
					height={dragBox.h}
					fill="var(--color-accent)"
					fill-opacity="0.08"
					stroke="var(--color-accent)"
					stroke-opacity="0.6"
					stroke-width="1"
				/>
			{/if}
		</svg>

		{#if chart.series.length === 0 && emptyMessage}
			<p
				class="pointer-events-none absolute inset-x-0 text-center text-sm text-text-tertiary"
				style="top: {PAD_T + PLOT_H / 2 - 10}px; padding-left: {PAD_L}px"
			>
				{emptyMessage}
			</p>
		{/if}

		{#if zoom}
			<button
				type="button"
				class="absolute top-1 right-1 z-20 flex h-6 w-6 items-center justify-center rounded text-text-tertiary hover:bg-surface-raised hover:text-text"
				aria-label="Reset zoom"
				title="Reset zoom"
				onclick={() => {
					zoom = null;
					active = null;
				}}
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
				</svg>
			</button>
		{/if}

		{#if readout && !drag}
			<div
				class="pointer-events-none absolute top-2 z-10 w-72 rounded-md border border-border/70 bg-surface-raised p-2.5 text-xs shadow-[var(--shadow-lg)]"
				style="left: {Math.min(Math.max(readoutLeft + 12, 0), Math.max(width - 300, 0))}px"
				role="status"
			>
				<p class="mb-1.5 text-text-tertiary">{formatHp(readout.hp)} HP mob</p>
				{#each readout.rows as row, rank (row.series.index)}
					<div class="py-0.5">
						<div class="flex items-center gap-2">
							<span class="w-3 shrink-0 text-right tabular-nums text-text-tertiary">{rank + 1}</span>
							<svg width="12" height="6" class="shrink-0" aria-hidden="true">
								<line
									x1="1"
									y1="3"
									x2="11"
									y2="3"
									stroke="var(--compare-series-{row.series.slot})"
									stroke-width="2"
									stroke-linecap="round"
								/>
							</svg>
							<span
								class="tabular-nums {rank === 0 && readout.rows.length > 1
									? 'font-semibold text-text'
									: 'text-text-secondary'}">{formatKillPec(row.point.loss)} PEC lost</span
							>
							<span class="truncate text-text-tertiary">{row.series.label}</span>
						</div>
						<p class="pl-10 tabular-nums text-text-tertiary">
							{formatKillPec(row.point.cycled)} PEC cycled · {formatSeconds(row.point.seconds)}
						</p>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
