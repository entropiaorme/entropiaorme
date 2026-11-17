<script lang="ts">
	// A session's measured costs item by item, grouped by the Cycled bucket
	// each was billed to, as the session's figures are: the weapon (ammo
	// burned, the weapon and what it carries), enhancers, harvesting,
	// healing, armour (its parts first: unlimited, each confirmed limited
	// set at its markup, and what no confirmed set holds), and consumables.
	// A line charged nothing says why. Overhead is listed with the overhead.
	import type { MeasuredCostDetail } from '$lib/api/commands.gen';
	import CostLineRow from './CostLineRow.svelte';
	import { armourPartLine, BUCKET_LABELS, cycledGroups } from './costLines';
	import type { CostPlacementsModel } from './costPlacementsModel.svelte';

	let {
		costs,
		placements = null,
	}: {
		costs: MeasuredCostDetail;
		placements?: CostPlacementsModel | null;
	} = $props();

	const groups = $derived(cycledGroups(costs));
	const parts = $derived(costs.armourParts.map(armourPartLine));
</script>

{#each groups as group (group.bucket)}
	<div data-testid="cost-group-{group.bucket}">
		<h4 class="eyebrow mb-1.5">{BUCKET_LABELS[group.bucket]}</h4>
		{#if group.bucket === 'armour' && parts.length > 0}
			<div class="mb-1.5 space-y-0.5" data-testid="armour-parts">
				{#each parts as part, index (`${index}-${part.label}`)}
					<div class="flex items-baseline justify-between gap-4 py-0.5">
						<span class="min-w-0 break-words text-text-secondary">
							{part.label} <span class="text-text-tertiary">{part.detail}</span>
						</span>
						<span
							class="shrink-0 tabular-nums {part.cost.title ? 'text-text' : 'text-text-tertiary'}"
							title={part.cost.title ?? undefined}>{part.cost.text}</span
						>
					</div>
				{/each}
			</div>
		{/if}
		<div class={group.bucket === 'armour' && parts.length > 0 ? 'border-l border-border/40 pl-3' : ''}>
			{#each group.lines as line, index (`${index}-${line.item}`)}
				<CostLineRow {line} {placements} />
			{/each}
		</div>
	</div>
{/each}
