<script lang="ts">
	// The one marker beside the hotbar's tool display while reading the tool
	// in hand off the screen is turned on but the screen is not being read.
	// The hotbar's display and its cues stand as they always do; the marker
	// opens a sub-window that says why.
	import SubWindowToggle from '$lib/components/overlay/SubWindowToggle.svelte';
	import { SCREEN_NOT_READ_MARKER } from './heldTool';

	let {
		line,
		open,
		onToggle,
	}: {
		/** Why the screen is not being read. */
		line: string;
		open: boolean;
		onToggle: (anchor: HTMLElement) => void | Promise<void>;
	} = $props();
</script>

<span class="flex items-center shrink-0" data-testid="screen-unread-facet">
	<SubWindowToggle {open} title={line} testId="screen-unread" {onToggle}>
		<span
			class="text-[9px] leading-tight uppercase tracking-wider whitespace-nowrap text-amber-300/90"
			data-testid="screen-unread-marker"
		>
			{SCREEN_NOT_READ_MARKER}
		</span>
	</SubWindowToggle>
</span>
