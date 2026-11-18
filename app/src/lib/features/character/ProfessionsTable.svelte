<script lang="ts">
	import Pagination from '$lib/components/Pagination.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { type CharacterModel, PAGE_SIZE } from './characterModel.svelte';
	import { formatGain, formatProfLevel, gainColorClass } from './characterFormat';

	let { model }: { model: CharacterModel } = $props();
	const table = $derived(model.professionsTable);
</script>

<!-- Search -->
<SearchInput bind:value={table.search} placeholder="Search professions..." />

<div>
	<div class="overflow-x-auto">
		<table class="w-full text-sm">
			<thead>
				<tr class="border-b border-border">
					<th class="py-2 px-3 text-left eyebrow" aria-sort={table.sortKey === 'name' ? (table.sortDir === 'asc' ? 'ascending' : 'descending') : undefined}>
						<button type="button" class="inline-flex items-center gap-1 uppercase cursor-pointer transition-colors duration-[var(--duration-fast)] hover:text-text" onclick={() => table.setSort('name')}>Profession {#if table.sortKey === 'name'}<span class="text-accent">{table.sortDir === 'asc' ? '\u2191' : '\u2193'}</span>{/if}</button>
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
				</tr>
			</thead>
			<tbody>
				{#if table.pageRows.length === 0}
					<tr><td colspan="4" class="py-8 text-center text-text-tertiary">{model.loading ? 'Loading...' : 'No profession data'}</td></tr>
				{:else}
					{#each table.pageRows as prof}
						<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
							<td class="py-2.5 px-3 text-text">{prof.name}</td>
							<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{formatProfLevel(prof.anchorLevel)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(prof.gainSinceAnchor)}">{formatGain(prof.gainSinceAnchor)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums">{formatProfLevel(prof.level)}</td>
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
