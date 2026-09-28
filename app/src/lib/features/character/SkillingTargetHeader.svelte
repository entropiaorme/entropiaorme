<script lang="ts">
	import Input from '$lib/components/Input.svelte';
	import type { CharacterModel } from './characterModel.svelte';
	import CodexProfessionPicker from './CodexProfessionPicker.svelte';
	import SkillingBar from './SkillingBar.svelte';
	import { formatTargetValue } from './skillingFormat';

	// The hub's spine: the target is the page's title (the recommender's
	// picker, presented as a headline), with where you stand and the goal
	// every goal-bound facet answers beside it.
	let { model }: { model: CharacterModel } = $props();
	const hub = $derived(model.skilling);

	const levels = $derived(Object.fromEntries(model.professions.map((prof) => [prof.name, prof.level])));
	const isHp = $derived(hub.target.kind === 'hp');
	const current = $derived(hub.current);
	// Progress through the current whole level (profession levels carry a
	// fraction; HP reads as whole points and shows none).
	const fraction = $derived(current !== null && !isHp ? current - Math.floor(current) : null);
	const goalBelow = $derived(
		current !== null && hub.goal !== null && hub.goal <= current,
	);
</script>

<section aria-label="Skilling target">
	<span class="eyebrow block">Skill up</span>
	<div class="mt-1 flex flex-wrap items-end justify-between gap-x-10 gap-y-4">
		<div class="min-w-0 flex-1">
			<CodexProfessionPicker
				variant="headline"
				align="left"
				includeNone={false}
				professions={model.professions.map((prof) => prof.name)}
				{levels}
				hp={model.stats.hp}
				target={hub.target}
				onchange={hub.setTarget}
				class="min-w-0"
			/>

			{#if current !== null}
				<div class="mt-2.5 flex max-w-md items-center gap-3 text-xs tabular-nums">
					<span class="text-sm text-text-secondary">
						{#if isHp}
							<span class="font-medium text-text">{formatTargetValue(hub.target, current)}</span> HP now
						{:else}
							Level <span class="font-medium text-text">{formatTargetValue(hub.target, current)}</span>
						{/if}
					</span>
					{#if fraction !== null}
						<SkillingBar {fraction} class="flex-1" />
						<span class="whitespace-nowrap text-text-tertiary">
							{Math.floor(fraction * 100)}% to {Math.floor(current) + 1}
						</span>
					{/if}
				</div>
			{:else if hub.members.length > 0}
				<p class="mt-2.5 flex flex-wrap items-baseline gap-x-4 gap-y-1 text-sm text-text-secondary">
					{#each hub.members as member (member.name)}
						<button
							type="button"
							class="cursor-pointer rounded transition-colors hover:text-text focus:outline-none focus-visible:text-text"
							title="Skill up {member.name} on its own"
							onclick={() => hub.setTarget({ kind: 'profession', name: member.name })}
						>
							{member.name}
							<span class="ml-1 tabular-nums text-text-tertiary">{member.level.toFixed(2)}</span>
						</button>
					{/each}
				</p>
			{/if}
		</div>

		{#if current !== null}
			<label class="flex flex-col items-end gap-1.5">
				<span class="eyebrow">{isHp ? 'Goal HP' : 'Goal level'}</span>
				<Input
					class="w-32"
					align="right"
					type="number"
					min="1"
					step={isHp ? '1' : '0.01'}
					inputmode="decimal"
					aria-label={isHp ? 'Goal HP' : 'Goal level'}
					value={hub.goalInput}
					oninput={(event) => hub.setGoal(event.currentTarget.value)}
					onblur={hub.commitGoal}
					onkeydown={(event) => {
						if (event.key === 'Enter') hub.commitGoal();
					}}
				/>
			</label>
		{/if}
	</div>

	{#if goalBelow}
		<p class="mt-3 text-xs text-warning">
			Set a goal above your current {isHp ? 'HP' : 'level'} to see what reaching it takes.
		</p>
	{/if}
</section>
