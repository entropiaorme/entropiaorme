<script lang="ts">
	// The after-session review of which creatures the session's kills were
	// detected as. Each group is settled in one action for all its kills:
	// name the creature (a listed one, or a new name to keep), or put back
	// the declared mob. The least certain groups come first. Kills given the
	// creature being hunted (continuity) come last, each creature as one
	// quiet line with nothing asking to be done; the same settlement is a
	// click away. Shown for an ended session only (the model gives no groups
	// otherwise).
	import { Button, Divider, ErrorNotice, Input } from '$lib/components';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import {
		continuityLine,
		describeGrade,
		groupKey,
		isContinuity,
		isSuggestion,
		killCount,
		MAX_NAME_LENGTH,
		nameAction,
	} from './mobDetection';
	import type { DetectedCreature, MobDetectionGroup } from '$lib/api';
	import type { MobDetectionModel } from './mobDetectionModel.svelte';

	let { model }: { model: MobDetectionModel } = $props();

	// The continuity groups opened for correcting, by group key.
	let correcting = $state<Record<string, boolean>>({});
</script>

{#snippet settle(group: MobDetectionGroup, acting: boolean, offered: DetectedCreature | null)}
	<div class="mt-2 flex flex-wrap items-center gap-2">
		<Input
			class="min-w-[12rem] flex-1"
			value={model.draft(group)}
			maxlength={MAX_NAME_LENGTH}
			aria-label="Creature for {isContinuity(group.group.grade)
				? continuityLine(group.killCount)
				: group.exampleText === null
					? `${killCount(group.killCount)} pinned`
					: `${killCount(group.killCount)} read as ${group.exampleText}`}"
			disabled={model.busy !== null}
			oninput={(e: Event) => model.setDraft(group, (e.currentTarget as HTMLInputElement).value)}
			onkeydown={(e: KeyboardEvent) => {
				if (e.key === 'Enter') void model.name(group);
			}}
		/>
		<Button size="sm" variant="secondary" loading={acting} disabled={model.busy !== null} onclick={() => void model.name(group)}>
			{nameAction(group.group.grade, model.draft(group), group.group.creature.name)}
		</Button>
		<Button size="sm" variant="ghost" disabled={model.busy !== null} onclick={() => void model.restoreDeclared(group)}>
			Use declared mob
		</Button>
	</div>
	{#if offered}
		<p class="mt-2 flex flex-wrap items-center gap-2 text-xs text-text-secondary" role="status">
			Did you mean {offered.name}?
			<Button size="sm" variant="ghost" disabled={model.busy !== null} onclick={() => void model.acceptOffer(group)}>
				Use {offered.name}
			</Button>
		</p>
	{/if}
{/snippet}

{#if model.groups.length > 0}
	<Divider />
	<section aria-labelledby="mob-detections-heading" data-testid="mob-detections">
		<div class="mb-3 flex flex-wrap items-baseline gap-2">
			<h3 id="mob-detections-heading" class="eyebrow">Detected creatures</h3>
			<InDevelopmentMark id="creature-detection" align="left" />
		</div>

		<ErrorNotice message={model.error} onDismiss={model.dismissError} class="mb-3" />
		{#if model.notice}
			<p class="mb-3 text-xs text-text-secondary" role="status">{model.notice}</p>
		{/if}

		<div class="divide-y divide-border/40">
			{#each model.groups as group (groupKey(group.group))}
				{@const grade = group.group.grade}
				{@const acting = model.busy === groupKey(group.group)}
				{@const offered = model.offerFor(group)}
				{@const key = groupKey(group.group)}
				{#if isContinuity(grade)}
					<div class="py-2 text-sm" data-testid="mob-detection" data-grade={grade}>
						<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
							<span class="text-text-secondary">{group.group.creature.name}</span>
							<span class="text-xs text-text-tertiary"
								>{continuityLine(
									group.killCount,
									group.group.creature.name,
									group.currentName,
								)}</span
							>
							<button
								type="button"
								class="ml-auto text-xs text-text-tertiary hover:text-text"
								aria-expanded={correcting[key] === true}
								onclick={() => (correcting = { ...correcting, [key]: !correcting[key] })}
							>
								Correct
							</button>
						</div>
						{#if correcting[key]}
							<p class="mt-1 text-xs text-text-tertiary">
								Kills nothing on screen named took the creature the kills before them were.
								After a change of creature whose name was not read, the first few can be the
								one before.
							</p>
							{@render settle(group, acting, offered)}
						{/if}
					</div>
				{:else}
					<div class="py-3 text-sm" data-testid="mob-detection" data-grade={grade}>
						<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
							<span class="text-text">{group.group.creature.name}</span>
							<span class="text-xs text-text-tertiary">{describeGrade(grade)}</span>
							<span class="ml-auto text-xs tabular-nums text-text-tertiary">{killCount(group.killCount)}</span>
						</div>
						<p class="mt-1 text-xs text-text-tertiary">
							{#if group.exampleText === null}
								Pinned from what the screen read{#if group.currentName}; recorded as {group.currentName}{/if}.
							{:else}
								Read as “{group.exampleText}”{#if group.currentName}; recorded as {group.currentName}{/if}{#if isSuggestion(grade) && !group.reviewed}, your declared mob{/if}.
							{/if}
						</p>
						{@render settle(group, acting, offered)}
					</div>
				{/if}
			{/each}
		</div>
	</section>
{/if}
