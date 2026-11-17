<script lang="ts">
	/**
	 * What a run came to, before it is recorded. The keys always pay back
	 * their TT, so the headline is the count and the markup the run might
	 * fetch: an estimate from the player's market data, never realised until
	 * something sells. Every item is kept: recycling one into Nanocubes is
	 * done in game, and your account readings record it when it happens.
	 * The hits lead. A run read from your account can ask for a later
	 * reading while boxes remain, in case the site had not shown them all.
	 */
	import Button from '$lib/components/Button.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import {
		formatPed,
		formatPercent,
		formatQuantity,
		formatTt,
		formatTtOrCount,
		isZeroTt,
		NO_TT_TITLE,
	} from '$lib/utils/format';
	import CompositionBar from './CompositionBar.svelte';
	import {
		boxCost,
		isHit,
		lineMarkup,
		ammoNote,
		paidAloneSentence,
		planTotal,
		summarise,
		UNEXPECTED_LABEL,
		untrackedBoxes,
	} from './openingDraft';
	import type { OpeningModel } from './openingModel.svelte';

	let { model }: { model: OpeningModel } = $props();

	const result = $derived(model.result!);
	const summary = $derived(summarise(result));
	const paidAlone = $derived(paidAloneSentence(result));
	const readAmmo = $derived(ammoNote(result));
	const cost = $derived(boxCost(model.plan));
	const untracked = $derived(untrackedBoxes(model.plan, model.holding));
	const blocking = $derived(model.balanceRefusal ?? model.planRefusal);
	let editingSources = $state(false);
	$effect(() => {
		if (model.planRefusal) editingSources = true;
	});

	function signed(value: number): string {
		return `${value >= 0 ? '+' : '−'}${formatPed(Math.abs(value))}`;
	}

	function sourcesSentence(): string {
		const parts: string[] = [];
		if (model.plan.loot > 0) parts.push(`${formatQuantity(model.plan.loot)} from your loot`);
		const assets = model.plan.assets.reduce((sum, asset) => sum + asset.quantity, 0);
		if (assets > 0) parts.push(`${formatQuantity(assets)} from your assets`);
		if (model.plan.bought > 0) parts.push(`${formatQuantity(model.plan.bought)} bought`);
		return parts.length > 0 ? parts.join(', ') : 'Not yet said';
	}

	function setPlan(patch: Partial<typeof model.plan>) {
		model.plan = { ...model.plan, ...patch };
	}

	function count(value: unknown): number {
		const parsed = Number(value);
		return Number.isFinite(parsed) ? parsed : 0;
	}

	/** How far the estimate goes towards what the boxes cost. */
	const coverage = $derived(cost > 0 ? summary.estimatedMarkup / cost : null);
</script>

<div class="space-y-10">
	<!-- The reveal -->
	<section aria-label="Opening result" class="reveal grid gap-8 sm:grid-cols-[auto_minmax(0,1fr)] sm:items-end">
		<div>
			<span class="eyebrow">Opened</span>
			<p class="mt-1 flex items-baseline gap-3">
				<span class="text-6xl font-extralight leading-none tabular-nums text-text">
					{formatQuantity(result.boxes)}
				</span>
				<span class="text-base text-text-secondary">{result.boxItem}</span>
			</p>
		</div>
		<div class="sm:justify-self-end sm:text-right">
			<span class="eyebrow inline-flex items-center gap-1.5">
				Estimated markup
				<InfoTip label="About the estimate">
					What the kept items and the Nanocubes might fetch above TT, from your market data.
					Nothing is realised until it sells, and the TT the keys paid back is never counted as
					gain.
				</InfoTip>
			</span>
			<p class="mt-1 text-4xl font-light leading-none tabular-nums {summary.estimatedMarkup > 0 ? 'text-positive' : 'text-text'}">
				{signed(summary.estimatedMarkup)}
				<span class="text-base text-text-tertiary">PED</span>
			</p>
			<p class="mt-2 text-xs text-text-tertiary">
				{#if summary.hits > 0}
					{summary.hits} {summary.hits === 1 ? 'hit' : 'hits'} worth more than cubes
				{:else}
					No hits this run
				{/if}
				{#if summary.unvalued > 0}
					&middot; {summary.unvalued} {summary.unvalued === 1 ? 'item' : 'items'} with no market reading
				{/if}
			</p>
		</div>
	</section>

	{#if coverage !== null}
		<section aria-label="Against what the boxes cost" class="space-y-2">
			<div class="flex flex-wrap items-baseline justify-between gap-2 text-sm">
				<span class="text-text-secondary">
					The boxes cost <span class="tabular-nums text-text">{formatPed(cost)} PED</span> above TT
				</span>
				<span class="text-xs tabular-nums {coverage >= 1 ? 'text-positive' : 'text-negative'}">
					{#if coverage >= 1}
						On course to clear it by {formatPed(summary.estimatedMarkup - cost)} PED
					{:else}
						{formatPed(cost - summary.estimatedMarkup)} PED short of it
					{/if}
				</span>
			</div>
			<div class="relative h-1.5 w-full rounded-full bg-surface-raised/60">
				<span
					class="meter absolute inset-y-0 left-0 rounded-full {coverage >= 1 ? 'bg-positive' : 'bg-negative/80'}"
					style="width: {Math.min(Math.max(coverage, 0), 1) * 100}%"
				></span>
			</div>
		</section>
	{/if}

	<section aria-label="What the keys paid back as" class="space-y-3">
		<div class="flex items-baseline justify-between">
			<span class="eyebrow">{formatPed(summary.keysTt)} PED of keys, paid back as</span>
		</div>
		<CompositionBar
			ammoPed={summary.ammoPed}
			cubedTt={summary.cubedTt}
			keptTt={summary.keptTt}
			nanocubes={summary.nanocubes}
		/>
		{#if paidAlone}
			<p class="text-xs text-text-tertiary">{paidAlone}</p>
		{/if}
	</section>

	<!-- The items -->
	<section aria-label="What came out" class="space-y-3">
		<div class="flex items-baseline justify-between gap-4">
			<span class="eyebrow">What came out</span>
			{#if model.runFollowUp && model.runFollowUp.boxesLeft > 0}
				<button
					type="button"
					class="text-xs text-accent hover:text-text disabled:opacity-50"
					disabled={model.run.phase === 'reading'}
					onclick={() => void model.checkNextReading()}
				>
					{model.run.phase === 'reading' ? 'Reading your account...' : 'Opened more than these? Check the next reading'}
				</button>
			{/if}
		</div>

		{#if result.lines.length === 0}
			<p class="py-4 text-sm text-text-tertiary">
				{#if result.possible.length > 0}
					Nothing besides ammo for certain: what else arrived is below.
				{:else}
					Only ammo this run: every key came back as Universal Ammo.
				{/if}
			</p>
		{:else}
			<ul class="divide-y divide-border/50 border-y border-border/50">
				{#each result.lines as line, index (line.itemName)}
					{@const hit = isHit(line)}
					{@const markup = lineMarkup(line)}
					<li
						class="row grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-6 gap-y-1 py-3 sm:grid-cols-[minmax(0,1fr)_5rem_6rem_7.5rem]"
						style="animation-delay: {Math.min(index, 12) * 35}ms"
					>
						<span class="flex min-w-0 items-center gap-2.5">
							<span
								class="h-1.5 w-1.5 shrink-0 rounded-full {hit ? 'bg-accent shadow-[var(--shadow-glow)]' : 'bg-transparent'}"
								aria-hidden="true"
							></span>
							<span class="truncate text-sm {hit ? 'font-medium text-text' : 'text-text-secondary'}">
								{line.itemName}
							</span>
							{#if hit}<span class="sr-only">(a hit)</span>{/if}
						</span>
						<span class="hidden text-right text-xs tabular-nums text-text-tertiary sm:block">
							&times;{formatQuantity(line.quantity)}
						</span>
						<span class="hidden text-right text-sm tabular-nums text-text-secondary sm:block">
							{formatPed(line.ttValue)} <span class="text-xs text-text-tertiary">TT</span>
						</span>
						<span class="hidden text-right sm:block">
							{#if markup === null}
								<span class="text-xs text-text-tertiary">no reading</span>
							{:else}
								<span class="block text-sm tabular-nums {markup > 0.005 ? 'text-positive' : 'text-text-secondary'}">
									{signed(markup)}
								</span>
								<span class="block text-[11px] tabular-nums text-text-tertiary">
									{#if line.estimate.markupPct !== null}
										{formatPercent(line.estimate.markupPct / 100)}
									{/if}
								</span>
							{/if}
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	{#if result.possible.length > 0}
		<section aria-label="Arrived at the same time" class="space-y-3">
			<span class="eyebrow">Arrived at the same time, maybe not from the boxes</span>
			<ul class="divide-y divide-border/50 border-y border-border/50">
				{#each result.possible as line, index (line.itemName)}
					<li class="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-4 gap-y-1 py-2.5">
						<input
							id={`possible-${index}`}
							type="checkbox"
							class="h-4 w-4 accent-[var(--color-accent)]"
							checked={line.included}
							onchange={(event) => model.setPossible(line.itemName, event.currentTarget.checked)}
						/>
						<label for={`possible-${index}`} class="flex min-w-0 items-baseline gap-3">
							<span class="truncate text-sm {line.included ? 'text-text' : 'text-text-secondary'}" title={line.itemName}>
								Add {line.itemName}
							</span>
							{#if isZeroTt(line.ttValue)}
								<span class="shrink-0 text-xs tabular-nums text-text-tertiary" title={NO_TT_TITLE}>
									{formatTtOrCount(line.ttValue, line.quantity)}
								</span>
							{:else}
								<span class="shrink-0 text-xs tabular-nums text-text-tertiary">
									&times;{formatQuantity(line.quantity)}, {formatTt(line.ttValue)} TT
								</span>
							{/if}
						</label>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	{#if readAmmo || result.notes.length > 0 || result.unexpected.length > 0 || result.skipped.length > 0}
		<section aria-label="Left out" class="space-y-1.5 text-xs text-text-tertiary">
			{#each result.notes as note (note)}
				<p class="text-warning">{note}</p>
			{/each}
			{#if readAmmo}
				<p class="text-warning">{readAmmo}</p>
			{/if}
			{#if result.unexpected.length > 0}
				<p>
					Left out of the opening:
					{#each result.unexpected as row, i (row.itemName)}
						<span class="text-text-secondary">{row.itemName}</span>
						({UNEXPECTED_LABEL[row.reason]}){i < result.unexpected.length - 1 ? ', ' : '.'}
					{/each}
				</p>
			{/if}
			{#if result.skipped.length > 0}
				<p>{result.skipped.length} pasted {result.skipped.length === 1 ? 'line was' : 'lines were'} not an item and were passed over.</p>
			{/if}
		</section>
	{/if}

	<!-- Where the boxes came from -->
	<section aria-label="Where the boxes came from" class="space-y-3">
		<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
			<span class="eyebrow">The boxes</span>
			<span class="text-sm text-text-secondary">{sourcesSentence()}</span>
			<button
				type="button"
				class="text-xs font-medium text-accent hover:text-text"
				aria-expanded={editingSources}
				onclick={() => (editingSources = !editingSources)}
			>
				{editingSources ? 'Done' : 'Change'}
			</button>
		</div>
		{#if untracked > 0}
			<p class="text-xs text-text-tertiary">
				{formatQuantity(untracked)} of these are more than the app saw you loot: what they bring in
				credits no activity.
			</p>
		{/if}

		{#if editingSources}
			<div class="max-w-2xl space-y-2.5 border-l border-border-bright/60 pl-4">
				<div class="grid grid-cols-[minmax(0,1fr)_6rem] items-center gap-3">
					<span class="text-sm text-text-secondary">
						From your loot
						<span class="block text-xs text-text-tertiary">
							{formatQuantity(model.holding?.lootQty ?? 0)} tracked; credits the hunts that looted them
						</span>
					</span>
					<Input
						type="number"
						min="0"
						step="1"
						align="right"
						aria-label="Boxes from your loot"
						bind:value={() => model.plan.loot, (next) => setPlan({ loot: count(next) })}
					/>
				</div>
				{#each model.plan.assets as asset, i (asset.id)}
					<div class="grid grid-cols-[minmax(0,1fr)_6rem] items-center gap-3">
						<span class="text-sm text-text-secondary">
							From an asset
							<span class="block text-xs text-text-tertiary">
								{formatQuantity(asset.held)} held at {formatPed(asset.markupPerBox)} PED a box{asset.divisible ? '' : ', whole only'}
							</span>
						</span>
						<Input
							type="number"
							min="0"
							step="1"
							align="right"
							aria-label="Boxes from this asset"
							bind:value={
								() => asset.quantity,
								(next) =>
									setPlan({
										assets: model.plan.assets.map((row, j) =>
											j === i ? { ...row, quantity: count(next) } : row,
										),
									})
							}
						/>
					</div>
				{/each}
				<div class="grid grid-cols-[minmax(0,1fr)_6rem] items-center gap-3">
					<span class="text-sm text-text-secondary">
						Bought for this opening
						<span class="block text-xs text-text-tertiary">no activity's; the run has to beat what they cost</span>
					</span>
					<Input
						type="number"
						min="0"
						step="1"
						align="right"
						aria-label="Boxes bought"
						bind:value={() => model.plan.bought, (next) => setPlan({ bought: count(next) })}
					/>
				</div>
				{#if model.plan.bought > 0}
					<div class="flex flex-wrap items-center justify-end gap-3">
						<span class="text-xs text-text-tertiary">Paid above TT</span>
						<Input
							type="number"
							min="0"
							step="0.01"
							align="right"
							class="w-28"
							aria-label="Markup paid for the bought boxes"
							bind:value={
								() => model.plan.boughtMarkup,
								(next) =>
									setPlan({
										boughtMarkup: next === null || String(next).trim() === '' ? null : count(next),
									})
							}
						/>
						<SegmentedControl
							options={[
								{ id: 'total', label: 'in total' },
								{ id: 'box', label: 'a box' },
							]}
							active={model.plan.boughtMarkupPer}
							label="Markup paid per"
							onchange={(id) => setPlan({ boughtMarkupPer: id as 'total' | 'box' })}
						/>
					</div>
				{/if}
				<p class="text-xs tabular-nums {planTotal(model.plan) === result.boxes ? 'text-text-tertiary' : 'text-warning'}">
					{formatQuantity(planTotal(model.plan))} of {formatQuantity(result.boxes)} boxes accounted for
				</p>
			</div>
		{/if}
	</section>

	<!-- Record -->
	<footer class="flex flex-wrap items-center gap-3 border-t border-border/50 pt-5">
		<Input type="date" class="w-40" aria-label="Opened on" bind:value={model.openedAt} />
		<span class="min-w-0 flex-1 text-xs {blocking || model.recordError ? 'text-error' : 'text-text-tertiary'}">
			{blocking ?? model.recordError ?? ''}
		</span>
		<Button variant="ghost" onclick={model.back}>Back</Button>
		<Button disabled={blocking !== null} loading={model.recording} onclick={() => void model.record()}>
			Record opening
		</Button>
	</footer>
</div>

<style>
	.reveal {
		animation: rise 420ms var(--ease-out) both;
	}
	.row {
		animation: rise 300ms var(--ease-out) both;
	}
	.meter {
		transition: width 320ms var(--ease-out);
	}
	@keyframes rise {
		from {
			opacity: 0;
			transform: translateY(6px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.reveal,
		.row {
			animation: none;
		}
		.meter {
			transition: none;
		}
	}
</style>
