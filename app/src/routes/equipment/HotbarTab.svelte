<script lang="ts">
	import type { Equipment } from '$lib/types';
	import type { Hotbar, HotbarPreset } from '$lib/types/settings';
	import { Divider, Select } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { NO_DATA } from '$lib/utils/format';
	import CarriedWeapons from '$lib/features/equipment/CarriedWeapons.svelte';
	import DamageRanges from '$lib/features/equipment/DamageRanges.svelte';
	import {
		addableWeapons,
		carriedWeapons,
		parkedHotbars,
		weaponBands
	} from '$lib/features/equipment/carriedWeapons';
	import {
		enabledCarried,
		hotbarReadingNote,
		slotIdleWhileReading
	} from '$lib/features/equipment/toolReading';

	let {
		equipment,
		hotbar,
		hotbarPresets,
		activeHotbarPreset,
		carriedWeaponIds,
		hotbarHooksEnabled,
		keyless = false,
		toolReading = false,
		lookalikeLabels = {},
		onassign,
		oncarriedchange
	}: {
		equipment: Equipment[];
		/** The live preset's slots. */
		hotbar: Hotbar;
		hotbarPresets: Hotbar[];
		activeHotbarPreset: HotbarPreset;
		carriedWeaponIds: number[];
		/** The hotbar key listener setting: presses declare the weapon in hand. */
		hotbarHooksEnabled: boolean;
		/** This machine runs keyless: the overlay declares the weapon. */
		keyless?: boolean;
		/** The tool in hand is read off the screen: the keys start consumables
		 * only, and the weapons on are the ones carried. */
		toolReading?: boolean;
		/** How entries the held-tool bar cannot tell apart are told apart. */
		lookalikeLabels?: Record<string, string>;
		onassign: (slot: string, equipmentId: number | null) => void;
		oncarriedchange?: (ids: number[]) => void;
	} = $props();

	let weapons = $derived(equipment.filter((e) => e.type === 'weapon'));
	let healingTools = $derived(equipment.filter((e) => e.type === 'healing'));
	let consumablesList = $derived(equipment.filter((e) => e.type === 'consumable'));
	let harvestingToolsList = $derived(equipment.filter((e) => e.type === 'tool'));

	// The weapons attribution chooses among: every preset's and the ones
	// carried without a hotkey, with the damage band each is checked against.
	let parked = $derived(parkedHotbars(hotbarPresets, activeHotbarPreset));
	let carried = $derived(
		toolReading
			? enabledCarried(equipment)
			: carriedWeapons(equipment, hotbar, carriedWeaponIds, parked)
	);
	let unslotted = $derived(
		carried.filter((entry) => entry.slot === null && entry.parked === null).map((entry) => entry.weapon)
	);
	let addable = $derived(addableWeapons(equipment, hotbar, carriedWeaponIds, parked));
	let bands = $derived(weaponBands(carried));

	function slotItem(slot: string): Equipment | null {
		const id = hotbar[slot];
		if (id === null || id === undefined) return null;
		return equipment.find((e) => String(e.id) === String(id)) ?? null;
	}

	function slotName(slot: string): string | null {
		return slotItem(slot)?.name ?? null;
	}

	function slotCost(slot: string): number | null {
		return slotItem(slot)?.costPerUse ?? null;
	}

	function slotType(slot: string): 'weapon' | 'healing' | 'consumable' | 'tool' | null {
		return slotItem(slot)?.type ?? null;
	}

	function assignSlot(slot: string, value: string) {
		const id = Number.parseInt(value, 10);
		onassign(slot, Number.isNaN(id) ? null : id);
	}
</script>

<div class="space-y-4">
	{#if toolReading}
		<div class="flex items-center gap-2 max-w-2xl">
			<p class="text-xs text-text-tertiary" data-testid="hotbar-listener-state">
				{hotbarReadingNote({ keyless, hotbarHooksEnabled })}
			</p>
			<InDevelopmentMark id="held-tool-reading" align="left" />
		</div>
	{:else}
		<p class="text-xs text-text-tertiary max-w-2xl" data-testid="hotbar-listener-state">
			{#if keyless}
				Keyless tracking is on for this machine: pick the weapon in hand from the overlay's
				equipment glyph, and each weapon's damage range checks it.
			{:else if hotbarHooksEnabled}
				The hotbar key listener is on: a press tells the app which weapon is in hand, and each
				weapon's damage range checks it.
			{:else}
				The hotbar key listener is off, so shots are attributed by damage range alone.
				<a href="/settings#cost-attribution" class="linklet">Turn it on in Settings</a> for exact
				switching.
			{/if}
		</p>
	{/if}

	<div>
		{#if weapons.length === 0 && healingTools.length === 0 && consumablesList.length === 0 && harvestingToolsList.length === 0}
			<p class="text-sm text-text-tertiary py-4">
				Add equipment in the Library tab first, then assign them to hotbar slots here.
			</p>
		{:else}
			<div class="space-y-1">
				{#each ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'] as slot}
					{@const name = slotName(slot)}
					{@const cost = slotCost(slot)}
					{@const type = slotType(slot)}
					{@const idle = toolReading && slotIdleWhileReading(type)}
					<div
						class="flex items-center gap-3 px-3 py-2.5 rounded-md
							{name ? 'bg-surface-hover/30' : 'hover:bg-surface-hover/20'}
							transition-colors duration-[var(--duration-fast)]"
					>
						<!-- Slot key badge -->
						<div
							class="shrink-0 h-7 w-7 rounded-md flex items-center justify-center text-xs font-bold
								{idle
								? 'bg-surface text-text-tertiary border border-border'
								: name
								? type === 'healing'
									? 'bg-emerald-500/15 text-emerald-400 border border-emerald-500/30'
									: type === 'consumable'
										? 'bg-amber-500/15 text-amber-400 border border-amber-500/30'
										: 'bg-accent/15 text-accent border border-accent/30'
								: 'bg-surface text-text-tertiary border border-border'}"
						>
							{slot}
						</div>

						<!-- Equipment selector -->
						<div class="flex-1 min-w-0">
							<Select
								value={hotbar[slot] != null ? String(hotbar[slot]) : ''}
								onchange={(e) => assignSlot(slot, e.currentTarget.value)}
							>
								<option value="">— Empty slot —</option>
								{#if toolReading}
									<!-- The keys start consumables only; a weapon or tool
										 already here stays for when the reading is off. -->
									{#if idle}
										<option value={String(hotbar[slot])}>{lookalikeLabels[String(hotbar[slot])] ?? name}</option>
									{/if}
								{:else if weapons.length > 0}
									<optgroup label="Weapons">
										{#each weapons as w}
											<option value={w.id}>
												{w.name}
												{#if w.amplifierName}+ {w.amplifierName}{/if}
											</option>
										{/each}
									</optgroup>
								{/if}
								{#if healingTools.length > 0}
									<optgroup label="Healing Tools">
										{#each healingTools as h}
											<option value={h.id}>{h.name}</option>
										{/each}
									</optgroup>
								{/if}
								{#if !toolReading && harvestingToolsList.length > 0}
									<optgroup label="Harvesting Tools">
										{#each harvestingToolsList as t}
											<option value={t.id}>{t.name}</option>
										{/each}
									</optgroup>
								{/if}
								{#if consumablesList.length > 0}
									<optgroup label="Consumables">
										{#each consumablesList as c}
											<option value={c.id}>{c.name}</option>
										{/each}
									</optgroup>
								{/if}
							</Select>
							{#if idle}
								<p class="mt-1 text-xs text-text-tertiary" data-testid="slot-idle">
									not used while the screen is read
								</p>
							{/if}
						</div>

						<!-- Cost display -->
						<div class="shrink-0 w-20 text-right">
							{#if idle}
								<span class="text-xs text-text-tertiary">{NO_DATA}</span>
							{:else if type === 'consumable'}
								<!-- The key is the dose: what one press books. -->
								{#if cost !== null && cost > 0}
									<span class="text-sm font-medium tabular-nums text-text">{(cost / 100).toFixed(2)}</span>
									<span class="text-xs text-text-tertiary ml-0.5">PED</span>
								{:else}
									<span class="text-xs text-text-tertiary" title="This item's doses book no cost">not booked</span>
								{/if}
							{:else if cost !== null}
								<span class="text-sm font-medium tabular-nums text-text">
									{cost.toFixed(2)}
								</span>
								<span class="text-xs text-text-tertiary ml-0.5">PEC</span>
							{:else}
								<span class="text-xs text-text-tertiary">—</span>
							{/if}
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>

	{#if weapons.length > 0}
		<Divider />
		{#if !toolReading}
			<!-- While the screen is read the weapons on are the carried ones. -->
			<CarriedWeapons
				carried={unslotted}
				{addable}
				carriedIds={carriedWeaponIds}
				onchange={oncarriedchange}
			/>
			<Divider />
		{/if}
		<DamageRanges banded={bands.banded} bandless={bands.bandless} />
	{/if}
</div>
