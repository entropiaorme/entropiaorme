<script lang="ts">
	import { NO_DATA } from '$lib/utils/format';
	import { formatRoutePed } from './skillingFormat';

	// What a way to the goal costs over the cheapest one, in total and per
	// hour of its own play: the price of choosing it.
	let { premium, perHour }: { premium: number | null; perHour: number | null } = $props();
</script>

{#if premium === null}
	<span class="text-text-tertiary">{NO_DATA}</span>
{:else if premium <= 0}
	<span class="text-success">Cheapest</span>
{:else}
	<span class="text-text">+{formatRoutePed(premium)}</span>
	{#if perHour !== null}
		<span class="ml-1 text-xs text-text-tertiary">{formatRoutePed(perHour)}/h</span>
	{/if}
{/if}
