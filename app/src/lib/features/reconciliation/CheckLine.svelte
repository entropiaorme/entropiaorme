<script lang="ts">
	/**
	 * One item whose quantity in the app differs from the account's: both
	 * quantities, the difference and what it is worth, and what happened to
	 * it. A large shortfall is marked quietly; it is never explained away.
	 */
	import { tick } from 'svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import SearchHighlight from '$lib/components/SearchHighlight.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { formatQuantity, formatTt } from '$lib/utils/format';
	import {
		choiceOf,
		choicesFor,
		conversionSentence,
		conversionTt,
		type Choice,
		type Draft,
		differenceTt,
		groupFiguresOf,
		hasExactTt,
		INEXACT_TT_REASON,
		isLargeShortfall,
		isShortfall,
		isShrapnel,
		isSurplus,
		type Line,
		remainderSentence,
		signedQuantity,
	} from './reconciliation';
	import ShrapnelSold from './ShrapnelSold.svelte';

	let {
		line,
		draft,
		query,
		selected,
		error = null,
		shrapnelError = null,
		ontoggle,
		onchoose,
		onsetapart,
		onshrapnelsold,
	}: {
		line: Line;
		draft: Draft;
		query: string;
		selected: boolean;
		/** Why the last change made on this line was refused. */
		error?: string | null;
		/** Why the last Shrapnel sold at TT was refused. */
		shrapnelError?: string | null;
		ontoggle: () => void;
		onchoose: (choice: Choice) => void;
		/** Take the line out of its conversion; the focus then moves to the
		 * line's own choice. */
		onsetapart: () => Promise<void>;
		onshrapnelsold: (quantity: number | null) => void;
	} = $props();

	const choice = $derived(choiceOf(line.disposition));
	const tt = $derived(differenceTt(line));
	const large = $derived(isLargeShortfall(line));
	const shortfall = $derived(isShortfall(line));
	/** A conversion's figures, said on the item that came out of it. */
	const figures = $derived(isSurplus(line) ? groupFiguresOf(draft, line) : null);
	const remainder = $derived(figures ? remainderSentence(figures) : null);
	const asset = $derived(
		line.assetQuantity > 0 ? `of which ${formatQuantity(line.assetQuantity)} held as an asset` : null,
	);
	let choiceControl = $state<HTMLElement | null>(null);

	async function setApart() {
		await onsetapart();
		await tick();
		const chosen =
			choiceControl?.querySelector<HTMLButtonElement>('button[aria-pressed="true"]') ??
			choiceControl?.querySelector<HTMLButtonElement>('button');
		chosen?.focus();
	}

	const uid = $props.id();
	const checkboxId = `check-line-${uid}`;
</script>

<li
	class="line grid grid-cols-[1.25rem_minmax(0,1fr)_auto] items-start gap-x-4 gap-y-2 py-3 sm:grid-cols-[1.25rem_minmax(0,1fr)_5.5rem_5.5rem_7.5rem_17rem]"
	aria-label={line.itemName}
>
	<input
		id={checkboxId}
		type="checkbox"
		class="mt-0.5 h-4 w-4 accent-[var(--color-accent)]"
		checked={selected}
		aria-label={`Select ${line.itemName}`}
		onchange={ontoggle}
	/>
	<div class="min-w-0">
		<label for={checkboxId} class="block truncate text-sm text-text" title={line.itemName}>
			<SearchHighlight text={line.itemName} {query} />
		</label>
		{#if choice === null}
			<p class="mt-0.5 text-xs text-text-tertiary">
				{conversionSentence(draft, line)}{#if figures}: {conversionTt(figures)}{/if}
			</p>
			{#if remainder}
				<p class="mt-0.5 text-xs text-text-tertiary">{remainder}</p>
			{/if}
		{:else if line.takenBack}
			<p class="mt-0.5 text-xs text-text-tertiary">You took back what your account recorded of it</p>
		{/if}
		<p class="mt-0.5 text-xs tabular-nums text-text-tertiary sm:hidden">
			{formatQuantity(line.appQuantity)} in the app{#if asset}, {asset}{/if},
			{formatQuantity(line.observedQuantity)} on your account
		</p>
	</div>
	<span class="hidden text-right tabular-nums sm:block">
		<span class="block text-sm text-text-secondary">{formatQuantity(line.appQuantity)}</span>
		{#if asset}
			<span class="block text-[11px] leading-tight text-text-tertiary">{asset}</span>
		{/if}
	</span>
	<span class="hidden text-right text-sm tabular-nums text-text-secondary sm:block">
		{formatQuantity(line.observedQuantity)}
	</span>
	<span class="text-right tabular-nums">
		<span class="flex items-center justify-end gap-1.5 text-sm {large ? 'font-medium text-warning' : shortfall ? 'text-text' : 'text-text-secondary'}">
			{#if large}
				<span class="h-1.5 w-1.5 rounded-full bg-warning" aria-hidden="true"></span>
				<span class="sr-only">A large shortfall:</span>
			{/if}
			<span>{signedQuantity(line.difference)}</span>
		</span>
		<span class="block text-[11px] text-text-tertiary">
			{#if tt === null}
				TT not known
			{:else}
				{tt.floor ? 'at least ' : ''}{formatTt(tt.ped)} PED
			{/if}
		</span>
	</span>
	<div class="col-span-full flex flex-col items-end gap-2 sm:col-span-1">
		{#if choice === null}
			<button type="button" class="text-xs font-medium text-accent hover:text-text" onclick={() => void setApart()}>
				Set apart
			</button>
		{:else}
			<span class="flex items-center gap-1.5" bind:this={choiceControl}>
				{#if isSurplus(line) && !hasExactTt(line)}
					<InfoTip label="How its TT is valued" align="right">
						<span class="block text-xs leading-relaxed text-text-secondary">{INEXACT_TT_REASON}</span>
					</InfoTip>
				{/if}
				<SegmentedControl
					options={choicesFor(line).map((option) => ({
						id: option.id,
						label: option.label,
						title: option.title,
					}))}
					active={choice}
					label={`What happened to ${line.itemName}`}
					onchange={(id) => {
						const picked = choicesFor(line).find((option) => option.id === id);
						if (picked) onchoose(picked.id);
					}}
				/>
			</span>
		{/if}
		{#if isShrapnel(line) && line.disposition.kind === 'shrapnelConverted'}
			<ShrapnelSold {line} error={shrapnelError} onchange={onshrapnelsold} />
		{/if}
	</div>
	{#if error}
		<p class="col-span-full text-right text-xs text-error" role="alert">{error}</p>
	{/if}
</li>

<style>
	/* A long check renders only the lines near the view. */
	.line {
		content-visibility: auto;
		contain-intrinsic-size: auto 3.5rem;
	}
</style>
