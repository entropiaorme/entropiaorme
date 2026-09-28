<script lang="ts">
	import Input from '$lib/components/Input.svelte';
	import type { CharacterModel } from './characterModel.svelte';
	import CodexProfessionPicker from './CodexProfessionPicker.svelte';
	import SkillingBar from './SkillingBar.svelte';
	import { formatTargetValue } from './skillingFormat';

	// The hub's spine: the target is the page's title (the recommender's
	// picker, presented as a headline) with the goal every section answers
	// right beside it, and where you stand beneath.
	let { model }: { model: CharacterModel } = $props();
	const hub = $derived(model.skilling);

	const levels = $derived(Object.fromEntries(model.professions.map((prof) => [prof.name, prof.level])));
	const isHp = $derived(hub.target.kind === 'hp');
	const isFamily = $derived(hub.target.kind === 'family');
	const current = $derived(hub.current);
	// Progress through the current whole level (profession levels carry a
	// fraction; HP reads as whole points and shows none).
	const fraction = $derived(
		current !== null && hub.target.kind === 'profession' ? current - Math.floor(current) : null,
	);
	const goalBelow = $derived(current !== null && hub.goal !== null && hub.goal <= current);
</script>

<section aria-label="Skilling target">
	<div class="flex min-w-0 flex-wrap items-center gap-x-6 gap-y-3">
		<CodexProfessionPicker
			variant="headline"
			align="left"
			includeNone={false}
			professions={model.professions.map((prof) => prof.name)}
			{levels}
			hp={model.stats.hp}
			target={hub.target}
			onchange={hub.setTarget}
			class="-ml-1.5 max-w-full shrink-0"
		/>

		{#if current !== null}
			<label class="flex items-center gap-2.5">
				<span class="eyebrow">Goal</span>
				<Input
					class="w-24"
					align="right"
					type="number"
					min="1"
					step={isHp ? '1' : '0.01'}
					inputmode="decimal"
					aria-label={isHp ? 'Goal HP' : isFamily ? 'Goal combined level' : 'Goal level'}
					aria-invalid={goalBelow}
					value={hub.goalInput}
					oninput={(event) => hub.setGoal(event.currentTarget.value)}
					onblur={hub.commitGoal}
					onkeydown={(event) => {
						if (event.key === 'Enter') hub.commitGoal();
					}}
				/>
				{#if goalBelow}
					<span class="text-xs text-warning">Below current</span>
				{/if}
			</label>
		{/if}
	</div>

	{#if current !== null}
		<div class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs tabular-nums">
			<span class="whitespace-nowrap text-sm text-text-secondary">
				{#if isHp}
					<span class="font-medium text-text">{formatTargetValue(hub.target, current)}</span> HP
				{:else}
					{isFamily ? 'Combined' : 'Level'}
					<span class="font-medium text-text">{formatTargetValue(hub.target, current)}</span>
				{/if}
			</span>
			{#if fraction !== null}
				<SkillingBar {fraction} class="w-80 max-w-full" />
				<span class="whitespace-nowrap text-text-tertiary">{Math.floor(fraction * 100)}%</span>
			{/if}
			{#each hub.members as member (member.name)}
				<button
					type="button"
					class="cursor-pointer rounded text-sm text-text-secondary transition-colors hover:text-text focus:outline-none focus-visible:text-text"
					title="Skill up {member.name} on its own"
					onclick={() => hub.setTarget({ kind: 'profession', name: member.name })}
				>
					{member.name}
					<span class="ml-1 text-text-tertiary">{member.level.toFixed(2)}</span>
				</button>
			{/each}
		</div>
	{/if}
</section>
