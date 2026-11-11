<script lang="ts">
	// The creature detection toggle: on, the app reads the name over the
	// creature the player is locked on to while a session runs. Rendered
	// inside a block gated on the in-development channel.
	import type { MobDetectionCapture } from '$lib/api';
	import { Toggle } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { captureNote } from './mobDetectionSetting';

	let {
		enabled,
		capture,
		disabled,
		saved,
		onchange,
	}: {
		enabled: boolean;
		capture: MobDetectionCapture;
		disabled: boolean;
		saved: boolean;
		onchange: (checked: boolean) => void;
	} = $props();

	const note = $derived(enabled ? captureNote(capture) : null);
</script>

<div class="py-5 flex items-start justify-between gap-6">
	<div>
		<p class="flex items-center gap-2 text-sm text-text">
			Detect the creature I am fighting
			<InDevelopmentMark id="creature-detection" align="left" />
		</p>
		<p class="text-xs text-text-tertiary mt-0.5">
			While you hunt with the game in front, reads the name over the creature you are locked on to
			about once a second and records which creature each kill was. Kills it cannot tell keep your
			declared mob.
		</p>
		{#if note}
			<p class="text-xs text-warning mt-1">{note}</p>
		{/if}
		{#if saved}
			<p class="text-xs text-success mt-1">Saved</p>
		{/if}
	</div>
	<Toggle checked={enabled} {disabled} {onchange} label="Detect the creature I am fighting" />
</div>
