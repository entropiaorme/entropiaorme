<script lang="ts">
	/**
	 * Weapons carried without a hotkey: ones the player switches to from the
	 * inventory rather than the hotbar. Listing them lets damage evidence
	 * recognise their hits when no hotbar press announced them.
	 */
	import { updateSettings } from '$lib/api';
	import { ErrorNotice, Select } from '$lib/components';
	import type { Equipment } from '$lib/types';
	import { describeError } from '$lib/view/errorState';

	let {
		carried,
		addable,
		carriedIds,
		enabled = true,
		onchange,
	}: {
		/** The weapons carried without a slot, in the stored order. */
		carried: Equipment[];
		/** The weapons that could be added. */
		addable: Equipment[];
		carriedIds: number[];
		enabled?: boolean;
		onchange?: (ids: number[]) => void;
	} = $props();

	let saving = $state(false);
	let error = $state<string | null>(null);

	async function persist(next: number[]) {
		if (!enabled || saving) return;
		saving = true;
		error = null;
		try {
			const settings = await updateSettings({ carried_weapon_ids: next });
			onchange?.(settings.carriedWeaponIds);
		} catch (e) {
			error = describeError(e, 'Could not save the carried weapons');
		} finally {
			saving = false;
		}
	}

	function add(value: string) {
		const id = Number.parseInt(value, 10);
		if (Number.isNaN(id) || carriedIds.includes(id)) return;
		void persist([...carriedIds, id]);
	}

	function remove(id: string) {
		void persist(carriedIds.filter((carriedId) => String(carriedId) !== id));
	}
</script>

<section aria-labelledby="carried-weapons-heading" class="space-y-2" data-guide-anchor="carried-weapons">
	<div>
		<h3 id="carried-weapons-heading" class="eyebrow">Carried without a hotkey</h3>
		<p class="mt-1 text-xs text-text-tertiary max-w-xl">
			Weapons you switch to from the inventory. Their damage ranges let a hit be recognised
			when no hotbar press announced it.
		</p>
	</div>

	<ErrorNotice message={error} onDismiss={() => (error = null)} />

	{#if carried.length > 0}
		<ul class="divide-y divide-border/40">
			{#each carried as weapon (weapon.id)}
				<li class="flex items-center gap-3 py-2">
					<span class="min-w-0 flex-1 truncate text-sm text-text">{weapon.name}</span>
					{#if weapon.costPerUse != null}
						<span class="shrink-0 text-sm tabular-nums text-text">
							{weapon.costPerUse.toFixed(2)}<span class="ml-0.5 text-xs text-text-tertiary">PEC</span>
						</span>
					{/if}
					<button
						type="button"
						class="linklet shrink-0 text-xs disabled:opacity-40"
						disabled={!enabled || saving}
						aria-label={`Stop carrying ${weapon.name}`}
						onclick={() => remove(weapon.id)}
					>
						Remove
					</button>
				</li>
			{/each}
		</ul>
	{/if}

	{#if addable.length > 0}
		<div class="max-w-xs">
			<Select
				value=""
				aria-label="Carry a weapon without a hotkey"
				disabled={!enabled || saving}
				onchange={(event) => {
					const select = event.currentTarget;
					add(select.value);
					select.value = '';
				}}
			>
				<option value="">Add a weapon…</option>
				{#each addable as weapon (weapon.id)}
					<option value={weapon.id}>{weapon.name}</option>
				{/each}
			</Select>
		</div>
	{:else if carried.length === 0}
		<p class="text-xs text-text-tertiary">Every weapon in the library is already on the hotbar.</p>
	{/if}
</section>
