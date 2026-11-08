<script lang="ts">
	/**
	 * Record a hand-in, an opened box, anything where some things went in and
	 * something else came out.
	 *
	 * Reads top to bottom as what was used and what was got. A line asks only
	 * for what the app cannot work out: an item it can see in the inventory
	 * needs a quantity and nothing else. The sentence at the bottom says what
	 * recording this will do, so nothing here has to be learnt first.
	 */
	import type { ActivityHistoryEntry, ItemConversionInput } from '$lib/api/commands.gen';
	import Button from '$lib/components/Button.svelte';
	import Input from '$lib/components/Input.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { todayDate } from '$lib/utils/format';
	import ConversionItemField from './ConversionItemField.svelte';
	import {
		type ConversionDraft,
		type ConvertibleItem,
		draftFromEntry,
		emptyDraft,
		emptyInput,
		emptyOutput,
		excessOverHeld,
		heldQuantity,
		type InputDraft,
		inputKind,
		isHeldItem,
		lastConversionFor,
		type OutputDraft,
		previewConversion,
		shownInputTt,
		shownOutputTt,
		toConversionRequest,
		worthIsShown,
	} from './conversionDraft';

	let {
		open = $bindable(false),
		items,
		start = null,
		history,
		onsubmit,
	}: {
		open?: boolean;
		items: ConvertibleItem[];
		/** The holding this was opened from, already filled in. */
		start?: { name: string; assetId: string | null } | null;
		history: ActivityHistoryEntry[];
		onsubmit: (request: ItemConversionInput) => Promise<void>;
	} = $props();

	let draft = $state<ConversionDraft>(emptyDraft());
	let convertedAt = $state(todayDate());
	let editingDate = $state(false);
	let saving = $state(false);
	let error = $state<string | null>(null);
	let initialised = $state(false);

	$effect(() => {
		if (open && !initialised) {
			draft = emptyDraft(start);
			convertedAt = todayDate();
			editingDate = false;
			error = null;
			initialised = true;
		}
		if (!open) initialised = false;
	});

	const preview = $derived(previewConversion(draft, items));
	const showWorth = $derived(worthIsShown(draft, items));
	const repeatable = $derived(lastConversionFor(draft.inputs[0]?.itemName ?? '', history));
	const canRepeat = $derived(
		repeatable !== null &&
			draft.inputs.length === 1 &&
			draft.outputs.every((line) => line.itemName.trim() === ''),
	);

	// Weighted splits leave fractions no one wants to read.
	const roundedQuantity = (value: number) => Math.round(value * 100) / 100;

	function setInputItem(line: InputDraft, name: string, assetId: string | null) {
		line.itemName = name;
		line.assetId = assetId;
		line.bought = false;
	}

	function setOutputItem(line: OutputDraft, name: string) {
		line.itemName = name;
	}

	function removeInput(id: number) {
		draft.inputs = draft.inputs.filter((line) => line.id !== id);
	}

	function removeOutput(id: number) {
		draft.outputs = draft.outputs.filter((line) => line.id !== id);
	}

	async function confirm() {
		if (saving || !preview.ready) return;
		saving = true;
		error = null;
		try {
			await onsubmit(toConversionRequest(draft, items, convertedAt || null));
			open = false;
		} catch (cause) {
			error = cause instanceof Error ? cause.message : 'Failed to record the conversion';
		} finally {
			saving = false;
		}
	}
</script>

<Modal bind:open title="Convert items" class="max-w-2xl">
	<div class="space-y-5">
		<section aria-label="Used" class="space-y-2">
			<div class="flex items-baseline justify-between">
				<span class="eyebrow text-text-tertiary">Used</span>
				{#if canRepeat && repeatable}
					<button
						type="button"
						class="text-xs font-medium text-accent hover:text-text"
						onclick={() => {
							if (repeatable) draft = draftFromEntry(repeatable, items);
						}}
					>
						Same as last time
					</button>
				{/if}
			</div>

			{#each draft.inputs as line, index (line.id)}
				{@const kind = inputKind(line, items)}
				{@const excess = excessOverHeld(line, items)}
				<div class="space-y-1">
					<div class="flex items-center gap-2">
						<ConversionItemField
							id={`conversion-input-${line.id}`}
							value={line.itemName}
							{items}
							class="min-w-0 flex-1"
							onchange={(name, assetId) => setInputItem(line, name, assetId)}
						/>
						{#if kind !== 'asset'}
							<Input
								type="number"
								min="0"
								step="1"
								align="right"
								class="w-20 shrink-0"
								placeholder="Qty"
								aria-label={`Quantity of ${line.itemName || 'item'}`}
								bind:value={line.quantity}
							/>
						{/if}

						{#if kind === 'bought'}
							<Input
								type="number"
								min="0"
								step="0.01"
								align="right"
								class="w-24 shrink-0"
								placeholder="TT"
								aria-label={`TT of ${line.itemName || 'item'}`}
								bind:value={
									() => shownInputTt(line, items),
									(next) => {
										line.ttValue = next === null ? null : Number(next);
										line.ttEdited = true;
									}
								}
							/>
							<Input
								type="number"
								min="0"
								step="0.01"
								align="right"
								class="w-28 shrink-0"
								placeholder="Markup paid"
								aria-label={`Markup paid for ${line.itemName || 'item'}`}
								bind:value={line.markup}
							/>
						{:else if showWorth}
							<Input
								type="number"
								min="0"
								step="0.01"
								align="right"
								class="w-24 shrink-0"
								placeholder="Worth"
								title="What these would have fetched above TT. Leave it empty to share what is left."
								aria-label={`What ${line.itemName || 'this line'} is worth above TT`}
								bind:value={line.markup}
							/>
						{/if}

						{#if draft.inputs.length > 1}
							<button
								type="button"
								class="h-6 w-6 shrink-0 text-text-tertiary hover:text-text"
								aria-label={`Remove ${line.itemName || `line ${index + 1}`}`}
								onclick={() => removeInput(line.id)}
							>
								&times;
							</button>
						{/if}
					</div>

					<div class="flex items-baseline gap-3 pl-0.5 text-xs text-text-tertiary">
						{#if kind === 'held'}
							<button
								type="button"
								class="tabular-nums hover:text-text"
								onclick={() => (line.quantity = heldQuantity(line, items))}
							>
								of {roundedQuantity(heldQuantity(line, items))} held
							</button>
							<button
								type="button"
								class="hover:text-text"
								onclick={() => (line.bought = true)}
							>
								I bought these
							</button>
						{:else if kind === 'bought' && isHeldItem(line, items)}
							<button
								type="button"
								class="hover:text-text"
								onclick={() => (line.bought = false)}
							>
								Use what I hold
							</button>
						{/if}
						{#if excess > 0}
							<span>{roundedQuantity(excess)} beyond what is tracked won't credit an activity.</span>
						{/if}
					</div>
				</div>
			{/each}

			<button
				type="button"
				class="text-xs font-medium text-accent hover:text-text"
				onclick={() => draft.inputs.push(emptyInput())}
			>
				+ Add
			</button>
		</section>

		<section aria-label="Got" class="space-y-2 border-t border-border/50 pt-4">
			<span class="eyebrow text-text-tertiary">Got</span>

			{#each draft.outputs as line, index (line.id)}
				<div class="flex items-center gap-2">
					<ConversionItemField
						id={`conversion-output-${line.id}`}
						value={line.itemName}
						{items}
						class="min-w-0 flex-1"
						onchange={(name) => setOutputItem(line, name)}
					/>
					{#if line.holding === 'loot'}
						<Input
							type="number"
							min="0"
							step="1"
							align="right"
							class="w-20 shrink-0"
							placeholder="Qty"
							aria-label={`Quantity of ${line.itemName || 'result'}`}
							bind:value={line.quantity}
						/>
					{/if}
					<Input
						type="number"
						min="0"
						step="0.01"
						align="right"
						class="w-24 shrink-0"
						placeholder="TT"
						aria-label={`TT of ${line.itemName || 'result'}`}
						bind:value={
							() => shownOutputTt(line, items),
							(next) => {
								line.ttValue = next === null ? null : Number(next);
								line.ttEdited = true;
							}
						}
					/>
					<SegmentedControl
						options={[
							{ id: 'loot', label: 'Loot' },
							{ id: 'asset', label: 'Asset' },
						]}
						active={line.holding}
						onchange={(id) => (line.holding = id === 'asset' ? 'asset' : 'loot')}
					/>
					{#if draft.outputs.length > 1 || draft.showPed}
						<button
							type="button"
							class="h-6 w-6 shrink-0 text-text-tertiary hover:text-text"
							aria-label={`Remove ${line.itemName || `result ${index + 1}`}`}
							onclick={() => removeOutput(line.id)}
						>
							&times;
						</button>
					{/if}
				</div>
			{/each}

			{#if draft.showPed}
				<div class="flex items-center gap-2">
					<span class="min-w-0 flex-1 text-sm text-text-secondary">PED received</span>
					<Input
						type="number"
						min="0"
						step="0.01"
						align="right"
						class="w-28 shrink-0"
						placeholder="PED"
						aria-label="PED received"
						bind:value={draft.pedAmount}
					/>
					<button
						type="button"
						class="h-6 w-6 shrink-0 text-text-tertiary hover:text-text"
						aria-label="Remove PED amount"
						onclick={() => {
							draft.showPed = false;
							draft.pedAmount = null;
						}}
					>
						&times;
					</button>
				</div>
			{/if}

			<div class="flex items-baseline gap-4">
				<button
					type="button"
					class="text-xs font-medium text-accent hover:text-text"
					onclick={() => draft.outputs.push(emptyOutput())}
				>
					+ Add
				</button>
				{#if !draft.showPed}
					<button
						type="button"
						class="text-xs font-medium text-accent hover:text-text"
						onclick={() => {
							draft.showPed = true;
							// Writing the result as one figure usually replaces the
							// item rows, so an untouched one gets out of the way.
							draft.outputs = draft.outputs.filter((line) => line.itemName.trim() !== '');
						}}
					>
						+ PED amount
					</button>
				{/if}
			</div>
		</section>

		{#if preview.sentence}
			<p class="border-t border-border/50 pt-4 text-sm leading-relaxed text-text-secondary">
				{preview.sentence}
			</p>
		{/if}

		{#if error}
			<p class="text-xs text-error">{error}</p>
		{/if}

		<div class="flex items-center justify-end gap-2">
			{#if editingDate}
				<Input type="date" class="mr-auto w-40" aria-label="Date" bind:value={convertedAt} />
			{:else}
				<button
					type="button"
					class="mr-auto text-xs text-text-tertiary hover:text-text"
					onclick={() => (editingDate = true)}
				>
					Today
				</button>
			{/if}
			<Button variant="ghost" onclick={() => (open = false)} disabled={saving}>Cancel</Button>
			<Button onclick={confirm} loading={saving} disabled={!preview.ready}>Convert</Button>
		</div>
	</div>
</Modal>
