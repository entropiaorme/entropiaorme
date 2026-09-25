<script lang="ts">
	/**
	 * The carried weapons' damage ranges on one axis: each weapon's regular
	 * hit band, and how far its criticals reach. These are the ranges weapon
	 * attribution checks every hit against, so an overlap is where only the
	 * hotbar can tell two weapons apart.
	 */
	import {
		axisTicks,
		formatRange,
		sharedRanges,
		type WeaponBand,
	} from './carriedWeapons';
	import type { Equipment } from '$lib/types';

	let {
		banded,
		bandless,
	}: {
		banded: WeaponBand[];
		bandless: Equipment[];
	} = $props();

	let hoverTip = $state<{ text: string; x: number; y: number } | null>(null);
	let chart: HTMLDivElement | null = $state(null);

	const ticks = $derived(axisTicks(Math.max(0, ...banded.map((band) => band.critMax))));
	const top = $derived(ticks[ticks.length - 1] || 1);
	const shared = $derived(sharedRanges(banded));

	function span(min: number, max: number): string {
		const left = (min / top) * 100;
		const width = Math.max(((max - min) / top) * 100, 1);
		return `left: ${left}%; width: ${width}%;`;
	}

	function showTip(event: PointerEvent, text: string) {
		const rect = chart?.getBoundingClientRect();
		if (!rect) return;
		hoverTip = { text, x: event.clientX - rect.left, y: event.clientY - rect.top };
	}
</script>

<section aria-labelledby="damage-ranges-heading" class="space-y-3">
	<div class="flex flex-wrap items-baseline justify-between gap-2">
		<h3 id="damage-ranges-heading" class="eyebrow">Damage ranges</h3>
		{#if banded.length > 0}
			<div class="flex items-center gap-3 text-[11px] text-text-tertiary" aria-hidden="true">
				<span class="flex items-center gap-1.5">
					<span class="h-1.5 w-4 rounded-full bg-accent"></span>Hit
				</span>
				<span class="flex items-center gap-1.5">
					<span class="h-3 w-4 rounded-full bg-accent/15"></span>Critical reach
				</span>
			</div>
		{/if}
	</div>

	{#if banded.length === 0}
		<p class="text-xs text-text-tertiary">
			Weapons on the hotbar or carried without a hotkey show their damage ranges here.
		</p>
	{:else}
		<div class="relative" bind:this={chart} data-guide-anchor="damage-ranges-chart">
			<div class="space-y-2" aria-hidden="true">
				{#each banded as band (band.id)}
					<div class="grid grid-cols-[9rem_1fr] items-center gap-3">
						<div class="flex min-w-0 items-center gap-1.5">
							<span
								class="flex h-4 w-4 shrink-0 items-center justify-center rounded text-[10px] font-semibold
									{band.slot ? 'bg-accent/15 text-accent' : 'bg-surface text-text-tertiary'}"
								title={band.slot ? `Hotbar slot ${band.slot}` : 'Carried without a hotkey'}
							>
								{band.slot ?? '·'}
							</span>
							<span class="truncate text-xs text-text-secondary" title={band.name}>{band.name}</span>
						</div>
						<div class="relative h-4">
							<div class="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-border/60"></div>
							<div
								role="presentation"
								class="absolute top-1/2 h-3 -translate-y-1/2 rounded-full bg-accent/15"
								style={span(band.min, band.critMax)}
								onpointerenter={(event) =>
									showTip(event, `${band.name} critical reach: ${formatRange(band.min, band.critMax)}`)}
								onpointermove={(event) =>
									showTip(event, `${band.name} critical reach: ${formatRange(band.min, band.critMax)}`)}
								onpointerleave={() => (hoverTip = null)}
							></div>
							<div
								role="presentation"
								class="absolute top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-accent"
								style={span(band.min, band.max)}
								onpointerenter={(event) =>
									showTip(event, `${band.name} hit: ${formatRange(band.min, band.max)}`)}
								onpointermove={(event) =>
									showTip(event, `${band.name} hit: ${formatRange(band.min, band.max)}`)}
								onpointerleave={() => (hoverTip = null)}
							></div>
						</div>
					</div>
				{/each}
				<div class="grid grid-cols-[9rem_1fr] gap-3">
					<span></span>
					<div class="relative h-4 text-[10px] tabular-nums text-text-tertiary">
						{#each ticks as tick, index (tick)}
							<span
								class="absolute {index === 0
									? ''
									: index === ticks.length - 1
										? '-translate-x-full'
										: '-translate-x-1/2'}"
								style={`left: ${(tick / top) * 100}%`}
							>
								{tick}
							</span>
						{/each}
					</div>
				</div>
			</div>

			{#if hoverTip}
				<div
					class="pointer-events-none absolute z-10 -translate-x-1/2 -translate-y-full whitespace-nowrap rounded-md border border-border-bright bg-surface-raised px-2 py-1 text-[11px] text-text shadow-lg"
					style={`left: ${hoverTip.x}px; top: ${hoverTip.y - 8}px;`}
				>
					{hoverTip.text}
				</div>
			{/if}

			<table class="sr-only">
				<caption>Damage ranges of the carried weapons</caption>
				<thead>
					<tr><th>Weapon</th><th>Hotbar slot</th><th>Hit</th><th>Critical reach</th></tr>
				</thead>
				<tbody>
					{#each banded as band (band.id)}
						<tr>
							<td>{band.name}</td>
							<td>{band.slot ?? 'None'}</td>
							<td>{formatRange(band.min, band.max)}</td>
							<td>up to {band.critMax.toFixed(1)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>

		{#if shared.length > 0}
			<ul class="space-y-1 text-xs text-text-secondary" data-testid="shared-ranges">
				{#each shared as range (`${range.first}|${range.second}`)}
					<li>
						<span class="text-warning">{range.first} and {range.second}</span> share
						<span class="tabular-nums">{formatRange(range.min, range.max)}</span>: a hit there is
						priced to the one on your hotbar, and left unpriced if neither is.
					</li>
				{/each}
			</ul>
		{/if}
	{/if}

	{#if bandless.length > 0}
		<p class="text-xs text-text-tertiary">
			No damage figure for {bandless.map((weapon) => weapon.name).join(', ')}: its hits
			follow the hotbar, and damage alone can't recognise it.
		</p>
	{/if}
</section>
