<script lang="ts">
	// Equipment's Mobs tab: the creatures kept for the name reader beside the
	// bundled list. One entry adds or edits a creature, typed as its plate
	// prints it and checked as it is typed; the list below holds what is kept.
	import { onMount } from 'svelte';
	import { Button, ErrorNotice, Input } from '$lib/components';
	import {
		createCreatureEntries,
		plateText,
	} from '$lib/features/equipment/creatureEntries.svelte';
	import type { CreatureEntry } from '$lib/api';

	const model = createCreatureEntries();
	onMount(() => {
		void model.load();
		return () => model.destroy();
	});

	/** What the entry would come to, in one line under it. */
	const checkLine = $derived.by((): { text: string; tone: 'quiet' | 'ready' | 'warn' } | null => {
		const check = model.check;
		if (!model.text.trim() || !check) return null;
		switch (check.kind) {
			case 'ready':
				return {
					text: check.anyNumber
						? `${check.name}, at any number (shown as ${check.maturity})`
						: check.maturity
							? `${check.name}, ${check.maturity}`
							: check.name,
					tone: 'ready',
				};
			case 'known': {
				const known = check.maturity ? `${check.name} ${check.maturity}` : check.name;
				const recorded =
					check.recordedAs.name.toLowerCase() === known.toLowerCase()
						? ''
						: `, recorded as ${check.recordedAs.name}`;
				return { text: `Already known: ${known}${recorded}`, tone: 'quiet' };
			}
			case 'refused':
				return { text: check.reason, tone: 'warn' };
		}
	});

	function detail(entry: CreatureEntry): string {
		const parts: string[] = [];
		if (entry.anyNumber) parts.push('any number');
		if (entry.species && entry.species !== entry.name) parts.push(entry.species);
		if (entry.source === 'review') parts.push('confirmed in a review');
		return parts.join(' · ');
	}
</script>

<div class="max-w-3xl space-y-6">
	<header>
		<h2 class="text-sm font-medium text-text">Creatures</h2>
		<p class="mt-1 text-sm leading-6 text-text-secondary">
			Creatures the bundled list lacks, so creature detection can name them. Type one as its
			plate shows it; a name ending in a number covers every number.
		</p>
	</header>

	<ErrorNotice message={model.error} onDismiss={() => model.dismissError()} />

	<form
		class="space-y-2"
		onsubmit={(event) => {
			event.preventDefault();
			void model.save();
		}}
	>
		<div class="flex flex-wrap items-center gap-3">
			<Input
				class="flex-1 min-w-64"
				placeholder="As the plate shows it, e.g. Survival Zombie 04"
				aria-label="Creature as its plate shows it"
				bind:value={() => model.text, (value) => (model.text = String(value ?? ''))}
			/>
			<Input
				class="w-44"
				placeholder="Species (optional)"
				aria-label="Species"
				bind:value={() => model.species, (value) => (model.species = String(value ?? ''))}
			/>
			<Button type="submit" size="sm" disabled={!model.ready} loading={model.busy}>
				{model.editing ? 'Save' : 'Add'}
			</Button>
			{#if model.editing}
				<Button type="button" size="sm" variant="ghost" onclick={() => model.cancel()}>
					Cancel
				</Button>
			{/if}
		</div>
		{#if checkLine}
			<p
				class="text-xs {checkLine.tone === 'warn'
					? 'text-warning'
					: checkLine.tone === 'ready'
						? 'text-text-secondary'
						: 'text-text-tertiary'}"
				role="status"
			>
				{checkLine.text}
			</p>
		{/if}
	</form>

	{#if model.loaded && model.entries.length === 0}
		<p class="text-sm text-text-tertiary">No creatures kept yet.</p>
	{:else if model.entries.length > 0}
		<ul class="divide-y divide-border/40 border-t border-border/40">
			{#each model.entries as entry (entry.name)}
				{@const note = detail(entry)}
				<li
					class="flex items-center gap-4 py-2.5 {model.editing === entry.name
						? 'opacity-50'
						: ''}"
				>
					<div class="min-w-0 flex-1">
						<p class="truncate text-sm text-text">{plateText(entry)}</p>
						{#if note}
							<p class="text-xs text-text-tertiary">{note}</p>
						{/if}
					</div>
					<Button
						size="sm"
						variant="ghost"
						disabled={model.busy}
						onclick={() => model.edit(entry)}
					>
						Edit
					</Button>
					<Button
						size="sm"
						variant="ghost"
						disabled={model.busy}
						onclick={() => void model.remove(entry)}
					>
						Remove
					</Button>
				</li>
			{/each}
		</ul>
	{/if}
</div>
