<script lang="ts">
	// The after-session review of which mobs the session's kills were. One
	// line for each mob, with how many kills it was given; how sure the
	// screen was of each kill is a quiet note, never a line of its own. A
	// line asks for nothing unless its mob is on no list (its spelling is the
	// screen's, to keep or correct); any other is a click from a correction.
	// The kills the screen could not name are the last line, named in one
	// action. Shown for an ended session only (the model gives no lines
	// otherwise).
	import { Button, Input } from '$lib/components';
	import SessionReviewBlock from './SessionReviewBlock.svelte';
	import {
		isUnnamed,
		killCount,
		lineName,
		lineNote,
		MAX_NAME_LENGTH,
		mobKey,
		nameAction,
		needsAnswer,
	} from './mobDetection';
	import type { ReviewedMob } from '$lib/api';
	import type { MobDetectionModel } from './mobDetectionModel.svelte';

	let { model }: { model: MobDetectionModel } = $props();

	// The lines opened for correcting, by key.
	let correcting = $state<Record<string, boolean>>({});
</script>

{#if model.mobs.length > 0}
	<SessionReviewBlock
		id="mob-detections"
		heading="Mobs"
		error={model.error}
		onDismissError={model.dismissError}
		notice={model.notice}
	>
		{#each model.mobs as mob (mobKey(mob.mob))}
			{@const key = mobKey(mob.mob)}
			{@const unnamed = isUnnamed(mob)}
			{@const asks = needsAnswer(mob)}
			{@const open = asks || correcting[key] === true}
			{@const note = lineNote(mob)}
			{@const offered = model.offerFor(mob)}
			<div class="py-2.5 text-sm" data-testid="mob-detection" data-line={mob.mob.kind}>
				<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
					<span class={unnamed ? 'text-text-secondary' : 'text-text'}>{lineName(mob)}</span>
					{#if note}
						<span class="text-xs text-text-tertiary">{note}</span>
					{/if}
					<span class="ml-auto text-xs tabular-nums text-text-tertiary"
						>{killCount(mob.killCount)}</span
					>
					{#if !asks}
						<button
							type="button"
							class="text-xs text-text-tertiary hover:text-text"
							aria-expanded={open}
							onclick={() => (correcting = { ...correcting, [key]: !correcting[key] })}
						>
							{unnamed ? 'Name' : 'Correct'}
						</button>
					{/if}
				</div>
				{#if open}
					{#if asks && mob.exampleText}
						<p class="mt-1 text-xs text-text-tertiary">Read as “{mob.exampleText}”.</p>
					{/if}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<Input
							class="min-w-[12rem] flex-1"
							value={model.draft(mob)}
							maxlength={MAX_NAME_LENGTH}
							placeholder={unnamed ? 'The mob, as the game shows it' : undefined}
							aria-label="Mob for {killCount(mob.killCount)} of {lineName(mob)}"
							disabled={model.busy !== null}
							oninput={(e: Event) =>
								model.setDraft(mob, (e.currentTarget as HTMLInputElement).value)}
							onkeydown={(e: KeyboardEvent) => {
								if (e.key === 'Enter') void model.name(mob);
							}}
						/>
						<Button
							size="sm"
							variant="secondary"
							loading={model.busy === key}
							disabled={model.busy !== null}
							onclick={() => void model.name(mob)}
						>
							{nameAction(mob, model.draft(mob))}
						</Button>
						{#if !unnamed || mob.currentName}
							<Button
								size="sm"
								variant="ghost"
								disabled={model.busy !== null}
								onclick={() => void model.restoreDeclared(mob)}
							>
								{unnamed ? 'Put back' : 'Use declared mob'}
							</Button>
						{/if}
					</div>
					{#if offered}
						<p
							class="mt-2 flex flex-wrap items-center gap-2 text-xs text-text-secondary"
							role="status"
						>
							Did you mean {offered.name}?
							<Button
								size="sm"
								variant="ghost"
								disabled={model.busy !== null}
								onclick={() => void model.acceptOffer(mob)}
							>
								Use {offered.name}
							</Button>
						</p>
					{/if}
				{/if}
			</div>
		{/each}
	</SessionReviewBlock>
{/if}
