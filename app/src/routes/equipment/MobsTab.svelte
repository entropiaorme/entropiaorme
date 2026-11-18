<script lang="ts">
	// Equipment's Mobs tab: the mobs kept for the name reader beside the
	// bundled list, and what mobs are shown as. One entry adds or edits a
	// mob, typed as its plate prints it and checked as it is typed; the list
	// below holds what is kept. Under it, the labels: a mob shown under
	// another name than the game's, which stays the same mob.
	import { onMount } from 'svelte';
	import { Button, ErrorNotice, Input } from '$lib/components';
	import {
		createCreatureEntries,
		plateText,
	} from '$lib/features/equipment/creatureEntries.svelte';
	import { createMobLabels, labelKey } from '$lib/features/equipment/mobLabels.svelte';
	import type { CreatureEntry } from '$lib/api';

	const model = createCreatureEntries();
	const labels = createMobLabels();
	onMount(() => {
		void model.load();
		void labels.load();
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
		<h2 class="text-sm font-medium text-text">Mobs</h2>
		<p class="mt-1 text-sm leading-6 text-text-secondary">
			Mobs the bundled list lacks, so mob detection can name them. Type one as its
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
				aria-label="Mob as its plate shows it"
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
		<p class="text-sm text-text-tertiary">No mobs kept yet.</p>
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

	<section class="space-y-4 border-t border-border/40 pt-6" aria-labelledby="mob-labels-heading">
		<header>
			<h2 id="mob-labels-heading" class="text-sm font-medium text-text">Shown as</h2>
			<p class="mt-1 text-sm leading-6 text-text-secondary">
				A mob is shown as the game prints it. Give one another label here; it stays the
				same mob, in every session already recorded too.
			</p>
		</header>

		<ErrorNotice message={labels.error} onDismiss={() => labels.dismissError()} />

		<form
			class="flex flex-wrap items-center gap-3"
			onsubmit={(event) => {
				event.preventDefault();
				void labels.save();
			}}
		>
			<Input
				class="flex-1 min-w-56"
				placeholder="Mob, as the game shows it"
				aria-label="Mob to label"
				bind:value={() => labels.mob, (value) => (labels.mob = String(value ?? ''))}
			/>
			<Input
				class="flex-1 min-w-56"
				placeholder="Show as"
				aria-label="Label"
				maxlength={64}
				bind:value={() => labels.label, (value) => (labels.label = String(value ?? ''))}
			/>
			<Button type="submit" size="sm" disabled={!labels.ready} loading={labels.busy}>
				Set label
			</Button>
		</form>

		{#if labels.labels.length > 0}
			<ul class="divide-y divide-border/40 border-t border-border/40">
				{#each labels.labels as held (labelKey(held))}
					<li class="flex items-center gap-4 py-2.5" data-testid="mob-label">
						<div class="min-w-0 flex-1">
							<p class="truncate text-sm text-text">{held.label}</p>
							<p class="text-xs text-text-tertiary">
								{held.printed}{#if !held.byPlayer} · as its plate prints it{/if}
							</p>
						</div>
						<Button
							size="sm"
							variant="ghost"
							disabled={labels.busy}
							aria-label="Change the label of {held.printed}"
							onclick={() => labels.edit(held)}
						>
							Change
						</Button>
						<Button
							size="sm"
							variant="ghost"
							disabled={labels.busy}
							aria-label="Remove the label of {held.printed}"
							onclick={() => void labels.remove(held)}
						>
							Remove label
						</Button>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
</div>
