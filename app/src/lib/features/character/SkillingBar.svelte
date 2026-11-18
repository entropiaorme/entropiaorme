<script lang="ts">
	// A hairline proportion bar: a row's share of the largest (or of a
	// whole), drawn under or beside the figure it illustrates.
	let {
		fraction,
		tone = 'accent',
		class: className = '',
	}: {
		/** 0 to 1; clamped. */
		fraction: number;
		tone?: 'accent' | 'muted';
		class?: string;
	} = $props();

	const width = $derived(Math.max(0, Math.min(1, Number.isFinite(fraction) ? fraction : 0)) * 100);
</script>

<span class="block h-1 overflow-hidden rounded-full bg-border/50 {className}" aria-hidden="true">
	<span
		class="block h-full rounded-full transition-[width] duration-[var(--duration-base)] ease-[var(--ease-out)]
			{tone === 'accent' ? 'bg-accent/80' : 'bg-text-tertiary/50'}"
		style="width: {width}%"
	></span>
</span>
