<script lang="ts">
	// The overlay's sub-window for an armour recording, inside the shared
	// shell. It stays while the player returns to the game to place items in
	// a terminal: only its own buttons, its control, or Escape close it.
	import SubWindowShell from '$lib/components/overlay/SubWindowShell.svelte';
	import { OVERLAY_ARMOUR_COST_CHANNEL, type OverlayArmourCostState } from '$lib/windows/overlayMenu';
	import ProtectionCostPanel from '../overlay/ProtectionCostPanel.svelte';
</script>

<SubWindowShell channel={OVERLAY_ARMOUR_COST_CHANNEL}>
	{#snippet children(held: { state: OverlayArmourCostState; close: () => Promise<void> })}
		<div class="armour-cost-panel">
			<ProtectionCostPanel
				repairOcrEnabled={held.state.repairOcrEnabled}
				onClose={() => void held.close()}
			/>
		</div>
	{/snippet}
</SubWindowShell>

<style>
	.armour-cost-panel {
		display: inline-flex;
		align-items: center;
		padding: 8px 10px;
		border-radius: 8px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(11, 15, 25, 0.96);
		backdrop-filter: blur(16px) saturate(150%);
	}
</style>
