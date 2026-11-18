<svelte:options runes />

<script lang="ts">
	import CharacterView from './CharacterView.svelte';
</script>

<div class="px-6 pb-6 space-y-6">
	<header class="flex flex-col gap-1.5">
		<h1 class="text-xl font-semibold text-text tracking-tight">Character</h1>
		<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
		<p class="text-sm text-text-secondary mt-0.5">Skills, attributes, professions, and codex progress</p>
	</header>

	<CharacterView />
</div>
