<script lang="ts">
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import Card from '$lib/components/Card.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Select from '$lib/components/Select.svelte';
	import StatDisplay from '$lib/components/StatDisplay.svelte';
	import type { ProspectSliceType } from '$lib/types/analytics';
	import { formatPed, formatPercent } from '$lib/utils/format';
	import type { CharacterModel } from './characterModel.svelte';
	import { formatProspectHours } from './prospectModel.svelte';

	let { model }: { model: CharacterModel } = $props();
	const optimizer = $derived(model.optimizer);
	const prospect = $derived(model.prospect);

	const currentProspectOptions = $derived(prospect.currentOptions);
	const prospectResult = $derived(prospect.result);
</script>

<div class="space-y-4">
	<div class="flex items-center gap-3">
		<label for="prospect-prof-select" class="text-sm text-text-secondary whitespace-nowrap">Profession</label>
		<Select
			id="prospect-prof-select"
			class="flex-1"
			bind:value={optimizer.selectedProfession}
			onchange={() => { optimizer.loadOptimizer(optimizer.selectedProfession); optimizer.pathResult = null; prospect.result = null; }}
		>
			<option value="">Select a profession...</option>
			{#each model.professions as prof}
				<option value={prof.name}>{prof.name} (Lv {prof.level.toFixed(2)})</option>
			{/each}
		</Select>
	</div>

	<SegmentedControl
		options={[
			{ id: 'global', label: 'Global' },
			{ id: 'tag', label: 'Tag' },
			{ id: 'mob', label: 'Mob' },
			{ id: 'weapon', label: 'Weapon' }
		]}
		active={prospect.sliceType}
		onchange={(id) => {
			prospect.sliceType = id as ProspectSliceType;
			prospect.result = null;
		}}
	/>

	<div class="grid gap-3 md:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_minmax(0,0.8fr)_auto]">
		{#if prospect.sliceType !== 'global'}
			<Select
				bind:value={prospect.sliceValue}
				onchange={() => (prospect.result = null)}
			>
				<option value="" disabled selected={prospect.sliceValue === ''}>
					Select a {prospect.sliceType} sample...
				</option>
				{#each currentProspectOptions as option}
					<option value={option.value}>
						{option.label} ({option.sessions}s {'\u00b7'} {formatPed(option.cycledPed)} PED)
					</option>
				{/each}
			</Select>
		{:else}
			<div class="flex items-center rounded-md border border-border bg-surface px-3 py-2 text-sm text-text-secondary">
				All eligible tracked sessions
			</div>
		{/if}

		<Input
			type="number"
			min="1"
			step="0.01"
			placeholder={optimizer.profLevel > 0 ? `Target level (current ${optimizer.profLevel.toFixed(2)})` : 'Target level'}
			bind:value={prospect.targetInput}
			oninput={() => (prospect.result = null)}
			onkeydown={(e) => { if (e.key === 'Enter') prospect.loadProspect(); }}
		/>

		<Input
			type="number"
			min="0"
			step="0.1"
			placeholder="Markup uplift %"
			bind:value={prospect.markupInput}
			oninput={() => (prospect.result = null)}
			onkeydown={(e) => { if (e.key === 'Enter') prospect.loadProspect(); }}
		/>

		<Button
			onclick={prospect.loadProspect}
			disabled={
				prospect.loading
				|| !optimizer.selectedProfession
				|| !prospect.targetInput
				|| parseFloat(prospect.targetInput) <= 0
				|| (prospect.sliceType !== 'global' && !prospect.sliceValue)
			}
		>
			{#snippet children()}Calculate{/snippet}
		</Button>
	</div>

	{#if prospect.sliceType !== 'global' && currentProspectOptions.length === 0}
		<p class="text-sm text-text-tertiary">No dominant {prospect.sliceType} samples are available yet.</p>
	{/if}

	{#if !optimizer.selectedProfession || !prospect.targetInput}
		<p class="text-sm text-text-tertiary py-4 text-center">Select a profession, choose a sample, and enter a target level to forecast the path.</p>
	{:else if prospect.loading}
		<p class="text-sm text-text-tertiary py-4 text-center">Calculating forecast...</p>
	{:else if prospectResult}
		{#if prospectResult.error}
			<Card class="p-4 border border-warning/30">
				<p class="text-sm font-medium text-warning">{prospectResult.error}</p>
			</Card>
		{:else}
			<div class="flex items-baseline gap-3 text-sm">
				<span class="text-text-secondary">Level</span>
				<span class="text-text tabular-nums font-medium">{prospectResult.currentLevel.toFixed(2)}</span>
				<span class="text-text-tertiary">{'\u2192'}</span>
				<span class="text-accent tabular-nums font-medium">{prospectResult.targetLevel.toFixed(2)}</span>
				<span class="text-text-tertiary text-xs">
					{prospectResult.sliceType === 'global' ? 'Global aggregate' : `${prospectResult.sliceType}: ${prospectResult.sliceValue}`}
				</span>
			</div>

			<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
				<StatDisplay label="Projected Cycled" value={formatPed(prospectResult.projectedCycledPed)} unit="PED" />
				<StatDisplay label="Projected Time" value={formatProspectHours(prospectResult.projectedHours)} />
				<StatDisplay label="Expected Loot TT" value={formatPed(prospectResult.expectedLootTt)} unit="PED" />
				<StatDisplay label="Baseline Net Burn" value={formatPed(prospectResult.expectedNetTtBurn)} unit="PED" />
			</div>

			{#if prospectResult.speculativeNetTtBurn !== null}
				<div class="grid gap-4 sm:grid-cols-2">
					<StatDisplay
						label="Speculative Loot"
						value={formatPed(prospectResult.speculativeLootTt ?? 0)}
						unit="PED"
						comparison={`with +${prospect.markupInput || '0'}% uplift`}
					/>
					<StatDisplay
						label="Speculative Net Burn"
						value={formatPed(prospectResult.speculativeNetTtBurn ?? 0)}
						unit="PED"
						comparison="manual markup uplift applied"
					/>
				</div>
			{/if}

			<Card class="p-4">
				<div class="flex flex-wrap gap-x-5 gap-y-2 text-sm">
					<div class="text-text-secondary">
						Sample: <span class="tabular-nums text-text">{prospectResult.sample.sessions}</span> sessions,
						<span class="tabular-nums text-text"> {prospectResult.sample.hours.toFixed(1)}h</span>,
						<span class="tabular-nums text-text"> {formatPed(prospectResult.sample.cycledPed)}</span> PED cycled
					</div>
					<div class="text-text-secondary">
						Loot rate: <span class="tabular-nums text-text">{formatPercent(prospectResult.sample.returnRate)}</span>
					</div>
					<div class="text-text-secondary">
						PES per 100 cycled: <span class="tabular-nums text-text">{(prospectResult.sample.pesPerPed * 100).toFixed(2)}</span>
					</div>
				</div>
			</Card>

			{#if prospectResult.warnings.length > 0}
				<Card class="p-4 border border-warning/30">
					<div class="space-y-1">
						{#each prospectResult.warnings as warning}
							<p class="text-sm text-warning">{warning}</p>
						{/each}
					</div>
				</Card>
			{/if}

			{#if prospectResult.rows.length > 0}
				<div class="overflow-x-auto">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-border">
								<th class="py-2 px-3 text-left eyebrow">Skill</th>
								<th class="py-2 px-3 text-right eyebrow">Weight</th>
								<th class="py-2 px-3 text-right eyebrow">Current</th>
								<th class="py-2 px-3 text-right eyebrow">Observed</th>
								<th class="py-2 px-3 text-right eyebrow">Projected Gain</th>
								<th class="py-2 px-3 text-right eyebrow">End Level</th>
								<th class="py-2 px-3 text-right eyebrow">Prof +Lv</th>
							</tr>
						</thead>
						<tbody>
							{#each prospectResult.rows as row}
								<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
									<td class="py-2.5 px-3 text-text">
										<div class="flex items-center gap-2">
											<span>{row.name}</span>
											{#if row.isAttribute}
												<Badge variant="neutral">Attribute</Badge>
											{/if}
											{#if !row.relevant}
												<Badge variant="neutral">Off-path</Badge>
											{/if}
										</div>
									</td>
									<td class="py-2.5 px-3 text-right tabular-nums">{row.weight > 0 ? row.weight : '\u2014'}</td>
									<td class="py-2.5 px-3 text-right tabular-nums">{row.currentLevel.toFixed(2)}</td>
									<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
										{#if row.isAttribute}
											{row.observedRate.toFixed(4)} lvl/PED
										{:else}
											{formatPercent(row.observedShare)}
										{/if}
									</td>
									<td class="py-2.5 px-3 text-right tabular-nums">{row.projectedGain.toFixed(2)}</td>
									<td class="py-2.5 px-3 text-right tabular-nums">{row.projectedEndLevel.toFixed(2)}</td>
									<td class="py-2.5 px-3 text-right tabular-nums {row.relevant ? 'text-accent font-medium' : 'text-text-tertiary'}">
										{row.professionContribution.toFixed(3)}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				<p class="text-xs text-text-tertiary">
					Baseline forecast uses tracked cycling versus loot TT only. Markup uplift is shown separately as a speculative adjustment.
				</p>
			{/if}
		{/if}
	{/if}
</div>
