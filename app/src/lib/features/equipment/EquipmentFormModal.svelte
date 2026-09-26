<script lang="ts">
	import { Button, Input, Modal, PickerInput, SegmentedControl } from '$lib/components';
	import type { EquipmentSearchResult } from '$lib/api';
	import { formatFactor } from './attackRate';
	import { formatPec } from './display';
	import type { EquipmentFormType, LibraryModel } from './libraryModel.svelte';
	import { WEAPON_EFFECT_OPTIONS, type WeaponEffectForm } from './weaponEffect';

	let { model }: { model: LibraryModel } = $props();

	const saveDisabled = $derived(
		(model.addType === 'weapon'
			? !model.weaponPicker.selected || model.weaponEffectProblem !== null
			: model.addType === 'healing'
				? !model.healerPicker.selected
				: model.addType === 'tool'
					? !model.toolPicker.selected
					: !model.consumablePicker.selected) ||
			(model.addType === 'healing' &&
				((model.healingMode !== 'over_time' &&
					(model.healMin === null || model.healMax === null)) ||
					(model.healingMode !== 'direct' &&
						(model.effectDurationSeconds === null ||
							model.effectDurationSeconds <= 0 ||
							model.tickMin === null ||
							model.tickMax === null)))) ||
			model.saving,
	);

	// The add-custom row appears once the query could have searched (two or
	// more characters) and no catalogue hit matches it exactly.
	const showConsumableCustomRow = $derived.by(() => {
		const trimmed = model.consumablePicker.query.trim();
		return (
			trimmed.length >= 2 &&
			!model.consumablePicker.results.some((r) => r.name.toLowerCase() === trimmed.toLowerCase())
		);
	});

	// One-line economy summary for a catalogue row: the absorption share for
	// split devices, decay (plus ammo when it burns any) for the rest.
	function economyLine(item: EquipmentSearchResult): string {
		if (item.absorptionPercent) return `-${item.absorptionPercent}% decay`;
		const parts = [`D:${item.decay.toFixed(3)}`];
		if (item.ammoBurn > 0) parts.push(`A:${item.ammoBurn.toFixed(2)}`);
		return `${parts.join(' ')} PEC`;
	}
</script>

{#snippet resultLine({ item }: { item: EquipmentSearchResult })}
	<span class="text-text truncate">{item.name}</span>
	<span class="text-xs text-text-tertiary tabular-nums shrink-0">{economyLine(item)}</span>
{/snippet}

{#snippet selectionLine(item: EquipmentSearchResult)}
	<span class="text-text font-medium truncate flex-1">{item.name}</span>
	<span class="text-xs text-text-tertiary tabular-nums shrink-0">{economyLine(item)}</span>
{/snippet}

{#snippet consumableCustomRow()}
	<button
		type="button"
		class="w-full text-left px-3 py-2 text-sm hover:bg-surface-hover
			transition-colors duration-[var(--duration-fast)] cursor-pointer
			border-t border-border/50"
		onclick={() => model.selectConsumableCustom(model.consumablePicker.query)}
	>
		<span class="text-text-secondary">Add custom: </span>
		<span class="text-text font-medium">{model.consumablePicker.query.trim()}</span>
	</button>
{/snippet}

<!-- Add Equipment Modal: a two-column loadout builder. The primary item
     and its pricing sit left; companion attachments sit right; the live
     cost and actions form a fixed summary bar so the whole form stays
     within one screen. -->
<Modal bind:open={model.showAddModal} title={model.editingEquipmentId ? 'Edit Equipment' : 'Add Equipment'} class="max-w-3xl">
	<div class="flex flex-col">
		<div class="space-y-5">
			<!-- Type toggle -->
			{#if !model.editingEquipmentId}
				<SegmentedControl
					size="md"
					options={[
						{ id: 'weapon', label: 'Weapon' },
						{ id: 'healing', label: 'Healing Tool' },
						{ id: 'tool', label: 'Harvesting Tool' },
						{ id: 'consumable', label: 'Consumable' }
					]}
					active={model.addType}
					onchange={(id) => model.setAddType(id as EquipmentFormType)}
				/>
			{/if}

			{#if model.addType === 'weapon'}
				<div class="grid md:grid-cols-2 gap-x-8 gap-y-4 items-start">
					<!-- Column: the item and its pricing -->
					<div class="space-y-4">
						<div>
							<label for="equipment-weapon-search" class="block eyebrow mb-1.5">
								Weapon
							</label>
							<PickerInput id="equipment-weapon-search" model={model.weaponPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.weaponPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-item-markup" class="text-xs text-text-tertiary">Weapon markup %</label>
									<Input id="equipment-item-markup" type="number" bind:value={model.markupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>

						<div>
							<label for="equipment-amp-search" class="block eyebrow mb-1.5">
								Amplifier
							</label>
							<PickerInput id="equipment-amp-search" model={model.ampPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.ampPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-amp-markup" class="text-xs text-text-tertiary">Amplifier markup %</label>
									<Input id="equipment-amp-markup" type="number" bind:value={model.ampMarkupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>

						<div class="grid grid-cols-2 gap-4">
							<div>
								<label for="equipment-damage-enhancers" class="block eyebrow mb-1.5">
									Damage Enhancer Tier
								</label>
								<Input id="equipment-damage-enhancers" type="number" bind:value={model.damageEnhancers} min={0} class="w-full" />
							</div>
						</div>
						{#if model.weaponPicker.selected?.isLimited || model.ampPicker.selected?.isLimited || model.scopePicker.selected?.isLimited || model.absorberPicker.selected?.isLimited || model.implantPicker.selected?.isLimited}
							<p class="text-xs text-text-tertiary">
								Markup is the replacement cost of limited items: 200% means each PEC of decay costs 2 PEC to replace.
							</p>
						{/if}
					</div>

					<!-- Column: attachments -->
					<div class="space-y-4">
						<div>
							<label for="equipment-scope-search" class="block eyebrow mb-1.5">
								Scope
							</label>
							<PickerInput id="equipment-scope-search" model={model.scopePicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.scopePicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-scope-markup" class="text-xs text-text-tertiary">Scope markup %</label>
									<Input id="equipment-scope-markup" type="number" bind:value={model.scopeMarkupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>

						<div>
							<label for="equipment-absorber-search" class="block eyebrow mb-1.5">
								Extender / Absorber
							</label>
							<PickerInput id="equipment-absorber-search" model={model.absorberPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.absorberPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-absorber-markup" class="text-xs text-text-tertiary">Absorber markup %</label>
									<Input id="equipment-absorber-markup" type="number" bind:value={model.absorberMarkupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>

						<div>
							<label for="equipment-implant-search" class="block eyebrow mb-1.5">
								Mindforce implant
							</label>
							<PickerInput id="equipment-implant-search" model={model.implantPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.implantPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-implant-markup" class="text-xs text-text-tertiary">Implant markup %</label>
									<Input id="equipment-implant-markup" type="number" bind:value={model.implantMarkupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>
					</div>
				</div>

				<!-- Damage over time: what one paid cast prints, and for how long its
				     ticks follow. Ticks are outcomes of the cast, never shots. -->
				<div class="space-y-3 border-t border-border/50 pt-4" data-guide-anchor="weapon-effect">
					<div class="flex flex-wrap items-start justify-between gap-3">
						<div class="min-w-0">
							<span class="block eyebrow">Damage pattern</span>
							<p class="mt-1 max-w-md text-xs text-text-tertiary">
								For a weapon whose cast keeps dealing damage after it lands. Its ticks cost
								nothing and never count as shots.
							</p>
						</div>
						<SegmentedControl
							size="sm"
							options={WEAPON_EFFECT_OPTIONS}
							active={model.weaponEffectMode}
							onchange={(id) => (model.weaponEffectMode = id as WeaponEffectForm)}
						/>
					</div>
					{#if model.weaponEffectMode !== 'direct'}
						<div class="grid grid-cols-2 gap-3 md:grid-cols-4">
							{#if model.weaponEffectMode === 'compound'}
								<label class="text-xs text-text-tertiary">
									Initial hit minimum
									<Input type="number" bind:value={model.hitMin} min={0} step="any" class="mt-1 w-full" />
								</label>
								<label class="text-xs text-text-tertiary">
									Initial hit maximum
									<Input type="number" bind:value={model.hitMax} min={0} step="any" class="mt-1 w-full" />
								</label>
							{/if}
							<label class="text-xs text-text-tertiary">
								Tick minimum
								<Input type="number" bind:value={model.tickMin} min={0} step="any" class="mt-1 w-full" />
							</label>
							<label class="text-xs text-text-tertiary">
								Tick maximum
								<Input type="number" bind:value={model.tickMax} min={0} step="any" class="mt-1 w-full" />
							</label>
							<label class="text-xs text-text-tertiary">
								Duration (seconds)
								<Input type="number" bind:value={model.effectDurationSeconds} min={0.1} max={600} step="any" class="mt-1 w-full" />
							</label>
							<label class="text-xs text-text-tertiary">
								Tick cadence (optional)
								<Input type="number" bind:value={model.tickSeconds} min={0.1} step="any" class="mt-1 w-full" />
							</label>
						</div>
						<p class="text-xs text-text-tertiary" data-testid="weapon-effect-hint">
							{model.weaponEffectProblem ??
								'Use the damage the game prints. The catalogue carries no figures for effects over time.'}
						</p>
					{/if}
				</div>
			{:else if model.addType === 'healing'}
				<div class="grid md:grid-cols-2 gap-x-8 gap-y-5 items-start">
					<div class="space-y-4">
						<div>
							<label for="equipment-healer-search" class="block eyebrow mb-1.5">
								Healing Tool
							</label>
							<PickerInput id="equipment-healer-search" model={model.healerPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.healerPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-heal-markup" class="text-xs text-text-tertiary">Healer markup %</label>
									<Input id="equipment-heal-markup" type="number" bind:value={model.markupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>
						<div>
							<span class="block eyebrow mb-1.5">Output pattern</span>
							<SegmentedControl
								size="sm"
								options={[
									{ id: 'direct', label: 'Direct' },
									{ id: 'over_time', label: 'Over time' },
									{ id: 'compound', label: 'Direct + over time' }
								]}
								active={model.healingMode}
								onchange={(id) => (model.healingMode = id as 'direct' | 'over_time' | 'compound')}
							/>
						</div>
						{#if model.healingMode !== 'over_time'}
							<div>
								<span class="block eyebrow mb-1.5">Direct heal interval</span>
								<div class="grid grid-cols-2 gap-3">
									<label class="text-xs text-text-tertiary">
										Minimum
										<Input type="number" bind:value={model.healMin} min={0} step="any" class="mt-1 w-full" />
									</label>
									<label class="text-xs text-text-tertiary">
										Maximum
										<Input type="number" bind:value={model.healMax} min={0} step="any" class="mt-1 w-full" />
									</label>
								</div>
								<p class="mt-1.5 text-xs text-text-tertiary">Catalogue defaults can be corrected for your effective heal range.</p>
							</div>
						{/if}
						{#if model.healerPicker.selected?.isLimited || model.implantPicker.selected?.isLimited}
							<p class="text-xs text-text-tertiary">
								Markup is the replacement cost of limited items: 200% means each PEC of decay costs 2 PEC to replace.
							</p>
						{/if}
					</div>
					<div class="space-y-4">
						<div>
							<label for="equipment-heal-implant-search" class="block eyebrow mb-1.5">
								Mindforce implant
							</label>
							<PickerInput id="equipment-heal-implant-search" model={model.implantPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.implantPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-heal-implant-markup" class="text-xs text-text-tertiary">Implant markup %</label>
									<Input id="equipment-heal-implant-markup" type="number" bind:value={model.implantMarkupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>
						{#if model.healingMode !== 'direct'}
							<div>
								<span class="block eyebrow mb-1.5">Over-time effect</span>
								<div class="grid grid-cols-2 gap-3">
									<label class="text-xs text-text-tertiary">
										Duration (seconds)
										<Input type="number" bind:value={model.effectDurationSeconds} min={0.1} step="any" class="mt-1 w-full" />
									</label>
									<label class="text-xs text-text-tertiary">
										Tick cadence (optional)
										<Input type="number" bind:value={model.tickSeconds} min={0.1} step="any" class="mt-1 w-full" />
									</label>
									<label class="text-xs text-text-tertiary">
										Tick minimum
										<Input type="number" bind:value={model.tickMin} min={0} step="any" class="mt-1 w-full" />
									</label>
									<label class="text-xs text-text-tertiary">
										Tick maximum
										<Input type="number" bind:value={model.tickMax} min={0} step="any" class="mt-1 w-full" />
									</label>
								</div>
								<p class="mt-1.5 text-xs text-text-tertiary">Later matching ticks stay attached to this activation and never add another use cost.</p>
							</div>
						{/if}
					</div>
				</div>
			{:else if model.addType === 'tool'}
				<div class="grid md:grid-cols-2 gap-x-8 gap-y-4 items-start">
					<div class="space-y-4">
						<div>
							<label for="equipment-tool-search" class="block eyebrow mb-1.5">
								Harvesting Tool
							</label>
							<PickerInput id="equipment-tool-search" model={model.toolPicker} placeholder="">
								{#snippet result({ item })}{@render resultLine({ item })}{/snippet}
								{#snippet selection({ item })}{@render selectionLine(item)}{/snippet}
							</PickerInput>
							{#if model.toolPicker.selected?.isLimited}
								<div class="mt-1.5 flex items-center justify-end gap-2">
									<label for="equipment-tool-markup" class="text-xs text-text-tertiary">Tool markup %</label>
									<Input id="equipment-tool-markup" type="number" bind:value={model.markupPercent} min={100} max={10000} class="w-20" />
								</div>
							{/if}
						</div>
					</div>
				</div>
			{:else if model.addType === 'consumable'}
				<div class="grid md:grid-cols-2 gap-x-8 gap-y-4 items-start">
					<div>
						<label for="equipment-consumable-search" class="block eyebrow mb-1.5">
							Consumable
						</label>
						<PickerInput
							id="equipment-consumable-search"
							model={model.consumablePicker}
							placeholder="Search or type a custom name…"
							extraRow={showConsumableCustomRow ? consumableCustomRow : undefined}
						>
							{#snippet result({ item })}
								<span class="text-text truncate">{item.name}</span>
							{/snippet}
							{#snippet selection({ item })}
								<span class="text-text font-medium truncate flex-1">{item.name}</span>
								{#if !item.catalogId}
									<span class="text-xs text-text-tertiary shrink-0">Custom entry</span>
								{/if}
							{/snippet}
						</PickerInput>
					</div>
				</div>
			{/if}
		</div>

		<!-- Summary bar: live cost and actions, always on screen -->
		<div class="shrink-0 mt-5 pt-4 border-t border-border/50 flex items-center justify-between gap-4">
			<div class="min-w-0">
				{#if model.liveCostPreview !== null}
					<span class="eyebrow">Estimated cost per use</span>
					<span class="ml-2 text-lg font-semibold tabular-nums text-accent">{formatPec(model.liveCostPreview)} PEC</span>
					{#if model.liveAttackRateFactor > 1}
						<span class="ml-2 text-xs text-text-tertiary" data-testid="preview-attack-rate">
							{formatFactor(model.liveAttackRateFactor)} at the server's attack-rate limit
						</span>
					{/if}
				{/if}
			</div>
			<div class="flex items-center gap-2 shrink-0">
				<Button type="button" variant="ghost" onclick={() => (model.showAddModal = false)}>Cancel</Button>
				<Button type="button" disabled={saveDisabled} onclick={() => model.saveEquipment()}>
					{model.saving ? 'Saving…' : model.editingEquipmentId ? 'Save Changes' : 'Save'}
				</Button>
			</div>
		</div>
	</div>
</Modal>
