<script lang="ts">
	/**
	 * The guided two-point capture calibration: starting the flow arms
	 * the backend's Enter listener, so the user switches to the game,
	 * hovers each corner of the coordinate readout, and confirms with
	 * Enter without refocusing the app. This modal mirrors the flow's
	 * live state (polled while open, visibility-gated) and echoes the
	 * validation read on completion so a wrong region is caught on the
	 * spot. Closing the modal mid-flow cancels it; the persisted region
	 * is only replaced by a completed flow.
	 */
	import Button from '$lib/components/Button.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import type { CoordCalibrationStatus } from '$lib/api';
	import {
		cancelMapsCalibration,
		getMapsCalibrationStatus,
		startMapsCalibration,
	} from '$lib/api';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { formatGamePoint } from './coords';

	let { open = $bindable(false) }: { open?: boolean } = $props();

	let status = $state<CoordCalibrationStatus | null>(null);
	let failed = $state<string | null>(null);

	async function begin() {
		failed = null;
		try {
			status = await startMapsCalibration();
		} catch {
			failed = 'Coordinate capture is unavailable on this installation.';
		}
	}

	// Start the flow when the modal opens; cancel any in-flight flow when
	// it closes (completion sets the phase idle server-side, so cancel is
	// then a no-op).
	$effect(() => {
		if (open) {
			void begin();
			return useVisiblePoll(
				async () => {
					try {
						status = await getMapsCalibrationStatus();
					} catch {
						// The poll retries on its next tick.
					}
				},
				{ intervalMs: 600, immediate: false },
			);
		}
		status = null;
		void cancelMapsCalibration().catch(() => {});
		return undefined;
	});

	const phase = $derived(status?.phase ?? 'idle');
	const validation = $derived(status?.lastValidation ?? null);
	const validationText = $derived.by(() => {
		if (!validation) return null;
		if (validation.status === 'read') {
			const point = formatGamePoint({ lon: validation.lon ?? 0, lat: validation.lat ?? 0 });
			return `We read: ${point}.`;
		}
		if (validation.status === 'unreadable') {
			return `The calibrated region did not read as coordinates (saw: "${validation.rawText ?? ''}").`;
		}
		if (validation.status === 'captureFailed') {
			return 'The screen could not be captured on this system.';
		}
		if (validation.status === 'engineUnavailable') {
			return 'The text recogniser is unavailable, so the region could not be verified.';
		}
		return 'The calibrated region could not be verified.';
	});
</script>

<Modal bind:open title="Calibrate coordinate capture">
	<div class="space-y-4 text-sm text-text-secondary">
		{#if failed}
			<p>{failed}</p>
			<div class="flex justify-end">
				<Button variant="ghost" onclick={() => (open = false)}>Close</Button>
			</div>
		{:else if phase === 'awaitTopLeft'}
			<p class="text-text">
				Step 1 of 2: switch to the game and find the position readout below the radar
				(<strong>LON</strong> on the left, <strong>LAT</strong> on the right). Hover the
				mouse over the <strong>top-left corner of LON</strong> (including the label is
				fine) and press
				<kbd class="rounded border border-border bg-surface px-1">Enter</kbd>.
			</p>
			<p>The app stays open; you do not need to switch back between steps.</p>
			<div class="flex justify-end">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			</div>
		{:else if phase === 'awaitBottomRight'}
			<p class="text-text">
				Step 2 of 2: hover over the
				<strong>bottom-right corner of the LAT digits</strong> (just past the last digit)
				and press
				<kbd class="rounded border border-border bg-surface px-1">Enter</kbd>. Draw the
				box around both numbers in one strip; the app finds the gap between them.
			</p>
			<div class="flex justify-end">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			</div>
		{:else}
			{#if validation && validationText}
				<p class="text-text">{validationText}</p>
				{#if validation.status === 'read'}
					<p>
						If those are your current in-game coordinates, the capture region is set.
						Otherwise recalibrate with the corners tighter around the numbers.
					</p>
				{:else}
					<p>
						The region was saved, but its first read failed. Recalibrate with the corners
						tighter around the two numbers, and check the radar is visible.
					</p>
				{/if}
				<div class="flex justify-end gap-2">
					<Button variant="secondary" onclick={begin}>Recalibrate</Button>
					<Button onclick={() => (open = false)}>Done</Button>
				</div>
			{:else}
				<p>Preparing the calibration flow…</p>
			{/if}
		{/if}
	</div>
</Modal>
