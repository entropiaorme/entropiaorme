<script lang="ts">
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
	import type { UnlistenFn } from '@tauri-apps/api/event';
	import { tick } from 'svelte';
	import ProtectionCostPanel from '../overlay/ProtectionCostPanel.svelte';
	import {
		OVERLAY_ARMOUR_COST_CLOSED_EVENT,
		OVERLAY_ARMOUR_COST_HIDE_EVENT,
		OVERLAY_ARMOUR_COST_READY_EVENT,
		OVERLAY_ARMOUR_COST_SHOW_EVENT,
		OVERLAY_ARMOUR_COST_UPDATE_EVENT,
		type OverlayArmourCostState
	} from '$lib/windows/overlayArmourCost';

	const SIZE_SLACK = 4;
	const FALLBACK_WIDTH = 320;
	const FALLBACK_HEIGHT = 64;
	const MIN_MEASURED_SIZE = 2;

	const currentWindow = getCurrentWebviewWindow();

	let panelState = $state<OverlayArmourCostState | null>(null);
	let shellEl: HTMLDivElement | null = $state(null);
	let lastWidth: number | null = null;
	let lastHeight: number | null = null;
	let firstShowDone = false;
	let panelVisible = $state(false);
	let measurementWindowShown = false;
	let syncFrame: number | null = null;
	let revealFrame: number | null = null;

	function cancelScheduledFrames() {
		if (syncFrame != null) {
			window.cancelAnimationFrame(syncFrame);
			syncFrame = null;
		}
		if (revealFrame != null) {
			window.cancelAnimationFrame(revealFrame);
			revealFrame = null;
		}
	}

	function resetPresentationState() {
		panelVisible = false;
		measurementWindowShown = false;
		lastWidth = null;
		lastHeight = null;
		firstShowDone = false;
		cancelScheduledFrames();
	}

	function scheduleSyncSizeAndPosition() {
		if (syncFrame != null) return;
		syncFrame = window.requestAnimationFrame(() => {
			syncFrame = null;
			void syncSizeAndPosition();
		});
	}

	async function syncSizeAndPosition() {
		if (!shellEl || !panelState) return;
		const rect = shellEl.getBoundingClientRect();
		if (rect.width < MIN_MEASURED_SIZE || rect.height < MIN_MEASURED_SIZE) {
			if (!measurementWindowShown) {
				measurementWindowShown = true;
				const { centerX, top } = panelState.anchor;
				await currentWindow.setSize(new LogicalSize(FALLBACK_WIDTH, FALLBACK_HEIGHT)).catch(() => {});
				await currentWindow
					.setPosition(new LogicalPosition(centerX - FALLBACK_WIDTH / 2, top))
					.catch(() => {});
				await currentWindow.show().catch(() => {});
			}
			scheduleSyncSizeAndPosition();
			return;
		}

		const width = Math.max(1, Math.ceil(rect.width + SIZE_SLACK));
		const height = Math.max(1, Math.ceil(rect.height + SIZE_SLACK));
		const sizeChanged = width !== lastWidth || height !== lastHeight;
		if (sizeChanged) {
			lastWidth = width;
			lastHeight = height;
			try {
				await currentWindow.setSize(new LogicalSize(width, height));
			} catch {
				lastWidth = null;
				lastHeight = null;
				return;
			}
		}
		try {
			const { centerX, top } = panelState.anchor;
			await currentWindow.setPosition(new LogicalPosition(centerX - width / 2, top));
		} catch { /* ignore */ }
		// First reveal happens once we have real measurements — popup is sized and
		// centred under the anchor before becoming visible, so the user never sees
		// it land at a wrong spot and snap.
		if (!firstShowDone && width > 1 && height > 1) {
			firstShowDone = true;
			await currentWindow.show().catch(() => {});
			await tick();
			revealFrame = window.requestAnimationFrame(() => {
				revealFrame = null;
				panelVisible = true;
			});
			await currentWindow.setFocus().catch(() => {});
		}
	}

	async function requestClose() {
		if (!panelState) return;
		panelState = null;
		resetPresentationState();
		await currentWindow.hide().catch(() => {});
		await currentWindow.emitTo('overlay', OVERLAY_ARMOUR_COST_CLOSED_EVENT).catch(() => {});
	}

	$effect(() => {
		if (!shellEl || !panelState) return;
		const observer = new ResizeObserver(() => {
			void syncSizeAndPosition();
		});
		observer.observe(shellEl);
		void syncSizeAndPosition();
		return () => observer.disconnect();
	});

	$effect(() => {
		let disposed = false;
		let unlistenShow: UnlistenFn | undefined;
		let unlistenUpdate: UnlistenFn | undefined;
		let unlistenHide: UnlistenFn | undefined;

		const handleEscape = (event: KeyboardEvent) => {
			if (event.key !== 'Escape') return;
			event.preventDefault();
			void requestClose();
		};

		void (async () => {
			unlistenShow = await currentWindow.listen<OverlayArmourCostState>(
				OVERLAY_ARMOUR_COST_SHOW_EVENT,
				async (event) => {
					if (disposed) return;
					resetPresentationState();
					panelState = event.payload;
				}
			);

			unlistenUpdate = await currentWindow.listen<OverlayArmourCostState>(
				OVERLAY_ARMOUR_COST_UPDATE_EVENT,
				async (event) => {
					if (disposed || !panelState) return;
					panelState = event.payload;
					void syncSizeAndPosition();
				}
			);

			unlistenHide = await currentWindow.listen(OVERLAY_ARMOUR_COST_HIDE_EVENT, async () => {
				if (disposed) return;
				panelState = null;
				resetPresentationState();
				await currentWindow.hide().catch(() => {});
			});

			await currentWindow
				.emitTo('overlay', OVERLAY_ARMOUR_COST_READY_EVENT, { label: currentWindow.label })
				.catch(() => {});
		})();

		window.addEventListener('keydown', handleEscape);

		return () => {
			disposed = true;
			unlistenShow?.();
			unlistenUpdate?.();
			unlistenHide?.();
			window.removeEventListener('keydown', handleEscape);
			cancelScheduledFrames();
		};
	});
</script>

{#if panelState}
	<!-- This workflow stays open while the player returns to the game to exchange armour and plates between steps. -->
	<div class="armour-cost-shell" bind:this={shellEl}>
		<div
			class="armour-cost-panel"
			class:armour-cost-panel-visible={panelVisible}
		>
			<ProtectionCostPanel
				sessionId={panelState.sessionId}
				repairOcrEnabled={panelState.repairOcrEnabled}
				steps={panelState.steps}
				protection={panelState.protection}
				requiresLoadoutSelection={panelState.requiresLoadoutSelection}
				onClose={() => void requestClose()}
			/>
		</div>
	</div>
{/if}

<style>
	.armour-cost-shell {
		display: inline-flex;
	}

	.armour-cost-panel {
		display: inline-flex;
		align-items: center;
		padding: 8px 10px;
		border-radius: 8px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(11, 15, 25, 0.96);
		backdrop-filter: blur(16px) saturate(150%);
		box-shadow:
			0 14px 30px rgba(0, 0, 0, 0.48),
			0 0 0 1px rgba(255, 255, 255, 0.03);
		opacity: 0;
		pointer-events: none;
		transform: translateY(-6px);
		transition:
			opacity 160ms ease,
			transform 160ms cubic-bezier(0.22, 1, 0.36, 1);
	}

	.armour-cost-panel-visible {
		opacity: 1;
		pointer-events: auto;
		transform: translateY(0);
	}

	@media (prefers-reduced-motion: reduce) {
		.armour-cost-panel {
			transition: none;
		}
	}
</style>
