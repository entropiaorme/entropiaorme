<script lang="ts">
	// A holdable library entry's on/off switch while the tool in hand is read
	// off the screen: an entry that is on is carried, and the reader matches
	// what the bar shows against it.
	import { Toggle } from '$lib/components';

	let {
		name,
		enabled,
		onchange,
	}: {
		name: string;
		enabled: boolean;
		onchange: (enabled: boolean) => void;
	} = $props();
</script>

<span class="shrink-0" data-testid="entry-switch">
	<Toggle
		checked={enabled}
		label={`${name} in use`}
		title={enabled
			? 'In use: matched when the screen shows it'
			: 'Not in use: kept, but not matched when the screen shows it'}
		{onchange}
	/>
</span>
