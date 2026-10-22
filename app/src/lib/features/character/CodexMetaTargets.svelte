<script lang="ts">
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import Select from '$lib/components/Select.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import { formatOrdinal } from '$lib/utils/format';
	import { ALL_PLANETS, createCodexMetaTargetsModel } from './codexMetaTargetsModel.svelte';

	// The codex meta-target ranking: which species reaches its next meta
	// reward for the least PED cycled. Picking a species opens it in the
	// species view, where its ranks are claimed; its chevron opens the
	// stages left on it, for deciding how long to stay.
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
					Open a species' stages to weigh staying on it: each later meta's PED, the total from now,
					and where that meta would sit in this list. The other species do not move while you stay,
					so its place is how many cheaper metas you pass up.
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
		<div>
			<!-- Column labels sit outside the scroll region so they stay in view; the
			     reserved scrollbar gutter on both keeps them aligned with the rows. -->
			<div
				class="flex items-baseline overflow-hidden border-b border-border/40 pb-1.5 [scrollbar-gutter:stable]"
				aria-hidden="true"
			>
				<div class="flex min-w-0 flex-1 items-baseline gap-3 pl-3">
					<span class="w-7 shrink-0"></span>
					<span class="eyebrow min-w-0 flex-1">Species</span>
					<span class="eyebrow w-24 shrink-0 text-right">Next meta</span>
					<span class="eyebrow w-32 shrink-0 text-right">Est. PED cycled</span>
				</div>
				<span class="w-24 shrink-0"></span>
			</div>
			<ol
				class="max-h-[min(36rem,calc(100vh-16rem))] overflow-y-auto overscroll-contain pt-1 [scrollbar-gutter:stable]"
				aria-label="Species by the estimated PED cycled to reach their next meta reward, cheapest first"
			>
				{#each model.rows as target, i (target.speciesName)}
					{@const hidden = model.isHidden(target.speciesName)}
					{@const open = model.stagesOpen(target.speciesName)}
					{@const stagesId = `codex-meta-stages-${i}`}
					<li
						class={hidden && i === model.visibleCount && i > 0
							? 'mt-2 border-t border-border/40 pt-2'
							: ''}
					>
						<div
							class="group flex items-center rounded-md transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover/60"
						>
							<button
								type="button"
								class="flex min-w-0 flex-1 cursor-pointer items-baseline gap-3 py-2 pl-3 text-left text-sm
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
									class="w-32 shrink-0 text-right tabular-nums {hidden ? 'opacity-60' : 'text-text'}"
									title="{codexPoints.format(target.cost)} codex points"
								>
									&asymp; {wholePed.format(target.estimatedPed)}
									<span class="sr-only">PED cycled</span>
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
							<span class="flex w-8 shrink-0 justify-center pr-1">
								<button
									type="button"
									class="flex h-6 w-6 cursor-pointer items-center justify-center rounded text-text-tertiary
										transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover hover:text-text
										focus:outline-none focus-visible:text-text focus-visible:[box-shadow:var(--shadow-glow)]
										{open ? 'text-text-secondary' : ''}"
									aria-expanded={open}
									aria-controls={stagesId}
									aria-label="{open ? 'Hide' : 'Show'} the stages left on {target.speciesName}"
									title={open ? 'Hide stages' : 'Stages to rank 25'}
									onclick={() => model.toggleStages(target.speciesName)}
								>
									<svg
										class="h-4 w-4 transition-transform duration-[var(--duration-fast)] {open ? 'rotate-180' : ''}"
										viewBox="0 0 20 20"
										fill="currentColor"
										aria-hidden="true"
									>
										<path
											fill-rule="evenodd"
											d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
											clip-rule="evenodd"
										/>
									</svg>
								</button>
							</span>
						</div>
						{#if open}
							<table
								id={stagesId}
								class="mb-2 ml-13 mt-0.5 text-xs tabular-nums {hidden ? 'opacity-70' : ''}"
							>
								<caption class="sr-only">Meta stages left on {target.speciesName}</caption>
								<thead>
									<tr>
										<th scope="col" class="eyebrow pb-1 pr-6 text-left font-normal">Until</th>
										<th scope="col" class="eyebrow pb-1 pl-6 text-right font-normal">This stage</th>
										<th scope="col" class="eyebrow pb-1 pl-6 text-right font-normal">From now</th>
										<th scope="col" class="eyebrow pb-1 pl-6 text-right font-normal">
											<span title="Where this meta would sit in the list as it stands">List place</span>
										</th>
									</tr>
								</thead>
								<tbody>
									{#each model.ladder(target) as stage (stage.targetRank)}
										<tr>
											<th scope="row" class="py-0.5 pr-6 text-left font-normal text-text-secondary">
												Rank {stage.targetRank}
											</th>
											<td
												class="py-0.5 pl-6 text-right text-text-secondary"
												title="{codexPoints.format(stage.cost)} codex points"
											>
												&asymp; {wholePed.format(stage.estimatedPed)}
											</td>
											<td class="py-0.5 pl-6 text-right text-text">
												&asymp; {wholePed.format(stage.cumulativePed)}
											</td>
											<td
												class="py-0.5 pl-6 text-right
													{stage.place === 1 ? 'text-success' : stage.place <= 3 ? 'text-accent' : 'text-text-tertiary'}"
											>
												{formatOrdinal(stage.place)}
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{/if}
					</li>
				{/each}
			</ol>
		</div>
	{/if}
</div>
