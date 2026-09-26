<script lang="ts">
	import type { ConsumableDose } from '$lib/api';
	import { doseState, formatCountdown, remainingSeconds, describeEffects } from './doses';
	import type { DosesModel } from './dosesModel.svelte';

	let {
		model,
		menuOpen = false,
		onStartTrigger
	}: {
		model: DosesModel;
		/** Whether the consumables menu is open over this section. */
		menuOpen?: boolean;
		/** Open (or close) the menu of consumables a dose can start from. */
		onStartTrigger: (anchor: HTMLElement) => void | Promise<void>;
	} = $props();

	// The section is absent, not empty, until there is something to show or
	// to start: an overlay for play without consumables stays as it was.
	const visible = $derived(
		model.rows.length > 0 || model.options.length > 0 || model.undoable !== null
	);

	function title(dose: ConsumableDose): string {
		const effects = describeEffects(dose.effects);
		return effects ? `${dose.itemName}: ${effects}` : dose.itemName;
	}
</script>

{#if visible}
	<div
		class="flex items-center gap-1.5 shrink-0 border-l border-white/10 pl-3"
		data-guide-anchor="overlay-doses-section"
		data-testid="overlay-doses"
	>
		<!-- A capsule: what is in force from a consumable. -->
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 16 16"
			fill="none"
			stroke="currentColor"
			stroke-width="1.3"
			class="h-4 w-4 shrink-0 text-white/40"
			aria-hidden="true"
		>
			<g transform="rotate(-35 8 8)">
				<rect x="2" y="5.25" width="12" height="5.5" rx="2.75" />
				<path d="M8 5.25v5.5" />
			</g>
		</svg>
		{#each model.rows as dose (dose.id)}
			{@const state = doseState(dose, model.now)}
			<div
				class="dose-chip {state === 'ended' ? 'dose-chip-ended' : ''}"
				title={title(dose)}
				data-testid="overlay-dose"
			>
				<span class="truncate max-w-[110px]">{dose.itemName}</span>
				{#if state === 'running'}
					<span class="tabular-nums text-white/55">{formatCountdown(remainingSeconds(dose, model.now))}</span>
					<button
						type="button"
						class="dose-btn"
						aria-label={`Remove this dose of ${dose.itemName}`}
						title="Remove this dose (a misclick): its effect and cost come off"
						disabled={model.busy !== null}
						onclick={() => void model.remove(dose)}
					>
						<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="h-2.5 w-2.5" aria-hidden="true">
							<path d="M5.28 4.22a.75.75 0 0 0-1.06 1.06L6.94 8l-2.72 2.72a.75.75 0 1 0 1.06 1.06L8 9.06l2.72 2.72a.75.75 0 1 0 1.06-1.06L9.06 8l2.72-2.72a.75.75 0 0 0-1.06-1.06L8 6.94 5.28 4.22Z" />
						</svg>
					</button>
				{:else}
					<span class="text-white/35">Ended</span>
					{#if dose.equipmentId !== null && model.options.some((option) => option.equipmentId === dose.equipmentId)}
						<button
							type="button"
							class="dose-btn"
							aria-label={`Take another dose of ${dose.itemName}`}
							title="Take another dose"
							disabled={model.busy !== null}
							onclick={() => dose.equipmentId !== null && void model.start(dose.equipmentId)}
						>
							<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="h-2.5 w-2.5" aria-hidden="true">
								<path fill-rule="evenodd" d="M13.836 2.477a.75.75 0 0 1 .75.75v3.182a.75.75 0 0 1-.75.75h-3.182a.75.75 0 0 1 0-1.5h1.37l-.84-.841a4.5 4.5 0 0 0-7.08.932.75.75 0 0 1-1.3-.75 6 6 0 0 1 9.44-1.242l.842.84V3.227a.75.75 0 0 1 .75-.75Zm-.911 7.5A.75.75 0 0 1 13.199 11a6 6 0 0 1-9.44 1.241l-.84-.84v1.371a.75.75 0 0 1-1.5 0V9.591a.75.75 0 0 1 .75-.75H5.35a.75.75 0 0 1 0 1.5H3.98l.841.841a4.5 4.5 0 0 0 7.08-.932.75.75 0 0 1 1.025-.273Z" clip-rule="evenodd" />
							</svg>
						</button>
					{/if}
				{/if}
			</div>
		{/each}
		{#if model.undoable}
			{@const removed = model.undoable.dose}
			<button
				type="button"
				class="dose-chip dose-undo"
				title={`Put the dose of ${removed.itemName} back`}
				disabled={model.busy !== null}
				onclick={() => void model.restore(removed)}
			>
				Removed · Undo
			</button>
		{/if}
		{#if model.options.length > 0}
			<button
				type="button"
				class="dose-btn dose-add {menuOpen ? 'dose-add-open' : ''}"
				aria-haspopup="menu"
				aria-expanded={menuOpen}
				aria-label="Take a dose"
				title="Take a dose of a consumable"
				disabled={model.busy !== null}
				onclick={(event) => onStartTrigger(event.currentTarget as HTMLElement)}
			>
				+
			</button>
		{/if}
	</div>
{/if}

<style>
	.dose-chip {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 2px 4px 2px 8px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.75);
		font-size: 11px;
		line-height: 1.35;
		white-space: nowrap;
	}
	.dose-chip-ended {
		color: rgba(255, 255, 255, 0.45);
		border-style: dashed;
	}
	.dose-undo {
		padding: 2px 8px;
		cursor: pointer;
		color: rgb(125, 211, 252);
		border-color: rgba(56, 189, 248, 0.35);
		background: rgba(56, 189, 248, 0.12);
	}
	.dose-undo:hover:not(:disabled),
	.dose-undo:focus-visible {
		background: rgba(56, 189, 248, 0.22);
		outline: none;
	}
	.dose-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 16px;
		height: 16px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.12);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.5);
		font-size: 11px;
		line-height: 1;
		cursor: pointer;
		transition: all 150ms ease-out;
	}
	.dose-btn:hover:not(:disabled),
	.dose-btn:focus-visible {
		background: rgba(255, 255, 255, 0.12);
		color: rgba(255, 255, 255, 0.95);
		border-color: rgba(255, 255, 255, 0.3);
		outline: none;
	}
	.dose-btn:disabled,
	.dose-undo:disabled {
		opacity: 0.35;
		cursor: default;
	}
	.dose-add {
		width: 18px;
		height: 18px;
	}
	.dose-add-open {
		background: rgba(56, 189, 248, 0.2);
		border-color: rgba(56, 189, 248, 0.4);
		color: rgb(125, 211, 252);
	}
</style>
