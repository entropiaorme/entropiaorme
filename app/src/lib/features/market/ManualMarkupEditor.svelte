<script lang="ts">
	import { clearManualMarkup, setManualMarkup, type MarketHarvestItem } from '$lib/api';
	import { Button } from '$lib/components';
	import Input from '$lib/components/Input.svelte';
	import Menu from '$lib/components/Menu.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { manualEntryAge, manualEntryText } from '$lib/features/analytics/marketConfidence';
	import type { MarkupBasis } from '$lib/features/analytics/treeCuttingModel.svelte';
	import { formatPed, formatPercent } from '$lib/utils/format';
	import { describeError } from '$lib/view/errorState';
	import {
		type EntryKind,
		entryMarkupPct,
		initialKind,
		initialText,
		parseEntry,
		unitTt,
	} from './manualMarkupEntry';
	import { formatAge } from './overviewModel.svelte';

	/**
	 * Enter an item's markup by hand, from the row it is shown in: a
	 * percentage of TT, or PED above TT per unit. The entry is used wherever
	 * it is newer than the item's market import, and a newer import replaces
	 * it; clearing it falls back to the import. Every surface valuing the
	 * item re-reads on the market change event, so the host needs no reload.
	 */
	let {
		itemName,
		quantity,
		ttValue,
		market,
		applied = null,
		onChanged,
	}: {
		itemName: string;
		quantity: number;
		ttValue: number;
		/** The item's current market signals, with any standing entry. */
		market: MarketHarvestItem | undefined;
		/** The valuation the host applies now, to say what is in use. */
		applied?: { markupPct: number; basis: MarkupBasis } | null;
		onChanged?: () => void;
	} = $props();

	const KIND_OPTIONS = [
		{ id: 'percent', label: '% of TT', ariaLabel: 'Markup as a percentage of TT' },
		{ id: 'ped_per_unit', label: '+PED / unit', ariaLabel: 'Markup as PED above TT per unit' },
	];

	let kind = $state<EntryKind>('percent');
	let text = $state('');
	let error = $state<string | null>(null);
	let saving = $state(false);

	const perUnitTt = $derived(unitTt(quantity, ttValue));
	const manual = $derived(market?.manual ?? null);
	const parsed = $derived(parseEntry(kind, text));
	const preview = $derived.by(() => {
		if (!parsed.ok) return null;
		const pct = entryMarkupPct(kind, parsed.value, perUnitTt);
		if (pct !== null) return `${formatPercent(pct / 100)} of TT`;
		return `${formatPed(parsed.value)} PED per unit`;
	});

	/** What the host values the item at now, in a line. */
	const inUse = $derived.by(() => {
		const now = Date.now() / 1000;
		const pct = applied ? formatPercent(applied.markupPct / 100) : null;
		switch (applied?.basis) {
			case 'manual':
				return manual
					? `Using your ${manualEntryText(manual)}, entered ${manualEntryAge(manual, now)}`
					: `Using your ${pct}`;
			case 'market': {
				const when = market?.marketObservedAt
					? `, imported ${formatAge(market.marketObservedAt, now)}`
					: '';
				const older = manual ? ' (newer than your entry)' : '';
				return `Using the market's ${pct}${when}${older}`;
			}
			case 'nanocube':
				return market?.markupPct != null
					? `Using the ${pct} Nanocube floor: the market reading is below the confidence setting`
					: `Using the ${pct} Nanocube floor: no market reading`;
			case 'nanocube_mark':
				return `Marked as Nanocube (${pct}) in the live figures`;
			default:
				return market?.markupPct != null ? `Market ${formatPercent(market.markupPct / 100)}` : 'No market reading';
		}
	});

	function reset() {
		kind = initialKind(market);
		text = initialText(market, kind);
		error = null;
	}

	function selectKind(id: string) {
		kind = id as EntryKind;
		text = initialText(market, kind);
		error = null;
	}

	async function save(close: () => void) {
		if (!parsed.ok) {
			error = parsed.error;
			return;
		}
		saving = true;
		error = null;
		try {
			await setManualMarkup({
				itemName,
				kind,
				value: parsed.value,
				unitTtPed: kind === 'ped_per_unit' ? perUnitTt : null,
			});
			close();
			onChanged?.();
		} catch (cause) {
			error = describeError(cause, 'Could not save the markup');
		} finally {
			saving = false;
		}
	}

	async function clear(close: () => void) {
		saving = true;
		error = null;
		try {
			await clearManualMarkup(itemName);
			close();
			onChanged?.();
		} catch (cause) {
			error = describeError(cause, 'Could not clear the markup');
		} finally {
			saving = false;
		}
	}
</script>

<Menu overlay align="right" panelClass="w-72 p-3" initialFocus="first-input" ariaLabel="Update markup">
	{#snippet trigger({ open, toggle })}
		<button
			type="button"
			class="icon-button-row p-1 {manual ? 'text-accent' : ''}"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-label="Update markup for {itemName}"
			title="Update markup"
			onclick={() => {
				if (!open) reset();
				toggle();
			}}
		>
			<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-3.5 w-3.5" aria-hidden="true">
				<path d="M13.586 3.586a2 2 0 1 1 2.828 2.828l-.793.793-2.828-2.828.793-.793ZM11.379 5.793 3 14.172V17h2.828l8.38-8.379-2.83-2.828Z" />
			</svg>
		</button>
	{/snippet}
	{#snippet children({ close })}
		<form
			class="flex flex-col gap-2.5"
			onsubmit={(event) => {
				event.preventDefault();
				void save(close);
			}}
		>
			<div class="flex flex-col gap-0.5">
				<span class="truncate text-xs font-medium text-text">{itemName}</span>
				<span class="text-[11px] leading-relaxed text-text-tertiary" data-testid="markup-in-use">{inUse}</span>
			</div>
			<SegmentedControl options={KIND_OPTIONS} active={kind} onchange={selectKind} label="Markup form" />
			<Input
				bind:value={text}
				inputmode="decimal"
				align="right"
				aria-label={kind === 'percent' ? 'Markup percent' : 'PED above TT per unit'}
				placeholder={kind === 'percent' ? '120' : '2.50'}
				disabled={saving}
			>
				{#snippet suffix()}
					<span class="text-xs">{kind === 'percent' ? '%' : 'PED'}</span>
				{/snippet}
			</Input>
			{#if error}
				<span class="text-[11px] text-error" role="alert">{error}</span>
			{:else if preview}
				<span class="text-[11px] tabular-nums text-text-tertiary">= {preview}</span>
			{/if}
			<span class="text-[11px] leading-relaxed text-text-tertiary">
				Used wherever it is newer than a market import. A newer import replaces it.
			</span>
			<div class="flex items-center justify-between gap-2 pt-0.5">
				{#if manual}
					<Button variant="ghost" size="sm" type="button" disabled={saving} onclick={() => void clear(close)}>
						Clear mine
					</Button>
				{:else}
					<span></span>
				{/if}
				<Button variant="primary" size="sm" type="submit" loading={saving} disabled={!parsed.ok}>Save</Button>
			</div>
		</form>
	{/snippet}
</Menu>
