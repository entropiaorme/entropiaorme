<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import Select from '$lib/components/Select.svelte';
	import { formatPed } from '$lib/utils/format';
	import type { CharacterModel } from './characterModel.svelte';
	import type { OptimizerMode } from './optimizerModel.svelte';

	let { model }: { model: CharacterModel } = $props();
	const optimizer = $derived(model.optimizer);
	const prospect = $derived(model.prospect);

	const pathResult = $derived(optimizer.pathResult);
</script>

<div class="space-y-4">
	<!-- Mode toggle: Profession / HP -->
	<SegmentedControl
		options={[
			{ id: 'profession', label: 'Profession' },
			{ id: 'hp', label: 'HP' }
		]}
		active={optimizer.mode}
		onchange={(id) => {
			optimizer.mode = id as OptimizerMode;
			if (id === 'hp' && optimizer.hpSkills.length === 0 && !optimizer.hpLoading) optimizer.loadHpOptimizer();
		}}
	/>

	{#if optimizer.mode === 'profession'}
		<!-- Profession selector -->
		<div class="flex items-center gap-3">
			<label for="prof-select" class="text-sm text-text-secondary whitespace-nowrap">Profession</label>
			<Select
				id="prof-select"
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

		<!-- Path view -->
		<div class="flex items-center gap-3">
			<label for="path-target" class="text-sm text-text-secondary whitespace-nowrap">Target Level</label>
			<Input
				id="path-target"
				type="number"
				min="1"
				step="1"
				placeholder={optimizer.profLevel > 0 ? `Current: ${optimizer.profLevel.toFixed(2)}` : 'e.g. 50'}
				class="flex-1"
				bind:value={optimizer.pathTargetInput}
				onkeydown={(e) => { if (e.key === 'Enter' && optimizer.selectedProfession && optimizer.pathTargetInput) optimizer.loadPathOptimizer(); }}
			/>
			<Button
				onclick={optimizer.loadPathOptimizer}
				disabled={optimizer.pathLoading || !optimizer.selectedProfession || !optimizer.pathTargetInput || parseFloat(optimizer.pathTargetInput) <= 0 || parseFloat(optimizer.pathTargetInput) <= optimizer.profLevel}
			>
				{#snippet children()}Calculate{/snippet}
			</Button>
		</div>

		{#if !optimizer.selectedProfession || !optimizer.pathTargetInput}
			<p class="text-sm text-text-tertiary py-4 text-center">Select a profession and target level to see the cheapest skills to level.</p>
		{:else if optimizer.pathLoading}
				<p class="text-sm text-text-tertiary py-4 text-center">Calculating optimal path...</p>
			{:else if pathResult}
				{#if pathResult.professionLevelsGained === 0}
					<p class="text-sm text-text-tertiary py-4 text-center">Already at or above target level.</p>
				{:else}
					<div class="flex items-baseline gap-3 text-sm">
						<span class="text-text-secondary">Level</span>
						<span class="text-text tabular-nums font-medium">{pathResult.currentLevel.toFixed(2)}</span>
						<span class="text-text-tertiary">{'\u2192'}</span>
						<span class="text-accent tabular-nums font-medium">{pathResult.endLevel.toFixed(2)}</span>
						<span class="text-text-tertiary text-xs">(+{pathResult.professionLevelsGained.toFixed(2)} levels for {formatPed(pathResult.totalPed)} PED)</span>
					</div>

					{@const allocated = pathResult.allocations.filter(a => a.levelsToGain > 0)}
					{@const unallocated = pathResult.allocations.filter(a => a.levelsToGain === 0)}
					{#if allocated.length > 0}
						<div class="overflow-x-auto">
							<table class="w-full text-sm">
								<thead>
									<tr class="border-b border-border">
										<th class="py-2 px-3 text-left eyebrow">#</th>
										<th class="py-2 px-3 text-left eyebrow">Skill</th>
										<th class="py-2 px-3 text-right eyebrow">Weight</th>
										<th class="py-2 px-3 text-right eyebrow">Level</th>
										<th class="py-2 px-3 text-right eyebrow">+Levels</th>
										<th class="py-2 px-3 text-right eyebrow">New Level</th>
										<th class="py-2 px-3 text-right eyebrow">PES Cost</th>
										<th class="py-2 px-3 text-right eyebrow">%</th>
									</tr>
								</thead>
								<tbody>
									{#each allocated as alloc, i}
										<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
											<td class="py-2.5 px-3 text-text-tertiary tabular-nums">{i + 1}</td>
											<td class="py-2.5 px-3 text-text">{alloc.name}</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{alloc.weight}</td>
											<td class="py-2.5 px-3 text-right tabular-nums">
												{#if alloc.currentLevel > 0}
													{alloc.currentLevel.toLocaleString()}
												{:else}
													<span class="text-text-tertiary">{'\u2014'}</span>
												{/if}
											</td>
											<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">+{alloc.levelsToGain.toLocaleString()}</td>
											<td class="py-2.5 px-3 text-right tabular-nums">{alloc.newLevel.toLocaleString()}</td>
											<td class="py-2.5 px-3 text-right tabular-nums font-medium
												{i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text'}">
												{formatPed(alloc.pedCost)}
											</td>
											<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
												{pathResult.totalPed > 0 ? (alloc.pedCost / pathResult.totalPed * 100).toFixed(1) : '0.0'}%
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>

						<p class="text-xs text-text-tertiary">Optimal allocation distributes skill gains to minimise total PED. Skills ranked by investment size.</p>
					{/if}

					{#if unallocated.length > 0 || pathResult.excluded.length > 0}
						<div class="pt-2">
							<p class="eyebrow mb-2">Not included in path</p>
							<div class="flex flex-wrap gap-2">
								{#each unallocated as skill}
									<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
										<span class="text-text">{skill.name}</span>
										<span class="text-text-tertiary">Lv {skill.currentLevel.toLocaleString()}</span>
										<span class="text-text-tertiary">wt {skill.weight}</span>
									</div>
								{/each}
								{#each pathResult.excluded as skill}
									<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs opacity-60">
										<span class="text-text">{skill.name}</span>
										<span class="text-text-tertiary">wt {skill.weight}</span>
										<span class="text-warning text-[10px]">{skill.reason}</span>
									</div>
								{/each}
							</div>
						</div>
					{/if}

					{#if pathResult.attributes.length > 0}
						<div class="pt-2">
							<p class="eyebrow mb-2">Attributes (if offered as a reward)</p>
							<div class="flex flex-wrap gap-2">
								{#each pathResult.attributes as attr}
									<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
										<span class="text-text">{attr.name}</span>
										<span class="text-text-tertiary">Lv {attr.currentLevel}</span>
										<span class="text-accent tabular-nums font-medium">{'\u00d7'}{attr.contributionFactor}</span>
									</div>
								{/each}
							</div>
							<p class="text-xs text-text-tertiary mt-1.5">Contribution factor = weight {'\u00d7'} 20. Pick the highest when choosing an attribute reward.</p>
						</div>
					{/if}
				{/if}
			{/if}

	{:else}
		<!-- HP optimizer mode -->
		{#if optimizer.hpLoading}
			<p class="text-sm text-text-tertiary py-4 text-center">Loading...</p>
		{:else if optimizer.hpSkills.length > 0}
			<div class="flex items-baseline gap-3 text-sm">
				<span class="text-text-secondary">Current HP</span>
				<span class="text-text tabular-nums font-medium">{optimizer.hpCurrent.toFixed(1)}</span>
				<span class="text-text-tertiary text-xs">({optimizer.hpSkills.length} contributing skills)</span>
			</div>

			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border">
							<th class="py-2 px-3 text-left eyebrow">#</th>
							<th class="py-2 px-3 text-left eyebrow">Skill</th>
							<th class="py-2 px-3 text-right eyebrow">Level</th>
							<th class="py-2 px-3 text-right eyebrow">Lvl / HP</th>
							<th class="py-2 px-3 text-right eyebrow">PES / HP</th>
							<th class="py-2 px-3 text-right eyebrow">HP / PES</th>
						</tr>
					</thead>
					<tbody>
						{#each optimizer.hpSkills as skill, i}
							<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
								<td class="py-2.5 px-3 text-text-tertiary tabular-nums">{i + 1}</td>
								<td class="py-2.5 px-3 text-text">{skill.name}</td>
								<td class="py-2.5 px-3 text-right tabular-nums">
									{#if skill.currentLevel > 0}
										{skill.currentLevel.toLocaleString()}
									{:else}
										<span class="text-text-tertiary">{'\u2014'}</span>
									{/if}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.levelsPerHp.toLocaleString()}</td>
								<td class="py-2.5 px-3 text-right tabular-nums font-medium
									{i === 0 ? 'text-success' : i < 3 ? 'text-accent' : 'text-text'}">
									{formatPed(skill.pedPerHp)}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.hpPerPed.toFixed(4)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<p class="text-xs text-text-tertiary"><strong>PES / HP</strong> = cost of gaining +1 HP by levelling this skill alone from your current level. <strong>HP / PES</strong> = HP gained per 1 PES of skill. Lower cost is better.</p>

			<!-- Attributes -->
			{#if optimizer.hpAttributes.length > 0}
				<div class="pt-2">
					<p class="eyebrow mb-2">Attributes (if offered as a reward)</p>
					<div class="flex flex-wrap gap-2">
						{#each optimizer.hpAttributes as attr}
							<div class="flex items-center gap-2 bg-surface rounded-md px-3 py-1.5 text-xs">
								<span class="text-text">{attr.name}</span>
								<span class="text-text-tertiary">Lv {attr.currentLevel}</span>
								<span class="text-accent tabular-nums font-medium">{attr.levelsPerHp} lvl/HP</span>
							</div>
						{/each}
					</div>
					<p class="text-xs text-text-tertiary mt-1.5">Levels per HP accounts for the {'\u00d7'}20 attribute multiplier. Pick the lowest when choosing an attribute reward for HP.</p>
				</div>
			{/if}
		{:else}
			<p class="text-sm text-text-tertiary py-4 text-center">No skill data available. Scan your skills first.</p>
		{/if}
	{/if}
</div>
