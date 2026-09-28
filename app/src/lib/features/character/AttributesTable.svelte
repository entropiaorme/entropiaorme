<script lang="ts">
	import type { CharacterModel } from './characterModel.svelte';
	import { formatGain, gainColorClass } from './characterFormat';

	let { model }: { model: CharacterModel } = $props();
</script>

<div>
	<div class="overflow-x-auto">
		<table class="w-full text-sm">
			<thead>
				<tr class="border-b border-border">
					<th class="py-2 px-3 text-left eyebrow">Attribute</th>
					<th class="py-2 px-3 text-right eyebrow">Anchor</th>
					<th class="py-2 px-3 text-right eyebrow">Gain</th>
					<th class="py-2 px-3 text-right eyebrow">Level</th>
				</tr>
			</thead>
			<tbody>
				{#if model.attributes.length === 0}
					<tr><td colspan="4" class="py-8 text-center text-text-tertiary">{model.loading ? 'Loading...' : 'No attributes calibrated yet'}</td></tr>
				{:else}
					{#each model.attributes as attr}
						<tr class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors">
							<td class="py-2.5 px-3 text-text">{attr.name}</td>
							<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">{attr.anchorLevel === null ? '\u2014' : attr.anchorLevel.toFixed(2)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums {gainColorClass(attr.gainSinceAnchor)}">{formatGain(attr.gainSinceAnchor)}</td>
							<td class="py-2.5 px-3 text-right tabular-nums">{attr.level.toFixed(2)}</td>
						</tr>
					{/each}
				{/if}
			</tbody>
		</table>
	</div>
</div>
