<script lang="ts">
	import Badge from '$lib/components/Badge.svelte';
	import Pagination from '$lib/components/Pagination.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { formatPed } from '$lib/utils/format';
	import { type CharacterModel, PAGE_SIZE } from './characterModel.svelte';
	import { formatGain, gainColorClass } from './prospectModel.svelte';

	let { model }: { model: CharacterModel } = $props();
	const table = $derived(model.skillsTable);
</script>

<!-- Search -->
<SearchInput bind:value={table.search} placeholder="Search skills..." />

<!-- Category filter pills + table -->
{#if table.categories.length > 1}
	<div class="flex flex-wrap gap-1">
		<button
			type="button"
			class="filter-chip {table.category === null ? 'is-active' : ''}"
			onclick={() => (table.category = null)}
		>All</button>
		{#each table.categories as cat}
			<button
				type="button"
				class="filter-chip {table.category === cat ? 'is-active' : ''}"
				onclick={() => (table.category = cat)}
			>{cat}</button>
		{/each}
	</div>
{/if}

<div>
	<div class="overflow-x-auto">
		<table class="w-full text-sm" data-testid="character-skills-table">
			<thead>
				<tr class="border-b border-border">
					<th class="py-2 px-3 text-left eyebrow" aria-sort={table.sortKey === 'name' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('name')}>Skill {#if table.sortKey === 'name'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
					</th>
					<th class="py-2 px-3 text-right eyebrow" aria-sort={table.sortKey === 'anchorLevel' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('anchorLevel')}>Anchor {#if table.sortKey === 'anchorLevel'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
					</th>
					<th class="py-2 px-3 text-right eyebrow" aria-sort={table.sortKey === 'gainSinceAnchor' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('gainSinceAnchor')}>Gain {#if table.sortKey === 'gainSinceAnchor'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
					</th>
					<th class="py-2 px-3 text-right eyebrow" aria-sort={table.sortKey === 'level' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('level')}>Level {#if table.sortKey === 'level'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
					</th>
					<th class="py-2 px-3 text-left eyebrow">Rank</th>
					<th class="py-2 px-3 text-right eyebrow" aria-sort={table.sortKey === 'ttValue' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('ttValue')}>PES {#if table.sortKey === 'ttValue'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
					</th>
				</tr>
			</thead>
			<tbody>
				{#if table.pageRows.length === 0}
					<tr><td colspan="6" class="py-8 text-center text-text-tertiary">{model.loading ? 'Loading...' : 'No skills calibrated yet'}</td></tr>
				{:else}
					{#each table.pageRows as skill}
						<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
							<td class="py-2.5 px-3 text-text">{skill.name}</td>
							<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{skill.anchorLevel === null ? '\u2014' : skill.anchorLevel.toFixed(2)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(skill.gainSinceAnchor)}">{formatGain(skill.gainSinceAnchor)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums">{skill.level.toFixed(2)}</td>
							<td class="py-2.5 px-3"><Badge variant="neutral">{skill.rankName}</Badge></td>
							<td class="py-2.5 px-3 text-right tabular-nums">{formatPed(skill.ttValue)}</td>
						</tr>
					{/each}
				{/if}
			</tbody>
		</table>
	</div>

	<!-- Pagination -->
	<Pagination
		bind:page={table.page}
		totalPages={table.totalPages}
		rangeLabel={`${table.page * PAGE_SIZE + 1}\u2013${Math.min((table.page + 1) * PAGE_SIZE, table.filtered.length)} of ${table.filtered.length}`}
	/>
</div>
