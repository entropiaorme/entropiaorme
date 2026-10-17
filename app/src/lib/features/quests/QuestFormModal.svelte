<script lang="ts">
	import { Button, Input, Modal, SegmentedControl, Select } from '$lib/components';
	import { PLANETS, type CooldownUnit, type QuestsModel } from './questsModel.svelte';

	let {
		model,
		compact = false
	}: {
		model: QuestsModel;
		/** The dashboard board's custom-quest form: the planet comes from the
		 * board, and the fields the board does not use (family, target mobs)
		 * stay out of the way. */
		compact?: boolean;
	} = $props();

	const title = $derived(
		compact
			? model.editingQuest
				? `Edit ${model.editingQuest.name}`
				: `New quest on ${model.questForm.planet}`
			: model.editingQuest
				? 'Edit Quest'
				: 'New Quest'
	);

	// Visible auto-attach: while creating and until the family select is
	// touched, a name reading "Family: Variant" keeps the select on its
	// matching family, so membership is suggested in the open form rather
	// than applied invisibly on save.
	$effect(() => {
		if (!model.showQuestModal || model.editingQuest || model.familySelectTouched) return;
		model.questForm.family_id = model.familyMatchForName(model.questForm.name)?.id ?? null;
	});

	// Manual hand-in always captures the user-confirmed raw clump. Keep the
	// reward policy visible but fixed so the form cannot suggest an
	// incompatible separate-reward rule.
	$effect(() => {
		if (model.questForm.completion_trigger !== 'manual_hand_in') return;
		model.questForm.reward_policy = 'completion_clump';
	});

	const selectedFamily = $derived(
		model.families.find((f) => f.id === model.questForm.family_id) ?? null,
	);
</script>

<Modal bind:open={model.showQuestModal} {title} class="max-w-lg">
	{#snippet children()}
		<form class="space-y-3" onsubmit={(e) => { e.preventDefault(); model.saveQuest(); }}>
			<div class="grid grid-cols-2 gap-3">
				<div class="col-span-2">
					<label class="block text-xs text-text-secondary mb-1" for="q-name">Name</label>
					<Input id="q-name" type="text" required bind:value={model.questForm.name}
						placeholder="e.g., Atlas Haven Imperium Ranger Hunt!" />
				</div>
				{#if !compact}
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-planet">Planet</label>
					<Select id="q-planet" bind:value={model.questForm.planet}>
						{#each PLANETS as planet}
							<option value={planet}>{planet}</option>
						{/each}
					</Select>
				</div>
				{/if}
				<div class={compact ? 'col-span-2' : ''}>
					<label class="block text-xs text-text-secondary mb-1" for="q-cat">Category</label>
					<Input id="q-cat" type="text" bind:value={model.questForm.category}
						placeholder="e.g., A.R.C. Faction" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-trigger">Completion</label>
					<Select id="q-trigger" bind:value={model.questForm.completion_trigger}>
						<option value="mission_log">Mission log</option>
						<option value="signal_item">Signal item</option>
						<option value="manual_hand_in">Manual hand-in</option>
					</Select>
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-reward-policy">Reward</label>
					<Select
						id="q-reward-policy"
						bind:value={model.questForm.reward_policy}
						disabled={model.questForm.completion_trigger === 'manual_hand_in'}
					>
						<option value="none">No separate reward</option>
						<option value="fixed_pes">Fixed PES</option>
						<option value="named_items">Specific items</option>
						<option value="completion_clump">Completion loot clump</option>
					</Select>
				</div>
				{#if model.questForm.reward_policy === 'fixed_pes'}
					<div>
						<label class="block text-xs text-text-secondary mb-1" for="q-reward">Amount (PES)</label>
						<Input id="q-reward" type="number" step="0.01" min="0" required bind:value={model.questForm.reward_ped} />
					</div>
				{/if}
				{#if model.questForm.reward_policy === 'named_items'}
					<div class="col-span-2">
						<div class="block text-xs text-text-secondary mb-1">Reward items</div>
						{#if model.questForm.reward_item_names.length > 0}
							<div class="flex flex-wrap gap-1 mb-1.5">
								{#each model.questForm.reward_item_names as item}
									<span class="text-xs px-2 py-0.5 rounded-full bg-accent/10 text-accent border border-accent/20 flex items-center gap-1">
										{item}
										<button type="button" class="hover:text-text cursor-pointer" aria-label="Remove {item}" onclick={() => model.removeRewardItem(item)}>&times;</button>
									</span>
								{/each}
							</div>
						{/if}
						<div class="flex gap-2">
							<Input type="text" bind:value={model.rewardItemInput} placeholder="e.g., Hyperion Daily Voucher"
								class="flex-1"
								onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); model.addRewardItem(); } }} />
							<Button type="button" size="sm" variant="secondary" onclick={() => model.addRewardItem()}>{#snippet children()}Add{/snippet}</Button>
						</div>
						<p class="text-[11px] text-text-secondary/70 mt-1">Every matching line is separated from ordinary loot with its observed quantity and TT. Missing expected items remain unresolved.</p>
					</div>
				{:else if model.questForm.reward_policy === 'completion_clump'}
					<p class="col-span-2 text-[11px] text-text-secondary/70">
						{#if model.questForm.completion_trigger === 'manual_hand_in'}
							Selecting this quest in Activities starts its run. Use Hand in quest there to review the latest clump or wait for the next one, then confirm the exact reward.
						{:else}
							Captures every loot line accompanying an isolated NPC hand-in. A tick containing combat evidence is left unresolved instead of consuming ordinary loot.
						{/if}
					</p>
				{/if}
				<div class="col-span-2">
					<label class="block text-xs text-text-secondary mb-1" for="q-rdesc">Reward Note</label>
					<Input id="q-rdesc" type="text" bind:value={model.questForm.reward_description}
						placeholder="e.g., 3x A.R.C. Faction Badge" />
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-cd">Cooldown</label>
					<div class="flex items-stretch gap-2">
						<Input id="q-cd" type="number" step="1" min="0" bind:value={model.cooldownInput}
							class="flex-1 min-w-0"
							placeholder={model.cooldownUnit === 'hours' ? '21' : '7'} />
						<SegmentedControl
							size="md"
							options={[
								{ id: 'hours', label: 'Hours' },
								{ id: 'days', label: 'Days' }
							]}
							active={model.cooldownUnit}
							onchange={(id) => {
								if (id === 'hours' && model.cooldownUnit === 'days' && model.cooldownInput != null) {
									model.cooldownInput = model.cooldownInput * 24;
								} else if (id === 'days' && model.cooldownUnit === 'hours' && model.cooldownInput != null) {
									model.cooldownInput = Math.round((model.cooldownInput / 24) * 10) / 10;
								}
								model.cooldownUnit = id as CooldownUnit;
							}}
						/>
					</div>
				</div>
				<div>
					<label class="block text-xs text-text-secondary mb-1" for="q-wp">Waypoint</label>
					<Input id="q-wp" type="text" bind:value={model.questForm.waypoint}
						class="font-mono"
						placeholder="/wp [Planet, Lon, Lat, Alt]" />
				</div>
				{#if (model.cooldownInput ?? 0) > 0}
					<!-- The anchor decides WHEN the quest's own timer starts;
						 the observed daily behaviour starts it at collection,
						 not completion, so getting this wrong misreports
						 availability for a whole cycle. -->
					<div class="col-span-2">
						<div class="block text-xs text-text-secondary mb-1">Cooldown starts</div>
						<SegmentedControl
							size="md"
							options={[
								{ id: 'completion', label: 'On completion' },
								{ id: 'pickup', label: 'On pickup' }
							]}
							active={model.questForm.cooldown_anchor}
							onchange={(id) => (model.questForm.cooldown_anchor = id as 'pickup' | 'completion')}
						/>
						<p class="text-[11px] text-text-secondary/70 mt-1">
							{#if model.questForm.cooldown_anchor === 'pickup'}
								Timer runs from when the mission is collected; abandoning or completing it does not restart the wait.
							{:else}
								Timer runs from when the quest completes.
							{/if}
						</p>
					</div>
				{/if}
			</div>

			<!-- Family: variants of one repeatable slot cool as a unit. -->
			{#if !compact}
			<div>
				<label class="block text-xs text-text-secondary mb-1" for="q-family">Family</label>
				<Select id="q-family" bind:value={model.questForm.family_id}
					onchange={() => (model.familySelectTouched = true)}>
					<option value={null}>None (standalone)</option>
					{#each model.families as family (family.id)}
						<option value={family.id}>{family.name}</option>
					{/each}
				</Select>
				<p class="text-[11px] text-text-secondary/70 mt-1">
					{#if selectedFamily}
						Availability follows the family: completing or collecting any variant gates every sibling{#if selectedFamily.cooldownDurationHours}&nbsp;for {selectedFamily.cooldownDurationHours}h from {selectedFamily.cooldownAnchor === 'pickup' ? 'pickup' : 'completion'}{/if}.
					{:else}
						Optional. Rotating variants of one repeatable slot ("Family: Variant" names match automatically) share the family's cooldown.
					{/if}
				</p>
			</div>
			{/if}

			{#if model.questForm.completion_trigger === 'signal_item'}
			<div>
				<label class="block text-xs text-text-secondary mb-1" for="q-signal">Signal Loot (auto-complete)</label>
				<Input id="q-signal" type="text" bind:value={model.questForm.signal_loot_item}
					required
					placeholder="e.g., Hyperion Daily Voucher" />
				<p class="text-[11px] text-text-secondary/70 mt-1">
					{#if model.questForm.signal_loot_item.trim()}
						Completes when this item drops outside a mission completion; focusing it in the overlay starts a run.
					{:else}
						Name the item that proves completion. It may independently be selected as a reward item above.
					{/if}
				</p>
			</div>
			{/if}

			<!-- Target Mobs -->
			{#if !compact}
			<div>
				<div class="block text-xs text-text-secondary mb-1">Target Mobs</div>
				{#if model.questForm.mobs.length > 0}
					<div class="flex flex-wrap gap-1 mb-1.5">
						{#each model.questForm.mobs as mob}
							<span class="text-xs px-2 py-0.5 rounded-full bg-accent/10 text-accent border border-accent/20 flex items-center gap-1">
								{mob}
								<button type="button" class="hover:text-text cursor-pointer" aria-label="Remove {mob}" onclick={() => model.removeMob(mob)}>&times;</button>
							</span>
						{/each}
					</div>
				{/if}
				<div class="flex gap-2">
					<Input type="text" bind:value={model.mobInput} placeholder="Type mob name, press Enter"
						class="flex-1"
						onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); model.addMob(); } }} />
					<Button type="button" size="sm" variant="secondary" onclick={() => model.addMob()}>{#snippet children()}Add{/snippet}</Button>
				</div>
			</div>
			{/if}


			<div class="flex justify-end gap-2 pt-1">
				<Button type="button" variant="ghost" onclick={() => (model.showQuestModal = false)}>{#snippet children()}Cancel{/snippet}</Button>
				<Button type="submit">{#snippet children()}{model.editingQuest ? 'Save' : 'Create'}{/snippet}</Button>
			</div>
		</form>
	{/snippet}
</Modal>
