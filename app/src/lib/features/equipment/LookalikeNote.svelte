<script lang="ts">
	// The one quiet line under a library entry the held-tool bar cannot tell
	// from others, asking, while more than one of them is on, to keep one.
	import type { LookalikeNote } from './toolReading';

	let { note }: { note: LookalikeNote | undefined } = $props();
</script>

{#if note}
	<p class="mt-0.5 text-xs text-text-tertiary" data-testid="lookalike-note">
		{note.text}
		{#if note.leaveOne}
			<span class="text-warning" data-testid="lookalike-leave-one">Leave one on.</span>
		{/if}
	</p>
{/if}
