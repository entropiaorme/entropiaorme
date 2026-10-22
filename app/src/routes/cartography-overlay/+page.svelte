<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { type PinConfig } from '$lib/api';
	import { describeError } from '$lib/view/errorState';
	import { createWindowSizeSync } from '$lib/windows/windowSize';
	import { pinGlyph } from '$lib/features/maps/pinIcons';
	import {
		cartographyOverlay,
		createConfirmedPin,
		scanAndDropPin,
		type PinDropOutcome,
	} from '$lib/features/maps/cartographyOverlay.svelte';
	import { createCartographyOverlayController } from '$lib/features/maps/cartographyOverlayController.svelte';

	let root: HTMLDivElement;
	const sizeSync = createWindowSizeSync(() => root);
	const overlay = createCartographyOverlayController();
	let busy = $state(false);
	type PendingDuplicate = Extract<PinDropOutcome, { kind: 'duplicate' }>;
	let pendingDuplicate = $state<PendingDuplicate | null>(null);
	let keepExistingButton = $state<HTMLButtonElement>();
	let feedback = $state<{ text: string; success: boolean } | null>(null);
	let feedbackTimer: ReturnType<typeof setTimeout> | null = null;

	function flash(text: string, success = false) {
		feedback = { text, success };
		if (feedbackTimer) clearTimeout(feedbackTimer);
		feedbackTimer = setTimeout(() => (feedback = null), 3500);
	}

	onMount(() => {
		const stopOverlay = overlay.start();
		const disconnectSizeSync = sizeSync.connect();
		return () => {
			stopOverlay();
			disconnectSizeSync();
			if (feedbackTimer) clearTimeout(feedbackTimer);
		};
	});

	function startDraggingFromSurface(event: PointerEvent) {
		if (event.button !== 0 || !(event.target instanceof Element)) return;
		if (event.target.closest('button, select, input, textarea, a, [role="button"]')) return;
		void getCurrentWindow().startDragging();
	}

	async function dropPin(config: PinConfig) {
		if (busy) return;
		busy = true;
		try {
			const outcome = await scanAndDropPin(
				config,
				cartographyOverlay.context,
				overlay.calibratedPlanets.map((candidate) => candidate.name),
			);
			if (outcome.kind === 'placed') flash(`${outcome.label} pinned.`, true);
			else if (outcome.kind === 'error') flash(outcome.message);
			else {
				pendingDuplicate = outcome;
				await tick();
				keepExistingButton?.focus();
			}
		} catch (error) {
			flash(describeError(error, 'The pin could not be saved'));
		} finally {
			busy = false;
		}
	}

	function keepExisting() {
		pendingDuplicate = null;
		flash('Existing pin kept.');
	}

	async function createDuplicate() {
		const pending = pendingDuplicate;
		if (!pending || busy) return;
		busy = true;
		try {
			await createConfirmedPin(pending.input);
			pendingDuplicate = null;
			flash(`${pending.label} pinned.`, true);
		} catch (error) {
			pendingDuplicate = null;
			flash(describeError(error, 'The pin could not be saved'));
		} finally {
			busy = false;
		}
	}

	function hideOverlay() {
		pendingDuplicate = null;
		void getCurrentWindow().hide();
	}
</script>

<!-- The window surface is a drag handle except where an interactive control owns the pointer. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->

<div bind:this={root} class="overlay-frame flex w-max items-start" onpointerdown={startDraggingFromSurface}>
	<div class="glass-panel overlay-strip flex w-max items-center gap-3 rounded-xl px-4 py-2" data-overlay-surface>
		<div
			class="flex min-w-0 max-w-48 shrink-0 flex-col justify-center border-r border-white/10 pr-3"
			title={`${cartographyOverlay.context.planet ?? 'No planet'} · ${overlay.selectedMapName}`}
		>
			<span class="text-[9px] font-bold uppercase leading-none tracking-wider text-white/35">
				Pinning to
			</span>
			<span class="mt-1 truncate text-[11px] font-medium leading-none text-white/70">
				{cartographyOverlay.context.planet ?? 'No planet'} · {overlay.selectedMapName}
			</span>
		</div>

		{#if pendingDuplicate}
			<div class="flex shrink-0 items-center gap-2" role="group" aria-label="Nearby pin confirmation">
				<p class="max-w-48 text-[10px] font-medium leading-snug text-orange-200/90">
					There is already a pin ({pendingDuplicate.existingName}) {pendingDuplicate.distance.toFixed(1)} m away.
				</p>
				<button bind:this={keepExistingButton} class="pin-button" disabled={busy} onclick={keepExisting}>
					Keep existing
				</button>
				<button class="pin-button confirm-button" disabled={busy} onclick={createDuplicate}>
					Create anyway
				</button>
			</div>
		{:else if cartographyOverlay.configs.length === 0}
			<p class="max-w-56 shrink-0 text-[10px] leading-snug text-white/50">
				No pins configured for this map. Add some in Maps → Configure pin overlay.
			</p>
		{:else}
			<div class="flex shrink-0 items-center gap-1.5">
				{#each cartographyOverlay.configs as config (config.id)}
					<button
						class="pin-button"
						disabled={busy || !cartographyOverlay.context.planet || overlay.planets.length === 0}
						onclick={() => dropPin(config)}
					>
						<span class="swatch" style="background:{config.colour}" aria-hidden="true"></span>
						<span aria-hidden="true">{pinGlyph(config.icon)}</span>
						<span>{config.label}</span>
					</button>
				{/each}
			</div>
		{/if}

		{#if feedback}
			<p
				role="status"
				class="max-w-48 shrink-0 border-l border-white/10 pl-3 text-[10px] font-medium leading-snug {feedback.success
					? 'text-emerald-300/90'
					: 'text-orange-300/90'}"
			>
				{feedback.text}
			</p>
		{/if}

		<button
			class="release-btn shrink-0"
			aria-label="Hide pin overlay"
			onclick={hideOverlay}
		>×</button>
	</div>
</div>

<style>
	.overlay-frame,
	.overlay-strip {
		overflow: visible;
	}

	.glass-panel {
		background: rgba(10, 14, 23, 0.85);
		backdrop-filter: blur(16px) saturate(150%);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.pin-button {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		min-width: 76px;
		padding: 4px 9px;
		border-radius: 5px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.7);
		font-size: 11px;
		font-weight: 500;
		line-height: 1.25;
		cursor: pointer;
		transition: background 150ms ease-out, border-color 150ms ease-out, color 150ms ease-out;
	}
	.pin-button:hover {
		background: rgba(56, 189, 248, 0.12);
		border-color: rgba(56, 189, 248, 0.35);
		color: rgba(125, 211, 252, 0.95);
	}
	.swatch {
		width: 8px;
		height: 8px;
		border-radius: 9999px;
		box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.25);
	}
	.confirm-button {
		border-color: rgba(56, 189, 248, 0.35);
		background: rgba(56, 189, 248, 0.12);
		color: rgba(125, 211, 252, 0.95);
	}
	.pin-button:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.release-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.4);
		font-size: 12px;
		line-height: 1;
		cursor: pointer;
		transition: all 150ms ease-out;
	}
	.release-btn:hover {
		background: rgba(255, 255, 255, 0.1);
		border-color: rgba(255, 255, 255, 0.25);
		color: rgba(255, 255, 255, 0.7);
	}
</style>
