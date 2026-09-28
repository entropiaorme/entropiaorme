<svelte:options runes />

<script lang="ts">
	/** Thin Maps shell: the page heading over the shared maps surface. */
	import MapsSurface from '$lib/features/maps/MapsSurface.svelte';
</script>

<MapsSurface class="h-full px-4 pb-4 sm:px-6 sm:pb-6">
	{#snippet heading()}
		<div class="flex min-w-0 flex-col gap-1.5">
			<h1 class="text-xl font-semibold text-text tracking-tight">Maps</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="mt-0.5 text-sm text-text-secondary">
				Explore planet maps, record locations, and copy waypoints back into the game.
			</p>
		</div>
	{/snippet}
</MapsSurface>
