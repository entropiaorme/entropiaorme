<script lang="ts">
	import { tick } from 'svelte';
	import type { EquipmentSearchResult, SetupComponentRole } from '$lib/api';
	import { Input, PickerInput } from '$lib/components';
	import { OPTIONAL_ROLES, ROLE_LABEL, type SetupDraft } from './compareModel.svelte';

	/**
	 * A setup edited in place, in its table row: the weapon, any parts, and
	 * a markup beside each limited item. The trailing blank row shows only
	 * the weapon search.
	 */
	let { draft, blank = false }: { draft: SetupDraft; blank?: boolean } = $props();

	const idPrefix = $derived(`compare-setup-${draft.id}`);

	function economyLine(item: EquipmentSearchResult): string {
		if (item.absorptionPercent) return `-${item.absorptionPercent}% decay`;
		const parts = [`D:${item.decay.toFixed(3)}`];
		if (item.ammoBurn > 0) parts.push(`A:${item.ammoBurn.toFixed(2)}`);
		return `${parts.join(' ')} PEC`;
	}

	/** Swap an item back into a search box, focused, to pick another. */
	async function change(role: SetupComponentRole) {
		draft.pickers[role].clear();
		await tick();
		document.getElementById(`${idPrefix}-${role}`)?.focus();
	}
</script>

{#snippet resultLine({ item }: { item: EquipmentSearchResult })}
	<span class="text-text truncate">{item.name}</span>
	<span class="text-xs text-text-tertiary tabular-nums shrink-0">{economyLine(item)}</span>
{/snippet}

{#snippet selectionLine({ item }: { item: EquipmentSearchResult })}
	<span class="text-text truncate flex-1">{item.name}</span>
{/snippet}

{#snippet item(role: SetupComponentRole)}
	{@const picker = draft.pickers[role]}
	{@const selected = picker.selected}
	<div class="flex min-w-0 items-center gap-3">
		{#if role !== 'weapon'}
			<span class="w-16 shrink-0 text-xs text-text-tertiary">{ROLE_LABEL[role]}</span>
		{/if}
		{#if selected}
			<button
				type="button"
				class="min-w-0 truncate text-left cursor-pointer hover:text-accent {role === 'weapon'
					? 'text-text'
					: 'text-xs text-text-secondary'}"
				title="Change {selected.name}"
				onclick={() => change(role)}
			>
				{selected.name}
			</button>
		{:else}
			<PickerInput
				id="{idPrefix}-{role}"
				model={picker}
				placeholder={blank ? 'Add a weapon…' : `Search ${ROLE_LABEL[role].toLowerCase()}s`}
				class="min-w-0 flex-1 max-w-md"
			>
				{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
				{#snippet selection({ item })}{@render selectionLine({ item })}{/snippet}
			</PickerInput>
		{/if}
		{#if selected?.isLimited}
			<label class="flex shrink-0 items-center gap-1.5 text-xs text-text-tertiary">
				<Input
					id="{idPrefix}-{role}-markup"
					type="number"
					align="right"
					min={100}
					step="any"
					class="h-7 w-20"
					value={draft.markupFor(role)}
					aria-label="{ROLE_LABEL[role]} markup percent"
					oninput={(event) => {
						const raw = (event.currentTarget as HTMLInputElement).value;
						draft.setMarkup(role, raw === '' ? null : Number(raw));
					}}
				/>
				%
			</label>
		{:else if selected}
			<!-- An unlimited item's markup is one-off capital, in PED. -->
			<label class="flex shrink-0 items-center gap-1.5 text-xs text-text-tertiary">
				+
				<Input
					id="{idPrefix}-{role}-capital"
					align="right"
					class="h-7 w-24"
					placeholder="0"
					value={draft.capitalTextFor(role)}
					aria-label="{ROLE_LABEL[role]} markup paid, PED"
					title="Markup paid over TT, e.g. 150k"
					oninput={(event) => draft.setCapitalText(role, (event.currentTarget as HTMLInputElement).value)}
				/>
				PED
			</label>
		{/if}
		{#if role !== 'weapon'}
			<button
				type="button"
				class="shrink-0 text-text-tertiary hover:text-text cursor-pointer"
				aria-label="Remove the {ROLE_LABEL[role].toLowerCase()}"
				title="Remove"
				onclick={() => draft.removePart(role)}
			>
				<svg width="8" height="8" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 1 L9 9 M9 1 L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
				</svg>
			</button>
		{/if}
	</div>
{/snippet}

<div class="min-w-0 space-y-1.5">
	{@render item('weapon')}
	{#if !blank}
		{#each OPTIONAL_ROLES as role (role)}
			{#if draft.isShown(role)}
				{@render item(role)}
			{/if}
		{/each}
		{#if OPTIONAL_ROLES.some((role) => !draft.isShown(role))}
			<div class="flex flex-wrap gap-x-3 gap-y-0.5">
				{#each OPTIONAL_ROLES as role (role)}
					{#if !draft.isShown(role)}
						<button
							type="button"
							class="text-xs text-text-tertiary hover:text-accent cursor-pointer"
							onclick={() => draft.show(role)}
						>
							+ {ROLE_LABEL[role]}
						</button>
					{/if}
				{/each}
			</div>
		{/if}
		{#if draft.problem}
			<p class="text-xs text-error">{draft.problem}</p>
		{/if}
	{/if}
</div>
