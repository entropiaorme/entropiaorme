<script lang="ts">
	import { highlightMatches } from '$lib/view/search';

	/** Text with the parts a search query matched brought forward. */
	let { text, query }: { text: string; query: string } = $props();

	const segments = $derived(highlightMatches(text, query));
</script>

{#each segments as segment, i (i)}{#if segment.match}<mark
			class="bg-transparent font-semibold text-accent">{segment.text}</mark
		>{:else}{segment.text}{/if}{/each}
