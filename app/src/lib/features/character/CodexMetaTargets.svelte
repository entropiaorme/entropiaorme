<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Select from '$lib/components/Select.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { ALL_PLANETS, createCodexMetaTargetsModel } from './codexMetaTargetsModel.svelte';

	// The codex meta-target ranking: which species reaches its next meta
	// reward for the least PED cycled. Picking a species opens it in the
	// species view, where its ranks are claimed.
	let { onOpenSpecies }: { onOpenSpecies: (speciesName: string) => void } = $props();

	const model = createCodexMetaTargetsModel();
	$effect(() => {
		void model.load();
	});

	const wholePed = new Intl.NumberFormat('en-GB', { maximumFractionDigits: 0 });
	const codexPoints = new Intl.NumberFormat('en-GB', { maximumFractionDigits: 2 });
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<p class="text-sm text-text-secondary">
			{#if model.result}
				<span class="tabular-nums font-medium text-text">{model.result.metasEarned}</span>
				of <span class="tabular-nums">{model.result.metaCap}</span> metas earned
			{:else}
				Next meta rewards, cheapest first
			{/if}
		</p>
		<div class="flex items-center gap-2">
			<label for="codex-meta-planet" class="sr-only">Planet</label>
			<Select
				id="codex-meta-planet"
				class="w-48"
				value={model.planet}
				disabled={model.loading || model.planets.length === 0}
				onchange={(e) => model.setPlanet(e.currentTarget.value)}
			>
				<option value={ALL_PLANETS}>All planets</option>
				{#each model.planets as planet (planet)}
					<option value={planet}>{planet}</option>
				{/each}
			</Select>
			<InfoTip label="How this list is ranked">
				<p class="text-xs font-semibold leading-relaxed text-text">Cheapest next meta first</p>
				<p class="mt-1 text-xs leading-relaxed text-text-secondary">
					Every species pays the same meta reward at ranks 5, 10, 15, 20 and 25, so the best next
					target is the one whose remaining ranks cost least. Progress within your current rank is not
					counted.
				</p>
				<p class="mt-2 text-xs leading-relaxed text-text-secondary">
					PED is an estimate of PED cycled at {model.result?.pedPerCost ?? 1.5} PED per codex point.
					Codex progress follows the work done on a species rather than its kill count; tracked play
					has ranged from about 1.2 to 2.8 PED per point depending on the gear used.
				</p>
			</InfoTip>
		</div>
	</div>

	{#if model.error}
		<ErrorNotice message={model.error} />
	{:else if model.loading}
		<div class="space-y-3" aria-busy="true">
			{#each { length: 8 } as _}
				<Skeleton class="h-5 w-full" />
			{/each}
		</div>
	{:else if model.rows.length === 0}
		<p class="py-4 text-sm text-text-tertiary">
			{model.planet === ALL_PLANETS
				? 'Every species has reached rank 25.'
				: `Every species on ${model.planet} has reached rank 25.`}
		</p>
	{:else}
		<ol aria-label="Species by the cost of their next meta reward, cheapest first">
			{#each model.rows as target, i (target.speciesName)}
				{@const hidden = model.isHidden(target.speciesName)}
				<li
					class="group flex items-center rounded-md transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover/60
						{hidden && i === model.visibleCount && i > 0 ? 'mt-2 border-t border-border/40 pt-2' : ''}"
				>
					<button
						type="button"
						class="flex min-w-0 flex-1 cursor-pointer items-baseline gap-3 py-2 pl-3 pr-2 text-left text-sm
							{hidden ? 'text-text-tertiary' : 'text-text-secondary group-hover:text-text'}"
						title="Open {target.speciesName}"
						onclick={() => onOpenSpecies(target.speciesName)}
					>
						<span
							class="w-7 shrink-0 text-right text-xs tabular-nums
								{hidden ? 'text-transparent' : i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text-tertiary'}"
							aria-hidden={hidden}
						>
							{hidden ? '' : i + 1}
						</span>
						<span class="min-w-0 flex-1 truncate">
							<span class={hidden ? '' : 'text-text'}>{target.speciesName}</span>
							{#if model.planet === ALL_PLANETS && target.planets.length > 0}
								<span class="ml-2 text-xs text-text-tertiary">{target.planets.join(', ')}</span>
							{/if}
						</span>
						<span class="w-24 shrink-0 text-right text-xs tabular-nums text-text-tertiary">
							Rank {target.currentRank} &rarr; {target.targetRank}
						</span>
						<span
							class="w-28 shrink-0 text-right tabular-nums {hidden ? 'opacity-60' : 'text-text'}"
							title="{codexPoints.format(target.cost)} codex points"
						>
							&asymp; {wholePed.format(target.estimatedPed)}
							<span class="text-xs text-text-tertiary">PED</span>
						</span>
					</button>
					<span class="w-16 shrink-0 text-right">
						{#if hidden}
							<button
								type="button"
								class="linklet px-2"
								aria-label="Restore {target.speciesName} to the ranking"
								onclick={() => model.restore(target.speciesName)}
							>
								Restore
							</button>
						{:else}
							<button
								type="button"
								class="linklet invisible px-2 group-hover:visible group-focus-within:visible"
								aria-label="Hide {target.speciesName} from the ranking"
								title="Hide a species you cannot hunt"
								onclick={() => model.hide(target.speciesName)}
							>
								Hide
							</button>
						{/if}
					</span>
				</li>
			{/each}
		</ol>
	{/if}
</div>
