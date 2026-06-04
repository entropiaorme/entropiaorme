<script lang="ts">
	import { onMount } from 'svelte';
	import { invoke } from '@tauri-apps/api/core';
	import { getCurrentWindow } from '@tauri-apps/api/window';
	import { listen, type UnlistenFn } from '@tauri-apps/api/event';
	import { LogicalSize } from '@tauri-apps/api/dpi';
	import Button from '$lib/components/Button.svelte';
	import {
		startManualSkillScan,
		captureManualSkillPage,
		cancelManualSkillScan,
		undoManualSkillCapture,
		getManualSkillScanStatus,
		setSpacebarCapture,
		type ScanManualStatus
	} from '$lib/api';
	import { SCAN_TOPIC } from '$lib/stores/scanStore';

	const OVERLAY_SIZE_SLACK = 36;

	let overlayRoot: HTMLDivElement | null = $state(null);
	let resizeFrame: number | null = null;
	let lastWindowWidth: number | null = null;
	let lastWindowHeight: number | null = null;

	let status = $state<ScanManualStatus | null>(null);
	let busy = $state(false);
	let message = $state<string>('');

	const PAGE_COUNT_MIN = 1;
	const PAGE_COUNT_MAX = 30;
	const SKILL_PAGE_COUNT_KEY = 'scan-overlay:skill-page-count';
	const SKILL_PAGE_COUNT_DEFAULT = 12;

	function clampPageCount(value: number, fallback: number): number {
		if (!Number.isFinite(value)) return fallback;
		return Math.min(PAGE_COUNT_MAX, Math.max(PAGE_COUNT_MIN, Math.round(value)));
	}

	function readStoredPageCount(key: string, fallback: number): number {
		if (typeof localStorage === 'undefined') return fallback;
		const raw = localStorage.getItem(key);
		const parsed = raw === null ? NaN : parseInt(raw, 10);
		return clampPageCount(parsed, fallback);
	}

	let skillPageCount = $state<number>(
		readStoredPageCount(SKILL_PAGE_COUNT_KEY, SKILL_PAGE_COUNT_DEFAULT),
	);

	$effect(() => {
		if (typeof localStorage === 'undefined') return;
		localStorage.setItem(SKILL_PAGE_COUNT_KEY, String(skillPageCount));
	});

	const SPACEBAR_CAPTURE_KEY = 'scan-overlay:spacebar-capture';

	function readStoredSpacebarCapture(): boolean {
		if (typeof localStorage === 'undefined') return false;
		return localStorage.getItem(SPACEBAR_CAPTURE_KEY) === 'true';
	}

	let spacebarCapture = $state<boolean>(readStoredSpacebarCapture());

	$effect(() => {
		if (typeof localStorage === 'undefined') return;
		localStorage.setItem(SPACEBAR_CAPTURE_KEY, String(spacebarCapture));
	});

	async function syncSpacebarCapture(enabled: boolean) {
		try {
			await setSpacebarCapture(enabled);
		} catch (err) {
			message = `spacebar toggle failed: ${err}`;
		}
	}

	async function toggleSpacebarCapture() {
		spacebarCapture = !spacebarCapture;
		await syncSpacebarCapture(spacebarCapture);
	}

	let phase = $derived(status?.phase ?? 'idle');
	let allCaptured = $derived(
		status !== null && status.captured_pages >= status.expected_pages,
	);
	let canUndo = $derived(status !== null && status.captured_pages > 0);

	onMount(async () => {
		// The initial hydrate lives in the scan-frame subscription effect below,
		// after the listener attaches, so a status change between the first read
		// and the listener attaching is not lost. Here we only replay the stored
		// spacebar-capture toggle so the backend listener state matches the UI on
		// every overlay (re)mount.
		await syncSpacebarCapture(spacebarCapture);
	});

	// Event-driven status, replacing the retired 500ms poll: re-read on each
	// backend scan frame the relay re-emits (a change driven by this overlay's
	// own actions, a spacebar capture, or background OCR progress). The producer
	// coalesces, so this fires once per discrete change rather than on a timer.
	// The webview keeps listening while the window is hidden, so the status stays
	// current without a poll; the relay's reconnect nudge re-reads after a stream
	// drop.
	$effect(() => {
		let unlisten: UnlistenFn | undefined;
		let disposed = false;
		void listen(SCAN_TOPIC, () => {
			void refreshStatus();
		}).then((fn) => {
			if (disposed) {
				fn();
				return;
			}
			unlisten = fn;
			// Hydrate after the listener attaches (not in onMount), so a frame
			// between the first read and the listener attaching is not lost.
			void refreshStatus();
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	// The retired 500ms poll also refreshed the environmental status fields
	// (game-window presence, OCR-engine availability) that no scan frame
	// announces, since no verb mutates them. Re-read on focus gain so launching
	// EU (or the OCR engine appearing) after the overlay opened un-wedges the
	// Start button. show_scan_overlay focuses the window, so this fires on show.
	$effect(() => {
		let unlisten: UnlistenFn | undefined;
		let disposed = false;
		void getCurrentWindow()
			.onFocusChanged(({ payload: focused }) => {
				if (!disposed && focused) void refreshStatus();
			})
			.then((fn) => {
				if (disposed) fn();
				else unlisten = fn;
			});
		return () => {
			disposed = true;
			unlisten?.();
		};
	});

	async function handleDrag(e: MouseEvent) {
		const target = e.target as HTMLElement;
		if (target.closest('button, [role="button"], input, select, textarea')) return;
		await getCurrentWindow().startDragging();
	}

	function measureOverlaySize(root: HTMLDivElement) {
		const rect = root.getBoundingClientRect();
		return {
			width: Math.max(1, Math.ceil(rect.width + OVERLAY_SIZE_SLACK)),
			height: Math.max(1, Math.ceil(rect.height + OVERLAY_SIZE_SLACK))
		};
	}

	async function syncOverlayWindowSize() {
		if (!overlayRoot) return;
		const { width, height } = measureOverlaySize(overlayRoot);
		if (width === lastWindowWidth && height === lastWindowHeight) return;
		lastWindowWidth = width;
		lastWindowHeight = height;
		try {
			await getCurrentWindow().setSize(new LogicalSize(width, height));
		} catch {
			lastWindowWidth = null;
			lastWindowHeight = null;
		}
	}

	function scheduleOverlayWindowSizeSync() {
		if (!overlayRoot || resizeFrame != null) return;
		resizeFrame = window.requestAnimationFrame(() => {
			resizeFrame = null;
			void syncOverlayWindowSize();
		});
	}

	$effect(() => {
		if (!overlayRoot) return;
		scheduleOverlayWindowSizeSync();
		const observer = new ResizeObserver(() => scheduleOverlayWindowSizeSync());
		observer.observe(overlayRoot);
		return () => {
			if (resizeFrame != null) {
				window.cancelAnimationFrame(resizeFrame);
				resizeFrame = null;
			}
			observer.disconnect();
		};
	});

	let statusInFlight = false;
	let statusRefetchQueued = false;

	// Coalesce overlapping reads (the same guard the shared scan store uses): a
	// frame arriving mid-read queues exactly one follow-up, so two reads can never
	// resolve out of order and wedge the overlay on a stale phase. The producer
	// emits the final processing frame and the awaiting_review frame microseconds
	// apart, so an unguarded read could otherwise settle on the earlier one.
	async function refreshStatus() {
		if (statusInFlight) {
			statusRefetchQueued = true;
			return;
		}
		statusInFlight = true;
		try {
			do {
				statusRefetchQueued = false;
				try {
					status = await getManualSkillScanStatus();
					// A good read clears a prior transient read-failure notice; an
					// action message (start/capture/undo result) is left intact.
					if (message.startsWith('status error:')) message = '';
				} catch (err) {
					// Transient read failure: keep the last good status and surface
					// the error. The catch is INSIDE the loop so a re-read a frame
					// queued during this attempt is not abandoned (it may be the
					// last transition, with no later frame to re-trigger it).
					message = `status error: ${err}`;
				}
			} while (statusRefetchQueued);
		} finally {
			statusInFlight = false;
		}
	}

	async function handleStart() {
		busy = true;
		message = '';
		try {
			const r = await startManualSkillScan(clampPageCount(skillPageCount, SKILL_PAGE_COUNT_DEFAULT));
			if ('error' in r && r.error) message = r.error;
			status = r;
		} catch (err) {
			message = `start failed: ${err}`;
		} finally {
			busy = false;
		}
	}

	async function handleCapture() {
		busy = true;
		message = '';
		try {
			const r = await captureManualSkillPage();
			if ('error' in r && r.error) {
				message = r.error;
			} else if (r.captured === false) {
				message = `page ${r.page}: capture failed`;
			}
			status = r;
		} catch (err) {
			message = `capture failed: ${err}`;
		} finally {
			busy = false;
		}
	}

	async function handleUndo() {
		busy = true;
		message = '';
		try {
			const r = await undoManualSkillCapture();
			if ('error' in r && r.error) {
				message = r.error;
			} else if (r.undone_page !== undefined) {
				message = `undid page ${r.undone_page}`;
			}
			status = r;
		} catch (err) {
			message = `undo failed: ${err}`;
		} finally {
			busy = false;
		}
	}

	async function handleCancel() {
		busy = true;
		try {
			const r = await cancelManualSkillScan();
			if ('error' in r && r.error) {
				message = r.error;
			} else {
				message = 'cancelled';
			}
			status = r;
		} finally {
			busy = false;
		}
	}

	function handleClose() {
		invoke('hide_scan_overlay').catch(() => {});
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="p-2 flex flex-col items-start overlay-frame w-max" bind:this={overlayRoot} onmousedown={handleDrag}>
	<!-- Glass panel -->
	<div class="overlay-strip glass-panel flex flex-col gap-3 rounded-xl px-4 py-3 w-72">
			<!-- Header: title + close -->
			<div class="flex items-center gap-3">
				<span class="text-[10px] font-bold tracking-wider uppercase text-white/40 shrink-0">Skill scan</span>
				<button
					type="button"
					class="ml-auto release-btn"
					aria-label="Close"
					onclick={handleClose}
					title="Close"
				>×</button>
			</div>

			{#if phase === 'idle'}
				<p class="text-[11px] leading-snug text-white/50 m-0">
					Dock the Skills panel to the <span class="text-white/80">bottom-right</span> of the EU window, then Start.
				</p>

				<div class="flex items-center gap-2">
					<span class="text-[10px] uppercase tracking-wider text-white/40">Pages</span>
					<button
						type="button"
						class="step-btn"
						onclick={() => (skillPageCount = clampPageCount(skillPageCount - 1, SKILL_PAGE_COUNT_DEFAULT))}
						disabled={busy || skillPageCount <= PAGE_COUNT_MIN}
						aria-label="Decrease page count"
					>−</button>
					<input
						type="number"
						class="page-count-input tabular-nums"
						min={PAGE_COUNT_MIN}
						max={PAGE_COUNT_MAX}
						step="1"
						bind:value={skillPageCount}
						onblur={() => (skillPageCount = clampPageCount(skillPageCount, SKILL_PAGE_COUNT_DEFAULT))}
					/>
					<button
						type="button"
						class="step-btn"
						onclick={() => (skillPageCount = clampPageCount(skillPageCount + 1, SKILL_PAGE_COUNT_DEFAULT))}
						disabled={busy || skillPageCount >= PAGE_COUNT_MAX}
						aria-label="Increase page count"
					>+</button>
					<span class="text-[10px] text-white/40">in your skills panel</span>
				</div>

				<label class="spacebar-toggle">
					<input
						type="checkbox"
						checked={spacebarCapture}
						onchange={toggleSpacebarCapture}
					/>
					<span class="spacebar-toggle-label">Capture with <kbd class="kbd">Spacebar</kbd></span>
				</label>

				<Button
					variant="primary"
					size="sm"
					onclick={handleStart}
					disabled={busy || !status?.configured || !status?.game_window_present}
				>Start</Button>

				{#if status && !status.configured}
					<p class="warn-line">Local OCR engine unavailable: check the backend log.</p>
				{:else if status && !status.game_window_present}
					<p class="warn-line">Entropia Universe window not found: start the game first.</p>
				{/if}
			{:else if phase === 'capturing'}
				<!-- Capture progress -->
				<div class="flex items-center gap-2">
					<span class="relative flex h-2 w-2 shrink-0">
						<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-accent opacity-75"></span>
						<span class="relative inline-flex rounded-full h-2 w-2 bg-accent"></span>
					</span>
					<span class="text-sm font-semibold text-white/90 tabular-nums">
						{status?.captured_pages ?? 0} / {status?.expected_pages ?? 0}
					</span>
					<span class="text-[10px] uppercase tracking-wider text-white/40">captured</span>
				</div>

				{#if allCaptured}
					<p class="text-[11px] leading-snug text-white/70 m-0">
						All pages captured. Open the main window and click <span class="text-white">Start Processing</span>.
					</p>
					<div class="flex items-center gap-1.5">
						<Button variant="secondary" size="sm" onclick={handleUndo} disabled={busy || !canUndo}>Undo</Button>
						<Button variant="secondary" size="sm" class="flex-1" onclick={handleCancel} disabled={busy}>Cancel</Button>
					</div>
				{:else}
					<p class="text-[11px] leading-snug text-white/50 m-0">
						Show a page in-game, click <span class="text-white/80">Capture</span>, flip to the next, repeat.
					</p>
					<div class="flex items-center gap-1.5">
						<Button variant="primary" size="sm" class="flex-1" onclick={handleCapture} disabled={busy}>Capture</Button>
						<Button variant="secondary" size="sm" onclick={handleUndo} disabled={busy || !canUndo}>Undo</Button>
						<Button variant="secondary" size="sm" onclick={handleCancel} disabled={busy}>Cancel</Button>
					</div>
					<label class="spacebar-toggle">
						<input
							type="checkbox"
							checked={spacebarCapture}
							onchange={toggleSpacebarCapture}
						/>
						<span class="spacebar-toggle-label">Capture with <kbd class="kbd">Spacebar</kbd></span>
					</label>
				{/if}
			{:else if phase === 'processing'}
				<div class="flex items-center gap-2">
					<span class="relative flex h-2 w-2 shrink-0">
						<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-accent opacity-75"></span>
						<span class="relative inline-flex rounded-full h-2 w-2 bg-accent"></span>
					</span>
					<span class="text-sm font-semibold text-white/90 tabular-nums">
						{status?.processing_progress.done ?? 0} / {status?.processing_progress.total ?? 0}
					</span>
					<span class="text-[10px] uppercase tracking-wider text-white/40">processed</span>
				</div>
				<p class="text-[11px] leading-snug text-white/50 m-0">
					Reading skill levels. Watch progress in the main window.
				</p>
			{:else if phase === 'awaiting_review'}
				<p class="text-[11px] leading-snug text-white/70 m-0">
					Results ready: review them in the main window and accept or reject.
				</p>
			{/if}

			{#if message}
				<p class="text-[11px] text-white/50 m-0 break-words">{message}</p>
			{/if}
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

	.page-count-input {
		width: 38px;
		padding: 2px 4px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.9);
		font-size: 12px;
		font-weight: 600;
		text-align: center;
	}
	.page-count-input:focus {
		outline: none;
		border-color: rgba(52, 211, 153, 0.5);
	}
	.page-count-input::-webkit-outer-spin-button,
	.page-count-input::-webkit-inner-spin-button {
		-webkit-appearance: none;
		margin: 0;
	}
	.page-count-input[type='number'] {
		appearance: textfield;
		-moz-appearance: textfield;
	}

	.step-btn {
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.6);
		font-size: 12px;
		line-height: 1;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 150ms ease-out;
	}
	.step-btn:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.9);
	}
	.step-btn:disabled {
		opacity: 0.3;
		cursor: not-allowed;
	}

	.warn-line {
		font-size: 11px;
		color: rgba(251, 146, 60, 0.85);
		margin: 0;
	}

	.spacebar-toggle {
		display: flex;
		align-items: center;
		gap: 6px;
		cursor: pointer;
		user-select: none;
	}
	.spacebar-toggle input[type='checkbox'] {
		-webkit-appearance: none;
		appearance: none;
		width: 12px;
		height: 12px;
		border-radius: 3px;
		border: 1px solid rgba(255, 255, 255, 0.2);
		background: rgba(255, 255, 255, 0.05);
		display: inline-flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		position: relative;
		transition: all 150ms ease-out;
	}
	.spacebar-toggle input[type='checkbox']:hover {
		border-color: rgba(255, 255, 255, 0.35);
	}
	.spacebar-toggle input[type='checkbox']:checked {
		background: rgba(52, 211, 153, 0.25);
		border-color: rgba(52, 211, 153, 0.55);
	}
	.spacebar-toggle input[type='checkbox']:checked::after {
		content: '';
		position: absolute;
		width: 4px;
		height: 7px;
		border: solid rgba(52, 211, 153, 0.95);
		border-width: 0 1.5px 1.5px 0;
		transform: rotate(45deg) translate(-1px, -1px);
	}
	.spacebar-toggle-label {
		font-size: 10px;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: rgba(255, 255, 255, 0.5);
	}
	.kbd {
		display: inline-block;
		padding: 1px 5px;
		border-radius: 3px;
		border: 1px solid rgba(255, 255, 255, 0.18);
		background: rgba(255, 255, 255, 0.06);
		color: rgba(255, 255, 255, 0.85);
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
		font-size: 9px;
		line-height: 1;
		text-transform: none;
		letter-spacing: 0;
		vertical-align: 1px;
	}

	.release-btn {
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.4);
		font-size: 12px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: all 150ms ease-out;
	}
	.release-btn:hover {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.7);
		border-color: rgba(255, 255, 255, 0.25);
	}
</style>
