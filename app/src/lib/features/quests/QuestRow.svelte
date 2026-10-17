<script lang="ts">
	import { Button, Menu } from '$lib/components';
	import type { Quest } from '$lib/types';
	import {
		formatCooldownHours,
		getCooldownGate,
		getCooldownRemaining,
		getCooldownStatus,
	} from './cooldown';
	import QuestActions from './QuestActions.svelte';
	import type { QuestsModel } from './questsModel.svelte';

	let { quest, now, model }: { quest: Quest; now: number; model: QuestsModel } = $props();

	const status = $derived(getCooldownStatus(quest, now));
	const remaining = $derived(getCooldownRemaining(quest, now));
	const familyGated = $derived(getCooldownGate(quest, now) === 'family');
</script>

<div class="bg-surface-raised/50 rounded-lg border border-border/50 hover:bg-surface-raised/70 transition-colors px-4 py-2.5">
	<!-- Top row -->
	<div class="flex items-center gap-2.5">
		<!-- Status dot -->
		<div class="shrink-0">
			{#if quest.startedAt}
				<div class="w-2.5 h-2.5 rounded-full bg-accent animate-pulse"></div>
			{:else if status === 'ready' || status === 'no_cooldown'}
				<div class="w-2.5 h-2.5 rounded-full bg-success"></div>
			{:else}
				<div class="w-2.5 h-2.5 rounded-full bg-text-tertiary"></div>
			{/if}
		</div>

		<!-- Quest info -->
		<div class="flex-1 min-w-0">
			<!-- Title line -->
			<div class="flex items-center gap-2 flex-wrap">
				<span class="text-sm font-medium text-text truncate">{quest.name}</span>
				{#if quest.rewardDescription}
					<span class="text-xs text-text-tertiary truncate hidden sm:inline">{quest.rewardDescription}</span>
				{/if}
				{#if quest.waypoint}
					<button
						class="text-[10px] text-accent hover:text-accent/80 transition-colors cursor-pointer shrink-0"
						onclick={() => model.copyWaypoint(quest.id, quest.waypoint!)}
					>{model.copiedWp === quest.id ? 'Copied!' : 'WP'}</button>
				{/if}
				{#if quest.familyName}
					<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-surface text-text-tertiary border border-border/50">{quest.familyName}</span>
				{/if}
				{#each quest.targetMobs as mob}
					<span class="text-[10px] px-1.5 py-0.5 rounded-full bg-accent/10 text-accent/70 border border-accent/20">{mob}</span>
				{/each}
			</div>
			<!-- Stats line -->
			<div class="flex items-center gap-1.5 mt-0.5 text-xs text-text-tertiary">
				{#if quest.rewardIsSkill && quest.reward}
					<span class="font-mono text-accent">
						{quest.reward.toFixed(2)}
					</span>
					<span>PES</span>
				{/if}
				{#if quest.rewardIsSkill && quest.reward && (quest.cooldownDurationHours || quest.familyCooldownDurationHours)}
					<span class="text-text-tertiary/50">|</span>
				{/if}
				{#if quest.cooldownDurationHours}
					<span>CD: {formatCooldownHours(quest.cooldownDurationHours)}{quest.cooldownAnchor === 'pickup' ? ' from pickup' : ''}</span>
				{/if}
				{#if quest.cooldownDurationHours && quest.familyCooldownDurationHours}
					<span class="text-text-tertiary/50">|</span>
				{/if}
				{#if quest.familyCooldownDurationHours}
					<span>Family CD: {formatCooldownHours(quest.familyCooldownDurationHours)}{quest.familyCooldownAnchor === 'pickup' ? ' from pickup' : ''}</span>
				{/if}
			</div>
		</div>

		<!-- Action area -->
		<div class="shrink-0 flex items-center gap-1.5">
			<QuestActions
				{quest}
				{status}
				{remaining}
				{familyGated}
				remainingDetail
				pendingCancelChoice={model.pendingCancelChoiceQuestId === quest.id}
				onStart={() => model.handleStart(quest.id)}
				onComplete={() => model.handleComplete(quest.id)}
				onCancel={(undoReward) => model.handleCancel(quest.id, undoReward)}
				onToggleCancelChoice={() => model.toggleCancelChoice(quest.id)}
			/>

			<!-- Three-dot menu: a curated quest is the app's to maintain. -->
			{#if !quest.curated}
			<Menu
				ariaLabel="Quest actions"
				items={[{ label: 'Edit', onSelect: () => model.openEditQuest(quest) }]}
			>
				{#if model.deleteConfirmId === quest.id}
					<div class="flex gap-1 px-2 py-1">
						<Button class="flex-1" size="sm" variant="danger" onclick={() => model.handleDeleteQuest(quest.id)}>
							{#snippet children()}Confirm{/snippet}
						</Button>
						<Button class="flex-1" size="sm" variant="ghost" onclick={() => (model.deleteConfirmId = null)}>
							{#snippet children()}Cancel{/snippet}
						</Button>
					</div>
				{:else}
					<button
						role="menuitem"
						tabindex="-1"
						class="w-full px-3 py-1.5 text-xs text-left text-text-secondary hover:bg-surface-hover hover:text-negative cursor-pointer"
						onclick={() => (model.deleteConfirmId = quest.id)}
					>Delete</button>
				{/if}
			</Menu>
			{/if}
		</div>
	</div>

	<!-- Notes (if present) -->
	{#if quest.notes}
		<div class="ml-[1.3rem] pl-3 mt-1.5 border-l-2 border-border/50">
			<p class="text-xs text-text-tertiary whitespace-pre-wrap">{quest.notes}</p>
		</div>
	{/if}
</div>
