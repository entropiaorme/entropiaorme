<script lang="ts">
	import { type Snippet, tick } from 'svelte';
	import { parseAmount } from './comparisonView';
	import { groupTicks, overlapsAny, type SliderTick, spreadLabels } from './sliderTicks';

	/**
	 * A slider from 0 to an editable upper end, finer below a breakpoint:
	 * the first half of the track covers 0 to the breakpoint, the second
	 * half the rest. The upper end reads as a quiet label until clicked.
	 */
	let {
		label,
		heading,
		unit,
		value,
		max,
		breakpoint,
		significant = 2,
		step,
		decimals = 0,
		fixedMax = false,
		ticks = [],
		valueNote,
		info,
		note,
		onValue,
		onMax,
	}: {
		label: string;
		/** Shown in the label's place, e.g. a control that changes what is slid. */
		heading?: Snippet;
		unit: string;
		value: number;
		max: number;
		/** Below it the track is finer; none keeps the track linear. */
		breakpoint?: number;
		/** Significant figures a slid value is rounded to. */
		significant?: number;
		/** A fixed step to round slid values to, instead of significant figures. */
		step?: number;
		/** Decimal places the value shows. */
		decimals?: number;
		/** The upper end is a hard limit, not an editable label. */
		fixedMax?: boolean;
		/** Reference points marked on the track; clicking a label slides to it. */
		ticks?: SliderTick[];
		/** What the value stands for, read after it, e.g. the tick it sits on. */
		valueNote?: string;
		/** An info tip beside the label. */
		info?: Snippet;
		/** A quiet line under the track. */
		note?: Snippet;
		onValue: (value: number) => void;
		onMax?: (max: number) => void;
	} = $props();

	const RESOLUTION = 10_000;
	const SPLIT = 0.5;
	const split = $derived(breakpoint !== undefined && max > breakpoint);
	const bp = $derived(breakpoint ?? max);

	function toValue(position: number): number {
		const fraction = position / RESOLUTION;
		if (!split) return fraction * max;
		return fraction <= SPLIT
			? (fraction / SPLIT) * bp
			: bp + ((fraction - SPLIT) / (1 - SPLIT)) * (max - bp);
	}
	function toPosition(at: number): number {
		const clamped = Math.min(max, Math.max(0, at));
		if (!split) return (clamped / max) * RESOLUTION;
		const fraction =
			clamped <= bp
				? (clamped / bp) * SPLIT
				: SPLIT + ((clamped - bp) / (max - bp)) * (1 - SPLIT);
		return fraction * RESOLUTION;
	}
	function rounded(raw: number): number {
		if (raw <= 0) return 0;
		if (step !== undefined) return Math.min(max, Math.round(raw / step) * step);
		const unit = Math.max(1, 10 ** (Math.floor(Math.log10(raw)) - (significant - 1)));
		return Math.min(max, Math.round(raw / unit) * unit);
	}

	// Tick labels centre under their ticks once the row is measured, pushed
	// apart where they would collide; an end label they cover gives way.
	const groups = $derived(groupTicks(ticks, decimals));
	let rowWidth = $state(0);
	let labelWidths = $state<number[]>([]);
	let zeroWidth = $state(0);
	let maxWidth = $state(0);
	const boxes = $derived(
		groups.map((group, index) => ({
			centre: (toPosition(group.value) / RESOLUTION) * rowWidth,
			width: labelWidths[index] ?? 0,
		})),
	);
	const lefts = $derived(rowWidth > 0 ? spreadLabels(boxes, rowWidth) : null);
	const zeroCovered = $derived(lefts !== null && overlapsAny(0, zeroWidth, lefts, boxes));
	const maxCovered = $derived(
		lefts !== null && overlapsAny(rowWidth - maxWidth, rowWidth, lefts, boxes),
	);
	const onTick = (group: SliderTick) => Math.abs(group.value - value) < 10 ** -decimals / 2;

	const compact = new Intl.NumberFormat('en-GB', { notation: 'compact', maximumFractionDigits: 1 });
	const full = $derived(
		new Intl.NumberFormat('en-GB', {
			minimumFractionDigits: decimals,
			maximumFractionDigits: decimals,
		}),
	);

	// The value, like the upper end, is text until clicked, then a field
	// for an exact figure.
	let editingValue = $state(false);
	let draftValue = $state('');
	let valueInput = $state<HTMLInputElement | null>(null);
	async function editValue() {
		draftValue = String(value);
		editingValue = true;
		await tick();
		valueInput?.select();
	}
	function commitValue() {
		const parsed = parseAmount(draftValue);
		if (parsed !== null) {
			// A figure past the upper end moves the end out to meet it.
			if (parsed > max) onMax?.(parsed);
			onValue(parsed);
		}
		editingValue = false;
	}

	let editing = $state(false);
	let draftMax = $state('');
	let maxInput = $state<HTMLInputElement | null>(null);
	async function editMax() {
		draftMax = String(max);
		editing = true;
		await tick();
		maxInput?.select();
	}
	function commitMax() {
		const parsed = parseAmount(draftMax);
		if (parsed !== null && parsed > 0) onMax?.(parsed);
		editing = false;
	}
</script>

<div class="min-w-0">
	<div class="flex items-baseline justify-between gap-2">
		{#if heading}
			{@render heading()}
		{:else}
			<span class="flex items-center gap-1.5">
				<span class="eyebrow">{label}</span>
				{#if info}{@render info()}{/if}
			</span>
		{/if}
		{#if editingValue}
			<span class="flex items-baseline gap-1 text-sm font-semibold text-text">
				<input
					bind:this={valueInput}
					class="h-6 w-28 rounded border border-border bg-surface px-1.5 text-right text-sm font-semibold text-text tabular-nums focus:border-accent/60 focus:outline-none"
					bind:value={draftValue}
					aria-label="{label}, exact"
					onblur={commitValue}
					onkeydown={(event) => {
						if (event.key === 'Enter') commitValue();
						if (event.key === 'Escape') {
							draftValue = String(value);
							editingValue = false;
						}
					}}
				/>
				{unit}
			</span>
		{:else}
			<button
				type="button"
				class="cursor-text text-sm font-semibold tabular-nums text-text hover:text-accent"
				title="Type an exact figure"
				onclick={editValue}
			>
				{full.format(value)}{unit ? ` ${unit}` : ''}{#if valueNote}<span
						class="text-xs font-normal text-text-tertiary"> · {valueNote}</span
					>{/if}
			</button>
		{/if}
	</div>
	<input
		type="range"
		class="mt-1 block w-full cursor-pointer"
		style="accent-color: var(--color-accent)"
		min={0}
		max={RESOLUTION}
		step={1}
		value={toPosition(value)}
		aria-label={label}
		aria-valuetext="{full.format(value)}{unit ? ` ${unit}` : ''}{valueNote ? `, ${valueNote}` : ''}"
		oninput={(event) => onValue(rounded(toValue(Number(event.currentTarget.value))))}
	/>
	<div
		class="relative mt-0.5 h-5 text-[10px] tabular-nums text-text-tertiary"
		bind:clientWidth={rowWidth}
	>
		<span class="absolute left-0 top-0.5" class:invisible={zeroCovered} bind:offsetWidth={zeroWidth}>0</span>
		{#if split}
			<span class="absolute top-0.5 -translate-x-1/2" style="left: {SPLIT * 100}%">
				{compact.format(bp)}
			</span>
		{/if}
		{#each groups as group, index (group.label)}
			{@const at = (toPosition(group.value) / RESOLUTION) * 100}
			<!-- Each tick sits on the track; its label slides the value to it. -->
			<span
				class="pointer-events-none absolute -top-[13px] h-2 w-px -translate-x-1/2 bg-text-secondary"
				style="left: {at}%"
				aria-hidden="true"
			></span>
			<button
				type="button"
				class="absolute top-0.5 cursor-pointer whitespace-nowrap rounded-sm hover:text-text
					focus:outline-none focus-visible:[box-shadow:var(--shadow-glow)]
					{lefts === null ? '-translate-x-1/2' : ''} {onTick(group) ? 'font-medium text-text' : ''}"
				style="left: {lefts === null ? `${at}%` : `${lefts[index]}px`}"
				title="Slide to {group.title}"
				aria-label="Slide to {group.title}"
				bind:offsetWidth={labelWidths[index]}
				onclick={() => onValue(group.value)}
			>
				{group.label}
			</button>
		{/each}
		<span class="absolute right-0 top-0" class:invisible={maxCovered} bind:offsetWidth={maxWidth}>
			{#if fixedMax}
				<span class="leading-tight">{compact.format(max)}</span>
			{:else if editing}
				<input
					bind:this={maxInput}
					class="h-5 w-20 rounded border border-border bg-surface px-1.5 text-right text-[11px] text-text tabular-nums focus:border-accent/60 focus:outline-none"
					bind:value={draftMax}
					aria-label="{label} upper end"
					onblur={commitMax}
					onkeydown={(event) => {
						if (event.key === 'Enter') commitMax();
						if (event.key === 'Escape') {
							draftMax = String(max);
							editing = false;
						}
					}}
				/>
			{:else}
				<button
					type="button"
					class="cursor-pointer border-b border-dotted border-text-tertiary/60 leading-tight hover:text-text"
					title="Change the upper end"
					onclick={editMax}
				>
					{compact.format(max)}
				</button>
			{/if}
		</span>
	</div>
	{#if note}
		<div class="mt-0.5 text-xs text-text-tertiary">{@render note()}</div>
	{/if}
</div>
