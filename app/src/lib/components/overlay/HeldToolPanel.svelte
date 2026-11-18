<script lang="ts">
	// The tool in hand in full, as a sub-window of the overlay's tool line:
	// its whole name, its set-up and what a use is priced at, what the
	// reading says about it, and, where the screen leaves it open, the
	// library entries to take it as.
	import type { OverlayHeldToolState, OverlayMenuSelection } from '$lib/windows/overlayMenu';
	import SubWindowPanel from './SubWindowPanel.svelte';

	let {
		panelState,
		onSelect,
	}: {
		panelState: OverlayHeldToolState;
		/** What the player chose: either way the panel is done with. */
		onSelect: (selection: OverlayMenuSelection) => void;
	} = $props();

	const setup = $derived(panelState.setup);
</script>

<SubWindowPanel heading="Tool" label="Tool in hand" testId="held-tool-panel">
	{#if setup}
		<p class="panel-value" data-testid="held-tool-name">{setup.name}</p>
	{/if}
	{#each panelState.lines as line (line)}
		<p class="panel-note" data-testid="held-tool-line">{line}</p>
	{/each}
	{#if setup}
		{#if setup.rows.length > 0}
			<dl class="setup-rows" data-testid="held-tool-setup">
				{#each setup.rows as row (row.label)}
					<dt class="panel-key">{row.label}</dt>
					<dd class="panel-value">{row.value}</dd>
				{/each}
			</dl>
		{/if}
		<div class="setup-cost" data-testid="held-tool-cost">
			<p><span class="panel-value">{setup.cost}</span> <span class="panel-key">{setup.per}</span></p>
			{#if setup.tt}<p class="panel-key">{setup.tt}</p>{/if}
		</div>
	{/if}
	{#if panelState.actions.length > 0}
		<div class="mt-2 flex flex-col gap-1.5">
			{#each panelState.actions as action, index (index)}
				{#if action.kind === 'declare'}
					<div class="flex items-center gap-2">
						<span class="panel-value min-w-0 flex-1">{action.name}</span>
						<button
							type="button"
							class="panel-chip shrink-0"
							disabled={panelState.declaring}
							aria-label={action.accessibleName}
							title="Take this as the tool in hand until the screen shows what is held"
							onclick={() =>
								onSelect({ kind: 'heldTool', action: 'declare', equipmentId: action.equipmentId })}
						>
							{action.label}
						</button>
					</div>
				{:else}
					<button
						type="button"
						class="panel-chip self-start"
						disabled={panelState.declaring}
						onclick={() => onSelect({ kind: 'heldTool', action: 'choose' })}
					>
						{action.label}
					</button>
				{/if}
			{/each}
		</div>
	{/if}
</SubWindowPanel>

<style>
	.setup-rows {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 2px 10px;
		margin: 8px 0 0;
	}
	.setup-rows dd {
		margin: 0;
		font-weight: 400;
	}
	.setup-cost {
		margin-top: 8px;
		padding-top: 8px;
		border-top: 1px solid rgba(255, 255, 255, 0.075);
	}
	.setup-cost p {
		margin: 0;
	}
</style>
