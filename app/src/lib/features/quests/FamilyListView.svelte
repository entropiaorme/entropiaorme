<script lang="ts">
	import { Button, Menu } from '$lib/components';
	import type { QuestFamily } from '$lib/types';
	import {
		formatCooldownHours,
		getFamilyCooldownRemaining,
		getFamilyCooldownStatus,
	} from './cooldown';
	import type { FamilyModel } from './familyModel.svelte';
	import type { QuestsModel } from './questsModel.svelte';

	let {
		model,
		questsModel,
		now,
	}: { model: FamilyModel; questsModel: QuestsModel; now: number } = $props();

	function memberNames(family: QuestFamily): string[] {
		return questsModel.quests.filter((q) => q.familyId === family.id).map((q) => q.name);
	}
</script>

<div>
{#if questsModel.families.length === 0}
	<div class="text-center py-8 text-sm text-text-tertiary space-y-1">
		<p>No families yet.</p>
		<p>
			A family groups the rotating variants of one repeatable slot (e.g. a daily whose giver hands
			out a different variant each day) so they share one cooldown.
		</p>
	</div>
{:else}
	<div class="space-y-1.5">
		{#each questsModel.families as family (family.id)}
			{@const status = getFamilyCooldownStatus(family, now)}
			{@const remaining = getFamilyCooldownRemaining(family, now)}
			{@const members = memberNames(family)}
			<div class="bg-surface-raised/50 rounded-lg border border-border/50 hover:bg-surface-raised/70 transition-colors px-4 py-2.5">
				<div class="flex items-center gap-2.5">
					<!-- Status dot -->
					<div class="shrink-0">
						{#if status === 'cooling'}
							<div class="w-2.5 h-2.5 rounded-full bg-text-tertiary"></div>
						{:else if status === 'ready'}
							<div class="w-2.5 h-2.5 rounded-full bg-success"></div>
						{:else}
							<div class="w-2.5 h-2.5 rounded-full bg-border"></div>
						{/if}
					</div>

					<!-- Family info -->
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2 flex-wrap">
							<span class="text-sm font-medium text-text truncate">{family.name}</span>
							<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-surface text-text-tertiary border border-border/50">{family.planet}</span>
						</div>
						<div class="flex items-center gap-1.5 mt-0.5 text-xs text-text-tertiary">
							<span>{family.memberCount} {family.memberCount === 1 ? 'variant' : 'variants'}</span>
							{#if family.cooldownDurationHours}
								<span class="text-text-tertiary/50">|</span>
								<span>CD: {formatCooldownHours(family.cooldownDurationHours)} from {family.cooldownAnchor === 'pickup' ? 'pickup' : 'completion'}</span>
							{:else}
								<span class="text-text-tertiary/50">|</span>
								<span>ungated</span>
							{/if}
						</div>
					</div>

					<!-- Availability -->
					<div class="shrink-0 flex items-center gap-1.5">
						{#if status === 'cooling' && remaining}
							<div class="text-right">
								<div class="text-xs text-warning tabular-nums font-mono">{remaining}</div>
								<div class="text-[10px] text-text-tertiary">remaining</div>
							</div>
						{:else if status === 'ready'}
							<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-success/10 text-success border border-success/20">ready</span>
						{/if}

						<!-- Three-dot menu -->
						<Menu
							ariaLabel="Family actions"
							items={[{ label: 'Edit', onSelect: () => model.openEditFamily(family) }]}
						>
							{#if questsModel.deleteConfirmId === `family-${family.id}`}
								<div class="flex gap-1 px-2 py-1">
									<Button class="flex-1" size="sm" variant="danger" onclick={() => model.handleDeleteFamily(family.id)}>
										{#snippet children()}Confirm{/snippet}
									</Button>
									<Button class="flex-1" size="sm" variant="ghost" onclick={() => (questsModel.deleteConfirmId = null)}>
										{#snippet children()}Cancel{/snippet}
									</Button>
								</div>
							{:else}
								<button
									role="menuitem"
									tabindex="-1"
									class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-negative cursor-pointer"
									onclick={() => (questsModel.deleteConfirmId = `family-${family.id}`)}
								>Delete</button>
							{/if}
						</Menu>
					</div>
				</div>

				<!-- Members -->
				{#if members.length > 0}
					<div class="ml-[1.3rem] pl-3 mt-1.5 border-l-2 border-border/50 flex flex-wrap gap-1">
						{#each members as name}
							<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-accent/10 text-accent/70 border border-accent/20">{name}</span>
						{/each}
					</div>
				{/if}
			</div>
		{/each}
	</div>
{/if}
</div>
