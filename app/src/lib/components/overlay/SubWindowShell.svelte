<script lang="ts" generics="S">
	// The sub-window's half of the overlay's sub-windows (the rules are in
	// `lib/windows/subWindow.ts`): every sub-window route renders what it
	// holds inside this shell, and the shell does everything they share.
	//
	// It takes what the host hands it to show, measures what that draws,
	// sizes its window to it and places it against its control (below, or
	// above when there is no room, and always on the screen), and only then
	// reveals it, so it never lands in one place and jumps to another. It
	// cuts the window's input to what is drawn. And it reports what happens
	// in it: the pointer coming and going, the player typing, and its own
	// closing, on Escape or on losing the keyboard to a click elsewhere.
	import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
	import type { UnlistenFn } from '@tauri-apps/api/event';
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import type { Snippet } from 'svelte';
	import {
		placeSubWindow,
		type SubWindowChannel,
		type SubWindowCloseReason,
		type SubWindowEnvelope,
		type SubWindowReport,
	} from '$lib/windows/subWindow';
	import { createWindowSizeSync } from '$lib/windows/windowSize';

	/** What a sub-window's content is given. */
	interface Held {
		id: string;
		state: S;
		/** Close the sub-window (an action that is done with it). */
		close: (reason?: SubWindowCloseReason) => Promise<void>;
	}

	let {
		channel,
		host = 'overlay',
		children,
	}: {
		channel: SubWindowChannel;
		/** The label of the window that opens this one. */
		host?: string;
		children: Snippet<[Held]>;
	} = $props();

	/** The size the window takes to lay its content out, where a hidden
	 * window will not lay out at all. */
	const MEASURING_WIDTH = 320;
	const MEASURING_HEIGHT = 64;
	const MIN_MEASURED = 2;
	/** Losing the keyboard this soon after being shown is the show itself
	 * settling, not the player looking away. */
	const SETTLE_MS = 200;

	const currentWindow = getCurrentWebviewWindow();

	let envelope = $state<SubWindowEnvelope<S> | null>(null);
	let shellEl: HTMLDivElement | null = $state(null);
	let visible = $state(false);
	let maxHeight = $state<number | null>(null);
	let shownAt = 0;
	let revealed = false;
	let measuring = false;
	let lastSize: string | null = null;
	let frame: number | null = null;
	let revealFrame: number | null = null;

	const regionSync = createWindowSizeSync(() => shellEl, { resize: false });

	function report(message: SubWindowReport) {
		void currentWindow.emitTo(host, channel.report, message).catch(() => {});
	}

	function cancelFrames() {
		if (frame != null) window.cancelAnimationFrame(frame);
		if (revealFrame != null) window.cancelAnimationFrame(revealFrame);
		frame = null;
		revealFrame = null;
	}

	function reset() {
		cancelFrames();
		visible = false;
		maxHeight = null;
		revealed = false;
		measuring = false;
		lastSize = null;
	}

	async function close(reason: SubWindowCloseReason = 'done') {
		const shown = envelope;
		if (!shown) return;
		envelope = null;
		reset();
		await currentWindow.hide().catch(() => {});
		report({ type: 'closed', id: shown.id, reason });
	}

	function schedulePlace() {
		// A window that is not on screen is given no animation frames, so
		// the placement that puts it there cannot wait for one: until the
		// window has been shown (to be measured, or revealed) it is placed
		// at once.
		if (!revealed && !measuring) {
			void place();
			return;
		}
		if (frame != null) return;
		frame = window.requestAnimationFrame(() => {
			frame = null;
			void place();
		});
	}

	/** Size the window to what is drawn and put it against its control;
	 * the first time through, reveal it. */
	async function place() {
		const shown = envelope;
		if (!shellEl || !shown) return;
		const rect = shellEl.getBoundingClientRect();
		if (rect.width < MIN_MEASURED || rect.height < MIN_MEASURED) {
			// Nothing laid out yet: a hidden window may not lay out at all,
			// so it is shown, its content still unseen, to be measured.
			if (!measuring) {
				measuring = true;
				const at = placeSubWindow(
					shown.anchor,
					{ width: MEASURING_WIDTH, height: MEASURING_HEIGHT },
					shown.bounds,
					shown.gap,
					shown.align,
				);
				await currentWindow
					.setSize(new LogicalSize(MEASURING_WIDTH, MEASURING_HEIGHT))
					.catch(() => {});
				await currentWindow.setPosition(new LogicalPosition(at.x, at.y)).catch(() => {});
				await currentWindow.show().catch(() => {});
			}
			schedulePlace();
			return;
		}

		const width = Math.max(1, Math.ceil(rect.width));
		const natural = Math.max(1, Math.ceil(shellEl.scrollHeight));
		const at = placeSubWindow(
			shown.anchor,
			{ width, height: natural },
			shown.bounds,
			shown.gap,
			shown.align,
		);
		const height = at.maxHeight === null ? natural : Math.max(1, Math.min(natural, at.maxHeight));
		// Taller than the room there is, it scrolls inside that room.
		maxHeight = height < natural ? height : null;
		const size = `${width}x${height}`;
		if (size !== lastSize) {
			lastSize = size;
			try {
				await currentWindow.setSize(new LogicalSize(width, height));
			} catch {
				lastSize = null;
				return;
			}
		}
		if (envelope !== shown) return;
		await currentWindow.setPosition(new LogicalPosition(at.x, at.y)).catch(() => {});
		if (revealed) return;
		revealed = true;
		// Settling is counted from the reveal: measuring a window made for
		// the first time can take longer than the settle itself.
		shownAt = Date.now();
		await currentWindow.show().catch(() => {});
		if (shown.focus) await currentWindow.setFocus().catch(() => {});
		revealFrame = window.requestAnimationFrame(() => {
			revealFrame = null;
			visible = true;
		});
	}

	// Follow what is drawn: its size moves the window, and its shape the
	// input region.
	$effect(() => {
		if (!shellEl || !envelope) return;
		const observer = new ResizeObserver(() => schedulePlace());
		observer.observe(shellEl);
		schedulePlace();
		const disconnectRegion = regionSync.connect();
		return () => {
			observer.disconnect();
			disconnectRegion();
		};
	});

	$effect(() => {
		let disposed = false;
		const stops: UnlistenFn[] = [];
		const keep = (stop: UnlistenFn) => {
			if (disposed) stop();
			else stops.push(stop);
		};

		const handleEscape = (event: KeyboardEvent) => {
			if (event.key !== 'Escape' || !envelope) return;
			event.preventDefault();
			void close('escape');
		};

		void (async () => {
			keep(
				await currentWindow.listen<SubWindowEnvelope<S>>(channel.show, (event) => {
					if (disposed) return;
					reset();
					shownAt = Date.now();
					envelope = event.payload;
				}),
			);
			keep(
				await currentWindow.listen<SubWindowEnvelope<S>>(channel.update, (event) => {
					if (disposed || !envelope || envelope.id !== event.payload.id) return;
					envelope = event.payload;
					schedulePlace();
				}),
			);
			keep(
				await currentWindow.listen(channel.hide, async () => {
					if (disposed) return;
					envelope = null;
					reset();
					await currentWindow.hide().catch(() => {});
				}),
			);
			keep(
				await currentWindow.onFocusChanged(({ payload: focused }) => {
					if (disposed || focused || !envelope) return;
					// One that never took the keyboard has none to lose, and a
					// task the player left for the game stays.
					if (!envelope.focus || envelope.stays) return;
					if (Date.now() - shownAt < SETTLE_MS) return;
					void close('blur');
				}),
			);
			await currentWindow
				.emitTo(host, channel.ready, { label: currentWindow.label })
				.catch(() => {});
		})();

		window.addEventListener('keydown', handleEscape);
		return () => {
			disposed = true;
			for (const stop of stops) stop();
			window.removeEventListener('keydown', handleEscape);
			cancelFrames();
		};
	});

	/** Whether `target` is somewhere the player types. */
	function typedIn(target: EventTarget | null): boolean {
		return target instanceof HTMLElement && target.matches('input, textarea, [contenteditable]');
	}

	function pointer(inside: boolean) {
		if (envelope) report({ type: 'pointer', id: envelope.id, inside });
	}

	function held(event: FocusEvent, isHeld: boolean) {
		if (envelope && typedIn(event.target)) {
			report({ type: 'held', id: envelope.id, held: isHeld });
		}
	}

	function interact() {
		if (envelope) report({ type: 'interact', id: envelope.id });
	}
</script>

{#if envelope}
	<!-- The pointer and focus handlers only watch: every control inside is
		 its own element with its own role. -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="sub-window"
		class:sub-window-visible={visible}
		bind:this={shellEl}
		data-overlay-surface
		data-testid="sub-window"
		data-sub-window={envelope.id}
		style:max-height={maxHeight === null ? undefined : `${maxHeight}px`}
		onpointerenter={() => pointer(true)}
		onpointerleave={() => pointer(false)}
		onpointerdown={interact}
		onwheel={interact}
		onfocusin={(event) => held(event, true)}
		onfocusout={(event) => held(event, false)}
	>
		{@render children({ id: envelope.id, state: envelope.state, close })}
	</div>
{/if}

<style>
	.sub-window {
		display: flex;
		width: max-content;
		height: max-content;
		border-radius: 8px;
		overflow: hidden auto;
		opacity: 0;
		transition: opacity 120ms ease;
	}

	.sub-window-visible {
		opacity: 1;
	}

	@media (prefers-reduced-motion: reduce) {
		.sub-window {
			transition: none;
		}
	}
</style>
