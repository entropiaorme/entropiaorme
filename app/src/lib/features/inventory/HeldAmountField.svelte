<script lang="ts">
	/**
	 * How much of a holding an action takes, entered as a count or as PED of
	 * TT, with the other form read out beneath. The count is the figure bound
	 * out: it is what every command takes (`heldAmount.ts` carries the maths).
	 *
	 * `field` is the full form for a modal built around the amount: the toggle
	 * sits on the label row (as the markup field's % and PED do) and the line
	 * beneath says what the amount comes to and offers all of it. `inline` fits
	 * a row of fields: the unit inside the field is the toggle.
	 *
	 * An item with no TT is entered by count alone; the field says so where the
	 * toggle would be.
	 */
	import Input from '$lib/components/Input.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import {
		type AmountMode,
		amountEquivalent,
		canEnterByPed,
		heldInMode,
		pedForUnits,
		unitsForPed,
	} from './heldAmount';

	let {
		units = $bindable<number | null>(null),
		mode = $bindable<AmountMode>('units'),
		unitTt,
		held = null,
		placeholder = null,
		label,
		subject,
		variant = 'field',
		describedby = undefined,
		class: className = '',
	}: {
		/** The units entered; null while the field is empty. */
		units?: number | null;
		mode?: AmountMode;
		/** The TT one unit carries; nothing for an item with no TT. */
		unitTt: number;
		/** What is held, for the way back to all of it; null when unknown. */
		held?: number | null;
		/** Units shown, greyed, while the field is empty. */
		placeholder?: number | null;
		/** The caption above a `field`; the toggle stands alone without one. */
		label?: string;
		/** What the amount is of, for assistive technology ("to remove"). */
		subject: string;
		variant?: 'field' | 'inline';
		/** The id of a line that says more about the amount (a reminder of
		 * what the account recorded), for assistive technology. */
		describedby?: string;
		class?: string;
	} = $props();

	const byPed = $derived(canEnterByPed(unitTt));
	const shown = $derived<AmountMode>(byPed ? mode : 'units');

	// What was typed in PED, kept while it still means the units bound out so
	// "1.5" is not rewritten to whatever the rounded units come back as.
	let pedDraft = $state<number | null>(null);

	const pedValue = $derived.by(() => {
		if (units === null) return null;
		if (pedDraft !== null && unitsForPed(pedDraft, unitTt, held) === units) return pedDraft;
		return pedForUnits(units, unitTt);
	});

	function setPed(next: number | string | null) {
		if (next === null || next === '' || !Number.isFinite(Number(next))) {
			pedDraft = null;
			units = null;
			return;
		}
		pedDraft = Number(next);
		units = unitsForPed(pedDraft, unitTt, held);
	}

	function setMode(next: AmountMode) {
		pedDraft = null;
		mode = next;
	}

	const equivalent = $derived(amountEquivalent(units, unitTt, shown));
	const atAll = $derived(held !== null && units !== null && Math.abs(units - held) < 1e-9);
	const pedPlaceholder = $derived(
		placeholder !== null ? pedForUnits(placeholder, unitTt).toString() : '0.00',
	);
	const ariaLabel = $derived(shown === 'ped' ? `PED ${subject}` : `How many ${subject}`);
</script>

{#snippet amountInput(inline: boolean)}
	{#if shown === 'ped'}
		<Input
			type="number"
			min="0"
			step="0.01"
			align="right"
			placeholder={pedPlaceholder}
			aria-label={ariaLabel}
			aria-describedby={describedby}
			bind:value={() => pedValue, setPed}
		>
			{#snippet suffix()}{@render unitMark(inline)}{/snippet}
		</Input>
	{:else}
		<Input
			type="number"
			min="0"
			step="1"
			align="right"
			placeholder={placeholder !== null ? placeholder.toString() : '0'}
			aria-label={ariaLabel}
			aria-describedby={describedby}
			bind:value={
				() => units,
				(next) => {
					pedDraft = null;
					units = next === null || !Number.isFinite(Number(next)) ? null : Number(next);
				}
			}
		>
			{#snippet suffix()}{@render unitMark(inline)}{/snippet}
		</Input>
	{/if}
{/snippet}

{#snippet unitMark(inline: boolean)}
	{#if inline && byPed}
		<button
			type="button"
			class="cursor-pointer text-[10px] font-medium uppercase tracking-wider text-text-tertiary
				transition-colors duration-[var(--duration-fast)] hover:text-accent"
			title={shown === 'ped' ? 'Enter how many instead' : 'Enter PED of TT instead'}
			aria-label={shown === 'ped'
				? `Entering PED ${subject}; switch to how many`
				: `Entering how many ${subject}; switch to PED`}
			onclick={() => setMode(shown === 'ped' ? 'units' : 'ped')}
		>
			{shown === 'ped' ? 'PED' : 'Qty'}
		</button>
	{:else}
		<span class="text-[10px] font-medium uppercase tracking-wider">
			{shown === 'ped' ? 'PED' : 'Qty'}
		</span>
	{/if}
{/snippet}

{#if variant === 'inline'}
	<div class={className}>{@render amountInput(true)}</div>
{:else}
	<div class="space-y-1.5 {className}">
		<div class="flex h-6 items-center justify-between gap-2">
			{#if label}<span class="eyebrow text-text-tertiary">{label}</span>{:else}<span></span>{/if}
			{#if byPed}
				<SegmentedControl
					label="Enter the amount as"
					options={[
						{ id: 'units', label: 'Qty', ariaLabel: 'By quantity' },
						{ id: 'ped', label: 'PED', ariaLabel: 'By PED of TT' },
					]}
					active={shown}
					onchange={(id) => setMode(id as AmountMode)}
				/>
			{:else}
				<span class="text-xs text-text-tertiary">No TT, so by count</span>
			{/if}
		</div>

		{@render amountInput(false)}

		<div class="flex items-baseline justify-between gap-2 text-xs">
			<span class="tabular-nums text-text-tertiary">{equivalent ?? ' '}</span>
			{#if held !== null}
				<button
					type="button"
					disabled={atAll}
					onclick={() => {
						pedDraft = null;
						units = held;
					}}
					class="cursor-pointer font-medium tabular-nums text-accent transition-colors
						duration-[var(--duration-fast)] hover:text-text disabled:cursor-default
						disabled:text-text-tertiary disabled:hover:text-text-tertiary"
				>
					All {heldInMode(held, unitTt, shown)}
				</button>
			{/if}
		</div>
	</div>
{/if}
