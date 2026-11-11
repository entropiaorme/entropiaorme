<script lang="ts">
	/**
	 * The hand-drawn coordinate region: an escape hatch for a setup where
	 * the radar cannot be found, read only when it is not. Starting the flow
	 * arms the backend's Enter listener, so the player hovers each corner of
	 * the coordinates in the game and confirms with Enter without refocusing
	 * the app. When the region is drawn, the modal reads the coordinates
	 * once, the way a pin would be read, and shows what came back. Closing
	 * the modal mid-flow cancels it; the stored region is only replaced by a
	 * completed flow.
	 */
	import Button from '$lib/components/Button.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import type { CoordCalibrationStatus, CoordScanResult } from '$lib/api';
	import {
		cancelMapsCalibration,
		confirmMapsCalibration,
		getMapsCalibrationStatus,
		scanMapCoordinates,
		startMapsCalibration,
	} from '$lib/api';
	import CountdownCapture from './CountdownCapture.svelte';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { coordRefusalMessage } from './coordinateRead';
	import { formatGamePoint } from './coords';

	let {
		open = $bindable(false),
		planet,
	}: {
		open?: boolean;
		/** The planet shown, when its coordinates are known. */
		planet: string | null;
	} = $props();

	let status = $state<CoordCalibrationStatus | null>(null);
	let failed = $state<string | null>(null);
	let check = $state<{ planet: string; result: CoordScanResult } | null>(null);
	let checking = $state(false);
	// Each check's number: one that lands after the flow restarted or the
	// modal closed is dropped.
	let checkRequest = 0;
	// The flow in progress: whether its second corner was seen, and the
	// region stored when it began. Both corners can land between two polls,
	// so a new region also marks the flow complete.
	let drawing = false;
	let sawSecondCorner = false;
	let regionBefore: string | null = null;

	// Without Enter (a machine that cannot read keys): the countdown takes
	// the corner at the cursor when it reaches zero.
	async function confirmAtCursor() {
		try {
			adopt(await confirmMapsCalibration());
		} catch {
			// The poll re-reads the flow on its next tick.
		}
	}

	async function begin() {
		failed = null;
		check = null;
		checkRequest++;
		try {
			const started = await startMapsCalibration();
			drawing = true;
			sawSecondCorner = false;
			regionBefore = JSON.stringify(started.region);
			adopt(started);
		} catch {
			failed = 'Setting the region is unavailable on this installation.';
		}
	}

	// The flow completes when the second corner lands: the phase returns to
	// idle with a region stored.
	function adopt(next: CoordCalibrationStatus) {
		status = next;
		if (next.phase === 'awaitBottomRight') sawSecondCorner = true;
		const changed = JSON.stringify(next.region) !== regionBefore;
		if (next.phase === 'idle' && drawing && next.region && (sawSecondCorner || changed)) {
			drawing = false;
			void checkRegion();
		}
	}

	async function checkRegion() {
		if (!planet) return;
		const request = ++checkRequest;
		const forPlanet = planet;
		checking = true;
		try {
			const result = await scanMapCoordinates(forPlanet);
			if (request === checkRequest && open) check = { planet: forPlanet, result };
		} catch {
			if (request === checkRequest) failed = 'The coordinates could not be read.';
		} finally {
			if (request === checkRequest) checking = false;
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
						adopt(await getMapsCalibrationStatus());
					} catch {
						// The poll retries on its next tick.
					}
				},
				{ intervalMs: 600, immediate: false },
			);
		}
		status = null;
		check = null;
		drawing = false;
		sawSecondCorner = false;
		checkRequest++;
		void cancelMapsCalibration().catch(() => {});
		return undefined;
	});

	const phase = $derived(status?.phase ?? 'idle');
	const checkText = $derived.by(() => {
		if (!check) return null;
		const { result } = check;
		if (result.kind === 'refused') return coordRefusalMessage(result.refusal, check.planet);
		const point = formatGamePoint({ lon: result.lon, lat: result.lat });
		return result.source === 'region'
			? `Read from the region: ${point}.`
			: `The radar was found, so the region is not used: ${point}.`;
	});
</script>

<Modal bind:open title="Set the coordinate region by hand">
	<div class="space-y-4 text-sm text-text-secondary">
		{#if failed}
			<p>{failed}</p>
			<div class="flex justify-end">
				<Button variant="ghost" onclick={() => (open = false)}>Close</Button>
			</div>
		{:else if phase === 'awaitTopLeft'}
			<p class="text-text">
				Step 1 of 2: in the game, hover the top-left corner of <strong>LON</strong> under the
				radar, label included, and press
				<kbd class="rounded border border-border bg-surface px-1">Enter</kbd>.
			</p>
			<div class="flex justify-end gap-2">
				<CountdownCapture oncapture={confirmAtCursor} />
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			</div>
		{:else if phase === 'awaitBottomRight'}
			<p class="text-text">
				Step 2 of 2: hover just past the last digit of <strong>LAT</strong> and press
				<kbd class="rounded border border-border bg-surface px-1">Enter</kbd>.
			</p>
			<div class="flex justify-end gap-2">
				<CountdownCapture oncapture={confirmAtCursor} />
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			</div>
		{:else if status?.region}
			{#if checking}
				<p>Reading the coordinates…</p>
			{:else if checkText}
				<p class="text-text">{checkText}</p>
			{:else if !planet}
				<p>The region is set. Open a planet with coordinates to check what it reads.</p>
			{:else}
				<p>The region is set.</p>
			{/if}
			<div class="flex justify-end gap-2">
				<Button variant="secondary" onclick={begin}>Set again</Button>
				<Button onclick={() => (open = false)}>Done</Button>
			</div>
		{:else}
			<p>Preparing…</p>
		{/if}
	</div>
</Modal>
