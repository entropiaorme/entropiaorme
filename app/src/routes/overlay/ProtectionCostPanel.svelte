<script lang="ts">
	import { onMount } from 'svelte';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import ArmourSessionPicker from '$lib/features/protection/ArmourSessionPicker.svelte';
	import { formatDay, formatSessionStart } from '$lib/features/protection/armourRecording';
	import { createArmourRecording } from '$lib/features/protection/armourRecordingModel.svelte';
	import { inDevelopment, InDevelopmentMark } from '$lib/inDevelopment';
	import { formatPed } from '$lib/utils/format';

	interface Props {
		repairOcrEnabled: boolean;
		onClose: () => void;
	}

	let { repairOcrEnabled, onClose }: Props = $props();

	const recording = createArmourRecording({
		repairOcrEnabled: () => repairOcrEnabled,
		limitedEnabled: () => inDevelopment.visible,
	});

	onMount(() => {
		void recording.load();
	});

	const limited = $derived(recording.stream.kind === 'limited');
	const terminal = $derived(limited ? 'Trade Terminal' : 'Repair Terminal');
	const canScan = $derived(limited || recording.repairOcrEnabled);
	const only = $derived(
		recording.spreads && recording.sessions.length === 1 && recording.earlier.length === 0
			? recording.sessions[0]
			: null,
	);
	const instruction = $derived.by(() => {
		if (!limited) return 'Place every unlimited armour piece and plate you are repairing in the Repair Terminal.';
		const items = recording.set?.kind === 'plates' ? 'seven plates' : 'seven armour pieces';
		return `Place the ${items} in the Trade Terminal. Do not sell them.`;
	});
	const confirmLabel = $derived(
		recording.increased ? 'Reset baseline' : recording.isBaseline ? 'Set baseline' : 'Record',
	);
</script>

<div class="flex w-[420px] flex-col gap-3 text-white" data-testid="armour-cost-panel">
	<div class="flex items-start justify-between gap-4 border-b border-white/10 pb-2.5">
		<div class="min-w-0">
			<div class="flex items-center gap-2">
				<span class="text-xs font-semibold">Record armour cost</span>
				{#if inDevelopment.visible}<InDevelopmentMark id="limited-protection" />{/if}
			</div>
			{#if recording.sets.length > 0}
				<div class="mt-2 flex flex-wrap items-center gap-1" role="radiogroup" aria-label="What you are recording">
					<button
						type="button"
						role="radio"
						aria-checked={!limited}
						class="stream-chip"
						class:stream-chip-active={!limited}
						disabled={recording.saving}
						onclick={() => recording.chooseStream({ kind: 'unlimited' })}
					>
						Unlimited repair
					</button>
					{#each recording.sets as set (set.id)}
						{@const active = recording.stream.kind === 'limited' && recording.stream.setId === Number(set.id)}
						<button
							type="button"
							role="radio"
							aria-checked={active}
							class="stream-chip"
							class:stream-chip-active={active}
							disabled={recording.saving}
							title={`${set.name}: limited ${set.kind === 'plates' ? 'plates' : 'armour'} at ${set.markupPercent.toFixed(2)}% MU`}
							onclick={() => recording.chooseStream({ kind: 'limited', setId: Number(set.id) })}
						>
							<span class="max-w-[120px] truncate">{set.name}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
		<Button variant="ghost" size="sm" onclick={onClose}>{recording.phase === 'saved' ? 'Done' : 'Close'}</Button>
	</div>

	{#if recording.phase === 'saved' && recording.result}
		{@const result = recording.result}
		<div class="flex items-center justify-between gap-6" role="status">
			<div>
				{#if result.kind === 'spread'}
					<p class="text-xs font-medium">{formatPed(result.costPed)} PED recorded</p>
					<p class="mt-1 text-[11px] text-white/45">
						Spread across {result.sessions} {result.sessions === 1 ? 'session' : 'sessions'} by hits taken.
					</p>
				{:else if result.kind === 'unattributed'}
					<p class="text-xs font-medium">{formatPed(result.costPed)} PED recorded</p>
					<p class="mt-1 text-[11px] text-white/45">No session carries it; it counts toward your overall costs only.</p>
				{:else if result.kind === 'baseline'}
					<p class="text-xs font-medium">Baseline set at {formatPed(result.ttValuePed)} PED</p>
					<p class="mt-1 text-[11px] text-white/45">The next reading measures what this set loses from here.</p>
				{:else}
					<p class="text-xs font-medium">Baseline reset to {formatPed(result.ttValuePed)} PED</p>
					<p class="mt-1 text-[11px] text-white/45">Nothing was recorded for the time before the reset.</p>
				{/if}
			</div>
			<Button size="sm" onclick={onClose}>Done</Button>
		</div>
	{:else}
		<p class="text-[11px] text-white/55">{instruction}</p>

		{#if recording.phase === 'ready'}
			<div class="flex items-center gap-2">
				{#if canScan}
					<Button size="sm" onclick={recording.scan}>Scan {terminal}</Button>
				{/if}
				<Button variant="secondary" size="sm" onclick={recording.enterManually}>Enter manually</Button>
				{#if recording.baselineTt !== null}
					<span class="ml-auto text-[10px] tabular-nums text-white/35">Baseline {formatPed(recording.baselineTt)} PED</span>
				{/if}
			</div>
		{:else if recording.phase === 'scanning'}
			<div class="py-2 text-center text-xs text-white/45 animate-pulse">Reading the {terminal}...</div>
		{:else}
			<div class="flex items-center gap-2">
				<Input
					class="w-28"
					bind:value={recording.value}
					type="text"
					inputmode="decimal"
					placeholder="0.00 PED"
					aria-label={limited ? 'Total TT value in PED' : 'Repair cost in PED'}
					disabled={recording.saving}
				/>
				{#if recording.rawText}<span class="max-w-[100px] truncate text-[10px] text-white/30" title={recording.rawText}>OCR: {recording.rawText}</span>{/if}
				{#if canScan}<Button variant="ghost" size="sm" onclick={recording.scan} disabled={recording.saving}>Re-scan</Button>{/if}
				{#if limited && recording.costPed !== null && !recording.isBaseline}
					<span class="ml-auto text-[10px] tabular-nums text-white/45" title="TT lost since the baseline, at this set's markup">
						{formatPed(recording.costPed)} PED cost
					</span>
				{/if}
			</div>
			{#if limited && !recording.calibrated}
				<p class="border-l-2 border-amber-400/70 pl-2 text-[10px] text-amber-200/80">Provisional coordinates were used. Check the value or enter it manually.</p>
			{/if}
			{#if recording.isBaseline}
				<p class="text-[10px] text-white/40">This is {recording.set?.name}'s first reading: it sets the starting point and records no cost.</p>
			{/if}
			{#if recording.increased}
				<div class="flex items-center gap-2 border-l-2 border-amber-400/70 pl-2">
					<span class="shrink-0 text-[10px] text-white/45">Above the baseline. Why?</span>
					<Input class="min-w-52" bind:value={recording.resetReason} placeholder="Pieces replaced or reading corrected" />
				</div>
			{/if}
		{/if}

		{#if recording.spreads && recording.phase !== 'scanning'}
			{#if recording.candidatesLoading && !recording.candidates}
				<p class="text-[10px] text-white/35 animate-pulse">Finding the sessions this covers...</p>
			{:else if recording.pickerVisible}
				<ArmourSessionPicker {recording} />
			{:else if only}
				<p class="text-[10px] text-white/40">
					Covers {only.definitionName ?? only.sessionName ?? 'your last session'}, {formatSessionStart(only.startedAt)} · {only.hitCount}
					{only.hitCount === 1 ? 'hit' : 'hits'}
				</p>
			{:else if recording.candidates && recording.sessions.length === 0}
				<p class="text-[10px] text-white/40">
					No hits recorded since the {limited ? 'baseline' : 'last repair'}{recording.candidates.since ? ` on ${formatDay(recording.candidates.since)}` : ''}; this cost will count toward your overall costs only.
				</p>
			{/if}
		{/if}

		{#if recording.notice}<p class="border-l-2 border-amber-400/70 pl-2 text-[10px] text-amber-200/80">{recording.notice}</p>{/if}

		{#if recording.phase === 'review'}
			<div class="flex items-center justify-end gap-3">
				{#if recording.spreads && recording.sessions.length > 0 && recording.chosen.length === 0}
					<span class="text-[10px] text-white/40">No session ticked: it will count toward overall costs only.</span>
				{/if}
				<Button size="sm" onclick={recording.confirm} disabled={!recording.canConfirm} loading={recording.saving}>{confirmLabel}</Button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.stream-chip {
		display: inline-flex;
		align-items: center;
		height: 1.375rem;
		padding: 0 0.5rem;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.04);
		font-size: 10px;
		color: rgba(255, 255, 255, 0.55);
		cursor: pointer;
		transition:
			background 120ms ease,
			color 120ms ease,
			border-color 120ms ease;
	}

	.stream-chip:hover {
		background: rgba(255, 255, 255, 0.08);
		color: rgba(255, 255, 255, 0.85);
	}

	.stream-chip:focus-visible {
		outline: 1px solid var(--color-accent);
		outline-offset: 1px;
	}

	.stream-chip-active {
		border-color: color-mix(in srgb, var(--color-accent) 45%, transparent);
		background: color-mix(in srgb, var(--color-accent) 18%, transparent);
		color: var(--color-accent);
	}

	.stream-chip:disabled {
		cursor: not-allowed;
		opacity: 0.5;
	}

	@media (prefers-reduced-motion: reduce) {
		.stream-chip {
			transition: none;
		}
	}
</style>
