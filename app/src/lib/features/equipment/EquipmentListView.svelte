<script lang="ts">
	import { Button, Card, Divider, Skeleton } from '$lib/components';
	import { IconConsumables, IconHealing, IconWeapons } from '$lib/icons';
	import { formatPec } from './display';
	import type { LibraryModel } from './libraryModel.svelte';
	import WeaponRow from './WeaponRow.svelte';

	let { model }: { model: LibraryModel } = $props();
</script>

<!-- Equipment library -->
<div class="mb-4 flex items-center gap-3">
	<h2 class="text-lg font-semibold text-text">Weapons</h2>
	<span class="text-text-tertiary" aria-hidden="true">
		<IconWeapons />
	</span>
</div>

{#if model.loading}
	<div class="space-y-1" aria-busy="true" data-testid="equipment-pending">
		{#each [0, 1, 2] as row (row)}
			<Skeleton class="h-14 w-full rounded-md" />
		{/each}
	</div>
{:else if model.sortedEquipment.length === 0}
	<Card class="p-8">
		<div class="flex flex-col items-center text-center gap-3">
			<svg
				xmlns="http://www.w3.org/2000/svg"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="1.5"
				class="h-10 w-10 text-text-tertiary"
			>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					d="M11.42 15.17L17.25 21A2.652 2.652 0 0021 17.25l-5.877-5.877M11.42 15.17l2.496-3.03c.317-.384.74-.626 1.208-.766M11.42 15.17l-4.655 5.653a2.548 2.548 0 11-3.586-3.586l6.837-5.63m5.108-.233c.55-.164 1.163-.188 1.743-.14a4.5 4.5 0 004.486-6.336l-3.276 3.277a3.004 3.004 0 01-2.25-2.25l3.276-3.276a4.5 4.5 0 00-6.336 4.486c.091 1.076-.071 2.264-.904 2.95l-.102.085"
				/>
			</svg>
			<p class="text-sm text-text-secondary">Add your first weapon to enable automatic cost tracking.</p>
			<Button size="sm" onclick={() => model.openAddModal()}>Add Equipment</Button>
		</div>
	</Card>
{:else}
	<div class="space-y-1">
		{#each model.sortedEquipment as item (item.id)}
			<WeaponRow {model} {item} />
		{/each}
	</div>
{/if}

<!-- Consumables section -->
<Divider />
<div>
	<div class="mb-4 flex items-center gap-3">
		<h2 class="text-lg font-semibold text-text">Consumables</h2>
		<span class="text-text-tertiary" aria-hidden="true">
			<IconConsumables />
		</span>
	</div>

	{#if model.loading}
		<div class="space-y-1" aria-busy="true">
			{#each [0, 1] as row (row)}
				<Skeleton class="h-14 w-full rounded-md" />
			{/each}
		</div>
	{:else if model.consumables.length === 0}
		<p class="text-sm text-text-tertiary py-4">
			No consumables configured.
		</p>
	{:else}
		<div class="space-y-1">
			{#each model.consumables as item (item.id)}
				<div
					class="flex items-center gap-3 px-4 py-3 rounded-md hover:bg-surface-hover/50
						transition-colors duration-[var(--duration-fast)]"
				>
					<div class="shrink-0 h-8 w-8 rounded-md bg-surface flex items-center justify-center">
						<div class="h-2 w-2 rounded-full bg-warning"></div>
					</div>
					<div class="flex-1 min-w-0">
						<span class="text-sm font-medium text-text">{item.name}</span>
					</div>
					<button
						type="button" class="linklet linklet-danger shrink-0"
						aria-label="Remove {item.name}"
						onclick={() => model.removeEquipment(item.id, 'consumable')}
						title="Remove"
					>
						<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5" aria-hidden="true">
							<path d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z" />
						</svg>
					</button>
				</div>
			{/each}
		</div>
	{/if}
</div>

<!-- Healing tools section -->
<Divider />
<div>
	<div class="mb-4 flex items-center gap-3">
		<h2 class="text-lg font-semibold text-text">Healing Tools</h2>
		<span class="text-text-tertiary" aria-hidden="true">
			<IconHealing />
		</span>
	</div>

	{#if model.healingTools.length === 0}
		<p class="text-sm text-text-tertiary py-4">
			No healing tools configured.
		</p>
	{:else}
		<div class="space-y-1">
			{#each model.healingTools as tool (tool.id)}
				<div
					class="flex items-center gap-3 px-4 py-3 rounded-md hover:bg-surface-hover/50
						transition-colors duration-[var(--duration-fast)]"
				>
					<div class="shrink-0 h-8 w-8 rounded-md bg-surface flex items-center justify-center">
						<div class="h-2 w-2 rounded-full bg-positive"></div>
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-text">{tool.name}</span>
						</div>
						<div class="mt-0.5 text-xs text-text-tertiary">
							{#if tool.profile?.mode === 'compound'}
								Direct {tool.profile.directMin}–{tool.profile.directMax}, then {tool.profile.tickMin}–{tool.profile.tickMax} over time
							{:else if tool.profile?.mode === 'over_time'}
								{tool.profile.tickMin}–{tool.profile.tickMax} over {tool.profile.effectDurationSeconds}s
							{:else if tool.profile}
								Direct {tool.profile.directMin}–{tool.profile.directMax}
							{/if}
							{#if tool.reloadSeconds !== null}
								<span class="ml-2">{tool.reloadSeconds}s reload</span>
							{/if}
						</div>
					</div>
					<div class="text-right shrink-0">
						<span class="text-sm font-medium tabular-nums text-text">
							{formatPec(tool.costPerHeal)}
						</span>
						<span class="text-xs text-text-tertiary ml-0.5">PEC/heal</span>
					</div>
					<button
						type="button" class="linklet shrink-0"
						onclick={() => model.openEditModal(tool.id)}
					>
						Edit
					</button>
					<button
						type="button" class="linklet linklet-danger shrink-0"
						aria-label="Remove {tool.name}"
						onclick={() => model.removeEquipment(tool.id, 'healing')}
						title="Remove"
					>
						<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5" aria-hidden="true">
							<path d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z" />
						</svg>
					</button>
				</div>
			{/each}
		</div>
	{/if}
</div>

<!-- Harvesting tools section -->
<Divider />
<div>
	<div class="mb-4 flex items-center gap-3">
		<h2 class="text-lg font-semibold text-text">Harvesting Tools</h2>
	</div>

	{#if model.harvestingTools.length === 0}
		<p class="text-sm text-text-tertiary py-4">
			No harvesting tools configured.
		</p>
	{:else}
		<div class="space-y-1">
			{#each model.harvestingTools as tool (tool.id)}
				<div
					class="flex items-center gap-3 px-4 py-3 rounded-md hover:bg-surface-hover/50
						transition-colors duration-[var(--duration-fast)]"
				>
					<div class="shrink-0 h-8 w-8 rounded-md bg-surface flex items-center justify-center">
						<div class="h-2 w-2 rounded-full bg-accent"></div>
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2">
							<span class="text-sm font-medium text-text">{tool.name}</span>
							{#if tool.isLimited}
								<span class="text-xs text-text-tertiary">(L)</span>
							{/if}
						</div>
					</div>
					<div class="text-right shrink-0">
						<span class="text-sm font-medium tabular-nums text-text">
							{formatPec(tool.costPerUse)}
						</span>
						<span class="text-xs text-text-tertiary ml-0.5">PEC/swing</span>
					</div>
					<button
						type="button" class="linklet linklet-danger shrink-0"
						aria-label="Remove {tool.name}"
						onclick={() => model.removeEquipment(tool.id, 'tool')}
						title="Remove"
					>
						<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5" aria-hidden="true">
							<path d="M5.28 4.22a.75.75 0 00-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 101.06 1.06L8 9.06l2.72 2.72a.75.75 0 101.06-1.06L9.06 8l2.72-2.72a.75.75 0 00-1.06-1.06L8 6.94 5.28 4.22z" />
						</svg>
					</button>
				</div>
			{/each}
		</div>
	{/if}
</div>
