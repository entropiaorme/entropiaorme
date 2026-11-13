<script lang="ts">
	// The overlay's sub-window for menus and panels: whichever one the
	// overlay asked for, inside the shared shell, which sizes, places,
	// reveals and closes it. This route only chooses the body and passes on
	// what the player picked.
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import { getManualMobSuggestions } from '$lib/api';
	import HeldToolPanel from '$lib/components/overlay/HeldToolPanel.svelte';
	import MobSourcePanel from '$lib/components/overlay/MobSourcePanel.svelte';
	import OverlayMenuPanel from '$lib/components/overlay/OverlayMenuPanel.svelte';
	import QuestHandInPanel from '$lib/components/overlay/QuestHandInPanel.svelte';
	import SubWindowPanel from '$lib/components/overlay/SubWindowPanel.svelte';
	import SubWindowShell from '$lib/components/overlay/SubWindowShell.svelte';
	import {
		OVERLAY_MENU_CHANNEL,
		OVERLAY_MENU_SELECT_EVENT,
		computeMenuHeight,
		isPanelState,
		menuRowCount,
		type OverlayMenuSelection,
		type OverlayMenuState,
	} from '$lib/windows/overlayMenu';

	const currentWindow = getCurrentWebviewWindow();

	function select(selection: OverlayMenuSelection) {
		return currentWindow.emitTo('overlay', OVERLAY_MENU_SELECT_EVENT, selection).catch(() => {});
	}
</script>

<SubWindowShell channel={OVERLAY_MENU_CHANNEL}>
	{#snippet children(held: { state: OverlayMenuState; close: () => Promise<void> })}
		{@const menuState = held.state}
		{@const pickAndClose = async (selection: OverlayMenuSelection) => {
			await select(selection);
			await held.close();
		}}
		{#if menuState.kind === 'mobSource'}
			<MobSourcePanel
				panelState={menuState}
				search={getManualMobSuggestions}
				onSelect={(selection) => void select(selection)}
				onDone={() => void held.close()}
			/>
		{:else if menuState.kind === 'heldTool'}
			<HeldToolPanel panelState={menuState} onSelect={(selection) => void pickAndClose(selection)} />
		{:else if menuState.kind === 'note'}
			<SubWindowPanel heading={menuState.heading} testId="note-panel">
				<p class="panel-note" data-testid="note-line">{menuState.text}</p>
			</SubWindowPanel>
		{:else if !isPanelState(menuState)}
			<div
				class="overlay-menu-shell"
				role={menuState.kind === 'questHandIn' ? 'dialog' : 'menu'}
				tabindex="-1"
				style:width={`${menuState.width}px`}
				style:height={`${computeMenuHeight(menuRowCount(menuState))}px`}
			>
				{#if menuState.kind === 'questHandIn'}
					<QuestHandInPanel
						initialState={menuState.handIn}
						onComplete={() => pickAndClose({ kind: 'questHandIn', action: 'completed' })}
						onCancel={() => pickAndClose({ kind: 'questHandIn', action: 'cancelled' })}
					/>
				{:else}
					<!-- An Activities action keeps the menu open: the overlay
						 brings its rows up to date where it stands, so declaring
						 one thing after another is not a close-and-reopen. -->
					<OverlayMenuPanel
						{menuState}
						onSelect={(selection) => void pickAndClose(selection)}
						onActivitySelect={(selection) => void select(selection)}
					/>
				{/if}
			</div>
		{/if}
	{/snippet}
</SubWindowShell>

<style>
	.overlay-menu-shell {
		display: flex;
		align-items: stretch;
		justify-content: stretch;
	}
</style>
