<script lang="ts">
	/**
	 * Capture without Enter: start a three-second countdown, hover the point
	 * in the game, and the capture lands at zero. For a machine that cannot
	 * read keys; Enter keeps working wherever it can be read.
	 */
	import Button from '$lib/components/Button.svelte';
	import { createCountdown } from './countdown.svelte';

	let { oncapture, disabled = false }: { oncapture: () => void | Promise<void>; disabled?: boolean } =
		$props();

	const countdown = createCountdown(3, () => oncapture());
	$effect(() => () => countdown.cancel());
</script>

{#if countdown.remaining === null}
	<Button variant="ghost" {disabled} onclick={() => countdown.start()}>Capture in 3 s</Button>
{:else}
	<Button variant="ghost" onclick={() => countdown.cancel()}>
		<span aria-live="polite">Capturing in {countdown.remaining}…</span>
	</Button>
{/if}
