<script lang="ts">
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import type { UnlistenFn } from '@tauri-apps/api/event';
	import OverlayMenuPanel from '$lib/components/overlay/OverlayMenuPanel.svelte';
	import QuestHandInPanel from '$lib/components/overlay/QuestHandInPanel.svelte';
	import {
		OVERLAY_MENU_CLOSED_EVENT,
		OVERLAY_MENU_HIDE_EVENT,
		OVERLAY_MENU_INTERACT_EVENT,
		OVERLAY_MENU_READY_EVENT,
		OVERLAY_MENU_SELECT_EVENT,
		OVERLAY_MENU_SHOW_EVENT,
		computeMenuHeight,
		menuRowCount,
		type OverlayMenuSelection,
		type OverlayMenuState
	} from '$lib/windows/overlayMenu';
	import { createWindowSizeSync } from '$lib/windows/windowSize';

	const currentWindow = getCurrentWebviewWindow();

	let menuState = $state<OverlayMenuState | null>(null);
	let menuShell: HTMLDivElement | null = $state(null);
	let suppressBlurCloseUntil = 0;

	// The host sizes and places this window; the window itself cuts its
	// input to the panel, so the rounded corners pass clicks through.
	const regionSync = createWindowSizeSync(() => menuShell, { resize: false });
	$effect(() => {
		if (!menuShell) return;
		return regionSync.connect();
	});

	const popupHeight = $derived(menuState ? computeMenuHeight(menuRowCount(menuState)) : 1);
	const popupWidth = $derived(menuState?.width ?? 1);

	async function requestClose() {
		if (!menuState) return;
		menuState = null;
		await currentWindow.hide().catch(() => {});
		await currentWindow.emitTo('overlay', OVERLAY_MENU_CLOSED_EVENT).catch(() => {});
	}

	async function handleSelection(selection: OverlayMenuSelection) {
		await currentWindow.emitTo('overlay', OVERLAY_MENU_SELECT_EVENT, selection).catch(() => {});
		await requestClose();
	}

	/** An Activities action keeps the control open (the overlay re-shows
	 * it with the refreshed rows), so declaring one thing after another
	 * is not a close-and-reopen, and the switch's effect is visible where
	 * it happened. */
	async function handleActivitySelection(selection: OverlayMenuSelection) {
		suppressBlurCloseUntil = Date.now() + 400;
		await currentWindow.emitTo('overlay', OVERLAY_MENU_SELECT_EVENT, selection).catch(() => {});
	}

	function signalInteraction() {
		if (!menuState) return;
		suppressBlurCloseUntil = Date.now() + 200;
		void currentWindow.emitTo('overlay', OVERLAY_MENU_INTERACT_EVENT).catch(() => {});
	}

	$effect(() => {
		let disposed = false;
		let unlistenShow: UnlistenFn | undefined;
		let unlistenHide: UnlistenFn | undefined;
		let unlistenFocus: UnlistenFn | undefined;

		const handleEscape = (event: KeyboardEvent) => {
			if (event.key !== 'Escape') return;
			event.preventDefault();
			void requestClose();
		};

		void (async () => {
			unlistenShow = await currentWindow.listen<OverlayMenuState>(OVERLAY_MENU_SHOW_EVENT, async (event) => {
				if (disposed) return;
				menuState = event.payload;
				suppressBlurCloseUntil = Date.now() + 200;
			});

			unlistenHide = await currentWindow.listen(OVERLAY_MENU_HIDE_EVENT, async () => {
				if (disposed) return;
				menuState = null;
				await currentWindow.hide().catch(() => {});
			});

			unlistenFocus = await currentWindow.onFocusChanged(({ payload: focused }) => {
				if (disposed || focused || !menuState) return;
				// Handing in a quest means going back to the game to do
				// it, so losing focus is the middle of the flow rather
				// than the end of it. A menu is dismissed by looking
				// away; this panel is dismissed by Cancel or Escape.
				if (menuState.kind === 'questHandIn') return;
				if (Date.now() < suppressBlurCloseUntil) return;
				void requestClose();
			});

			await currentWindow.emitTo('overlay', OVERLAY_MENU_READY_EVENT, { label: currentWindow.label }).catch(() => {});
		})();

		window.addEventListener('keydown', handleEscape);

		return () => {
			disposed = true;
			unlistenShow?.();
			unlistenHide?.();
			unlistenFocus?.();
			window.removeEventListener('keydown', handleEscape);
		};
	});
</script>

{#if menuState}
	<div
		class="overlay-menu-shell"
		bind:this={menuShell}
		role={menuState.kind === 'questHandIn' ? 'dialog' : 'menu'}
		tabindex="-1"
		style:width={`${popupWidth}px`}
		style:height={`${popupHeight}px`}
		onpointerdown={signalInteraction}
		onwheel={signalInteraction}
	>
		{#if menuState.kind === 'questHandIn'}
			<QuestHandInPanel
				initialState={menuState.handIn}
				onComplete={() => handleSelection({ kind: 'questHandIn', action: 'completed' })}
				onCancel={() => handleSelection({ kind: 'questHandIn', action: 'cancelled' })}
			/>
		{:else}
			<OverlayMenuPanel
				{menuState}
				onSelect={handleSelection}
				onActivitySelect={handleActivitySelection}
			/>
		{/if}
	</div>
{/if}

<style>
	.overlay-menu-shell {
		display: flex;
		align-items: stretch;
		justify-content: stretch;
	}
</style>
