<script lang="ts">
	// The after-session review of what the screen read as the tool in hand
	// that is not in the library: tools of the game's items held without an
	// entry, and names that matched no tool. Each is settled once: added to
	// equipment (a tool, once the session has ended), marked as not a tool
	// the player tracks, or left, each one action. Marking a real tool of
	// the game's items asks once more in place, since it stops the tool
	// being costed. A settled one collapses to one quiet line with its undo.
	// Not shown when there is nothing.
	import { Button } from '$lib/components';
	import SessionReviewBlock from './SessionReviewBlock.svelte';
	import { canAdd, describeSettled, describeSighted, sightingName } from './toolSighting';
	import type { ToolSightingModel } from './toolSightingModel.svelte';

	let { model }: { model: ToolSightingModel } = $props();

	// The tool of the game's items whose dismissal awaits its confirming click.
	let confirming = $state<string | null>(null);
</script>

{#if model.sightings.length > 0}
	<SessionReviewBlock
		id="tool-sightings"
		heading="Detected tools"
		error={model.error}
		onDismissError={model.dismissError}
		notice={model.notice}
	>
		{#if model.offer}
			<div class="flex flex-wrap items-center gap-2 py-2 text-xs text-text-secondary">
				<Button size="sm" variant="secondary" disabled={model.busy !== null} onclick={() => void model.turnOn()}>
					Turn {model.offer.label} on
				</Button>
			</div>
		{/if}
		{#each model.sightings as sighting (sighting.id)}
			{@const name = sightingName(sighting)}
			{#if sighting.settled}
				<div
					class="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-2 text-sm"
					data-testid="tool-sighting"
					data-settled={sighting.settled.kind}
				>
					<span class="text-text-secondary">{name}</span>
					<span class="text-xs text-text-tertiary">{describeSettled(sighting.settled)}</span>
					<button
						type="button"
						class="linklet ml-auto disabled:opacity-40"
						aria-label="Undo for {name}"
						disabled={model.busy !== null}
						onclick={() => void model.undo(sighting)}
					>
						Undo
					</button>
				</div>
			{:else}
				{@const acting = model.busy === sighting.id}
				<div class="py-3 text-sm" data-testid="tool-sighting">
					<div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
						<span class="text-text">{name}</span>
						<span class="text-xs text-text-tertiary">{describeSighted(sighting.sighted)}</span>
					</div>
					{#if confirming === sighting.id}
						<div class="mt-2 flex flex-wrap items-center gap-2" role="group" aria-label="Confirm for {name}">
							<span class="text-xs text-text-secondary">It will no longer be costed when held. Not a tool I track?</span>
							<Button
								size="sm"
								variant="ghost"
								disabled={model.busy !== null}
								onclick={() => {
									confirming = null;
									void model.dismiss(sighting);
								}}
							>
								Not a tool I track
							</Button>
							<Button size="sm" variant="ghost" onclick={() => (confirming = null)}>Cancel</Button>
						</div>
					{:else}
						<div class="mt-2 flex flex-wrap items-center gap-2" role="group" aria-label="Settle {name}">
							{#if canAdd(sighting, model.ended)}
								<Button
									size="sm"
									variant="secondary"
									loading={acting}
									disabled={model.busy !== null}
									onclick={() => void model.add(sighting)}
								>
									Add to equipment
								</Button>
							{/if}
							<Button
								size="sm"
								variant="ghost"
								disabled={model.busy !== null}
								onclick={() => {
									if (sighting.sighted.kind === 'bundled') confirming = sighting.id;
									else void model.dismiss(sighting);
								}}
							>
								Not a tool I track
							</Button>
							<Button size="sm" variant="ghost" disabled={model.busy !== null} onclick={() => void model.leave(sighting)}>
								Leave
							</Button>
						</div>
					{/if}
				</div>
			{/if}
		{/each}
	</SessionReviewBlock>
{/if}
