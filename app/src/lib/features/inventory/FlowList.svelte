<script lang="ts">
	/**
	 * One side of an event's flow: what went in, or what came out, a line
	 * each, with its count and its TT.
	 */
	import type { FlowLine } from './accountActivity';
	import { formatPed, formatQuantity, isZeroTt } from '$lib/utils/format';

	let {
		label,
		lines,
		struck = false,
	}: {
		label: string;
		lines: FlowLine[];
		struck?: boolean;
	} = $props();
</script>

<div class="min-w-0">
	<p class="text-[0.625rem] font-medium uppercase tracking-wider text-text-tertiary">{label}</p>
	<ul class="mt-1 flex flex-col gap-0.5">
		{#each lines as line, index (`${line.name}:${index}`)}
			<li class="flex min-w-0 items-baseline gap-2 text-sm {struck ? 'line-through' : ''}">
				<span class="min-w-0 flex-1 truncate text-text" title={line.name}>
					{#if line.quantity !== null}<span class="tabular-nums text-text-secondary"
							>{formatQuantity(line.quantity)}</span
						>{' '}{/if}{line.name}
				</span>
				{#if !isZeroTt(line.tt)}
					<span class="shrink-0 text-xs tabular-nums text-text-tertiary">{formatPed(line.tt)} PED</span>
				{/if}
			</li>
		{/each}
	</ul>
</div>
