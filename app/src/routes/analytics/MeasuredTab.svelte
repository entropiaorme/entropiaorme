<script lang="ts">
	import { onMount } from 'svelte';
	import MeasuredSessions from '$lib/features/account/MeasuredSessions.svelte';
	import { createMeasuredSessionsModel } from '$lib/features/account/measuredSessionsModel.svelte';

	const model = createMeasuredSessionsModel();
	onMount(() => model.connect());
</script>

<MeasuredSessions {model} />
