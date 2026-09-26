<script lang="ts">
	import { Button, ErrorNotice, Select } from '$lib/components';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import {
		describeDoseCost,
		describeEffects,
		describeReloadSpeed,
		describeSource,
		doseState,
		formatCountdown,
		formatDuration,
		remainingSeconds,
	} from './doses';
	import { createDosesModel } from './dosesModel.svelte';

	// The dashboard shows every dose in force, a heal's buff included, since
	// it is where the reload speed in effect is read in full.
	const model = createDosesModel({ includeOnUse: true });
	$effect(() => model.connect());
	$effect(() => {
		if (!model.ticking) return;
		return useVisiblePoll(model.tick, { intervalMs: 1000 });
	});

	let chosen = $state<string>('');
	const choice = $derived(
		model.options.find((option) => String(option.equipmentId) === chosen) ??
			model.options[0] ??
			null,
	);
	const visible = $derived(
		model.options.length > 0 || model.rows.length > 0 || model.undoable !== null,
	);
	const reloadLine = $derived(
		model.readout && model.readout.reloadSpeed.inEffectPercent !== 0
			? describeReloadSpeed(model.readout.reloadSpeed)
			: null,
	);
</script>

{#if visible}
	<section class="flex-shrink-0 px-1" data-testid="doses-panel" data-guide-anchor="dashboard-doses">
		<div class="flex items-baseline justify-between gap-4 mb-2">
			<h3 class="eyebrow">Doses</h3>
			{#if reloadLine}
				<span class="text-xs text-text-secondary tabular-nums" data-testid="doses-reload">{reloadLine}</span>
			{/if}
		</div>

		{#if model.error}
			<ErrorNotice message={model.error} onDismiss={model.dismissError} />
		{/if}

		{#if model.rows.length > 0}
			<ul class="divide-y divide-border/40">
				{#each model.rows as dose (dose.id)}
					{@const state = doseState(dose, model.now)}
					<li class="flex items-center gap-4 py-2" data-testid="dose-row">
						<div class="min-w-0 flex-1">
							<div class="text-sm text-text truncate">{dose.itemName}</div>
							<div class="text-xs text-text-tertiary truncate">
								{describeEffects(dose.effects) || 'No evaluated effect'} · {describeSource(dose)} · {describeDoseCost(dose)}
							</div>
						</div>
						{#if state === 'running'}
							<span class="text-sm tabular-nums text-text-secondary shrink-0">
								{formatCountdown(remainingSeconds(dose, model.now))}
							</span>
							{#if dose.source !== 'on_use'}
								<button
									type="button"
									class="linklet linklet-danger shrink-0"
									disabled={model.busy !== null}
									title="Remove this dose (a misclick): its effect and cost come off"
									onclick={() => void model.remove(dose)}
								>
									Remove
								</button>
							{/if}
						{:else}
							<span class="text-xs text-text-tertiary shrink-0">Ended</span>
							{#if dose.equipmentId !== null && model.options.some((option) => option.equipmentId === dose.equipmentId)}
								<button
									type="button"
									class="linklet shrink-0"
									disabled={model.busy !== null}
									onclick={() => dose.equipmentId !== null && void model.start(dose.equipmentId)}
								>
									Take another
								</button>
							{/if}
						{/if}
					</li>
				{/each}
			</ul>
		{/if}

		{#if model.undoable}
			{@const removed = model.undoable.dose}
			<div class="flex items-center gap-3 py-2 text-xs text-text-secondary">
				<span>Removed the dose of {removed.itemName}.</span>
				<button
					type="button"
					class="linklet"
					disabled={model.busy !== null}
					onclick={() => void model.restore(removed)}
				>
					Undo
				</button>
			</div>
		{/if}

		{#if model.options.length > 0}
			<div class="mt-2 flex items-center gap-2">
				<Select class="w-72" bind:value={chosen} aria-label="Consumable to take">
					{#each model.options as option (option.equipmentId)}
						<option value={String(option.equipmentId)}>
							{option.name} · {formatDuration(option.durationSeconds)}{option.hotbarSlot
								? ` · key ${option.hotbarSlot}`
								: ''}
						</option>
					{/each}
				</Select>
				<Button
					size="sm"
					variant="secondary"
					disabled={choice === null || model.busy !== null}
					onclick={() => choice && void model.start(choice.equipmentId)}
				>
					Take a dose
				</Button>
			</div>
		{/if}
	</section>
{/if}
