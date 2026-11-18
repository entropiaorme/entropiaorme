<script lang="ts">
	import ScanReviewKindPane from './ScanReviewKindPane.svelte';

	let {
		onComplete,
		saving = false,
	}: {
		onComplete: () => void;
		saving?: boolean;
	} = $props();
</script>

<div class="rounded-md border border-border bg-surface/40 p-3">
	<ScanReviewKindPane {onComplete} {saving} />
</div>
