<script lang="ts">
	import Button from '$lib/components/Button.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import {
		cancelRadarCalibration,
		confirmRadarCalibration,
		getRadarCalibrationStatus,
		getRadarGeometry,
		startRadarCalibration,
		type RadarCalibrationStatus,
	} from '$lib/api';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import CountdownCapture from './CountdownCapture.svelte';

	let { open = $bindable(false), oncomplete }: { open?: boolean; oncomplete: () => void } = $props();
	let phase = $state<RadarCalibrationStatus>('idle');
	let completed = $state(false);
	let error = $state<string | null>(null);

	// Without Enter: the countdown takes the point at the cursor at zero.
	async function confirmAtCursor() {
		try {
			phase = await confirmRadarCalibration();
		} catch {
			// The poll re-reads the flow on its next tick.
		}
	}

	async function begin() {
		completed = false;
		error = null;
		try {
			phase = await startRadarCalibration();
		} catch {
			error = 'Radar calibration is unavailable on this installation.';
		}
	}

	$effect(() => {
		if (!open) {
			void cancelRadarCalibration().catch(() => {});
			return;
		}
		void begin();
		return useVisiblePoll(async () => {
			try {
				const next = await getRadarCalibrationStatus();
				phase = next;
				if (next === 'idle' && await getRadarGeometry()) {
					completed = true;
					oncomplete();
				}
			} catch {
				// Retry on the next visible tick.
			}
		}, { intervalMs: 500, immediate: false });
	});
</script>

<Modal bind:open title="Calibrate radar guidance">
	<div class="space-y-4 text-sm text-text-secondary">
		{#if error}
			<p>{error}</p>
		{:else if phase === 'awaitCentre'}
			<p class="text-text">Step 1 of 2: lock the game radar north-up, hover its exact centre, then press <kbd class="rounded border border-border bg-surface px-1">Enter</kbd>.</p>
		{:else if phase === 'awaitNorthEdge'}
			<p class="text-text">Step 2 of 2: hover the top edge of the radar circle at north, then press <kbd class="rounded border border-border bg-surface px-1">Enter</kbd>.</p>
		{:else if completed}
			<p class="text-text">Radar guidance is calibrated.</p>
		{:else}
			<p>Preparing calibration…</p>
		{/if}
		{#if !completed && (phase === 'awaitCentre' || phase === 'awaitNorthEdge')}
			<p>Or start the capture and hover there before it ends.</p>
		{/if}
		<div class="flex justify-end gap-2">
			{#if !completed && (phase === 'awaitCentre' || phase === 'awaitNorthEdge')}
				<CountdownCapture oncapture={confirmAtCursor} />
			{/if}
			{#if completed}<Button onclick={() => (open = false)}>Done</Button>{:else}<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>{/if}
		</div>
	</div>
</Modal>
