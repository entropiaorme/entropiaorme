<script lang="ts">
	import { untrack } from 'svelte';
	import { quintOut } from 'svelte/easing';
	import Button from '$lib/components/Button.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import Select from '$lib/components/Select.svelte';
	import Toggle from '$lib/components/Toggle.svelte';
	import { shouldSettleInstantly } from '$lib/motion/testMotion';
	import type {
		DefinitionsModel,
		QuestCategoryGroup,
		RosterDraftEntry
	} from './definitionsModel.svelte';

	let {
		model
	}: {
		model: DefinitionsModel;
	} = $props();

	let nameInput = $state<HTMLInputElement | null>(null);
	let sourceFilter = $state('');

	// Activities are progressive disclosure, and so is the catalogue
	// inside them: a planet narrows it, categories fold their quests away
	// until asked for. Which categories are unfolded is pure view state;
	// the catalogue itself (planet, filter, grouping) belongs to the model.
	let activitiesOpen = $state(false);
	let unfolded = $state<Set<string>>(new Set());

	const editing = $derived(model.mode === 'edit');

	// A filter is a search: it unfolds everything it matched, because a
	// hit hidden inside a folded category reads as no hit at all.
	const filtering = $derived(model.catalogFilter.trim().length > 0);

	function isUnfolded(category: string): boolean {
		return filtering || unfolded.has(category);
	}

	// Null is the uncategorised tail, which has no fold of its own.
	function toggleCategory(category: string | null) {
		if (category === null) return;
		const next = new Set(unfolded);
		if (next.has(category)) next.delete(category);
		else next.add(category);
		unfolded = next;
	}

	function categoryFullyAdded(group: QuestCategoryGroup): boolean {
		return group.quests.every((quest) => model.hasRosterRef('quest', quest.id));
	}

	function kindLabel(entry: RosterDraftEntry): string {
		return entry.kind === 'quest_family' ? 'Family' : entry.kind === 'quest' ? 'Quest' : 'Segment';
	}

	/** The surface's entrance: it arrives AFTER the page content has
	 * animated away (the stage sequences that with a matching delay on
	 * its content wrapper), fading in with a slight upward settle; on
	 * close it leaves first, undelayed, and the page returns behind it.
	 * Reduced motion and the e2e freeze collapse both to an instant
	 * settle. */
	function surface(_node: HTMLElement, { entering }: { entering: boolean }) {
		if (shouldSettleInstantly()) return { duration: 0 };
		return {
			duration: entering ? 260 : 170,
			delay: entering ? 160 : 0,
			easing: quintOut,
			css: (t: number, u: number) => `opacity: ${t}; transform: translateY(${12 * u}px);`
		};
	}

	// Escape leaves without saving. Tab is deliberately NOT trapped: the
	// environment takes over the page's own region, not the window, so the
	// sidebar and titlebar stay reachable and navigating away is a valid
	// way out of it.
	function handleKeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		if (model.archiveArmed) {
			model.archiveArmed = false;
			return;
		}
		model.close();
	}

	// On open: remember the opener and move focus to the name field; on
	// close hand focus back. The disclosure starts open only when there is
	// something authored to see, and the planet choice starts fresh
	// (untracked: neither should move again while the editor is open).
	$effect(() => {
		if (model.mode === 'closed') return;
		void model.editingId;
		untrack(() => {
			activitiesOpen = model.roster.length > 0 || model.adHocSegments;
			unfolded = new Set();
		});
		const previouslyFocused =
			document.activeElement instanceof HTMLElement ? document.activeElement : null;
		nameInput?.focus();
		return () => {
			previouslyFocused?.focus();
		};
	});
</script>

<svelte:window onkeydown={model.mode !== 'closed' ? handleKeydown : undefined} />

{#if model.mode !== 'closed'}
	<div
		class="absolute inset-0 z-30 overflow-y-auto bg-base focus:outline-hidden"
		role="region"
		aria-label={editing ? 'Edit session' : 'New session'}
		tabindex="-1"
		in:surface={{ entering: true }}
		out:surface={{ entering: false }}
	>
		<div class="mx-auto flex min-h-full w-full max-w-2xl flex-col gap-6 px-6 py-10">
			<!-- Header: identity + the exits -->
			<div class="flex items-start justify-between gap-4">
				<h2 class="text-2xl font-semibold tracking-tight text-text">
					{editing ? 'Edit session' : 'New session'}
				</h2>
				<button
					class="h-8 w-8 flex items-center justify-center rounded-md text-text-secondary
						cursor-pointer transition-colors duration-[var(--duration-fast)]
						hover:text-text hover:bg-surface-hover shrink-0"
					onclick={() => model.close()}
					aria-label="Close without saving"
					title="Close without saving (Esc)"
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4.5 w-4.5">
						<path
							d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
						/>
					</svg>
				</button>
			</div>

			<!-- Name: the session's identity, so it gets the headline slot -->
			<input
				bind:this={nameInput}
				bind:value={model.name}
				class="w-full bg-transparent border-b border-border focus:border-accent text-xl
					font-semibold tracking-tight text-text px-1 py-2 outline-none
					placeholder:text-text-tertiary transition-colors"
				placeholder="Name this session..."
				aria-label="Session name"
				disabled={model.saving}
			/>

			<div class="border-y border-border/60 py-4">
				<div class="flex items-start justify-between gap-6">
					<div>
						<p class="text-sm font-medium text-text">Armour costs</p>
						<p class="mt-1 max-w-lg text-sm leading-6 text-text-secondary">
							Record hits taken, so armour repairs and readings can be spread over this session.
						</p>
					</div>
					<Toggle
						checked={model.trackProtectionCosts}
						label="Track armour costs"
						disabled={model.saving}
						onchange={(checked) => (model.trackProtectionCosts = checked)}
					/>
				</div>
			</div>

			<!-- Activities: the roster and its on-the-fly option, folded away
				 behind one disclosure. What is authored here is what the
				 overlay's Activities control offers while the session runs. -->
			<section class="panel flex flex-col">
				<div class="flex items-center gap-2 pr-4">
					<button
						type="button"
						class="flex flex-1 items-center gap-2.5 px-4 py-3 text-left cursor-pointer
							transition-colors duration-[var(--duration-base)] hover:bg-surface-hover/40"
						aria-expanded={activitiesOpen}
						aria-controls="session-activities"
						onclick={() => (activitiesOpen = !activitiesOpen)}
					>
						<svg
							class="h-3.5 w-3.5 shrink-0 text-text-secondary transition-transform
								{activitiesOpen ? '' : '-rotate-90'}"
							fill="none"
							stroke="currentColor"
							viewBox="0 0 24 24"
							aria-hidden="true"
						>
							<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
						</svg>
						<span class="text-sm font-semibold text-text">Activities</span>
						{#if model.roster.length > 0}
							<span class="text-sm text-text-secondary tabular-nums">{model.roster.length}</span>
						{/if}
					</button>
				</div>

				{#if activitiesOpen}
					<div id="session-activities" class="flex flex-col gap-3 border-t border-border/50 p-4">
						{#if model.roster.length > 0}
							<ul class="flex flex-col gap-1">
								{#each model.roster as entry, i (entry.key)}
									<li
										class="flex items-center gap-2 rounded-md border border-border/60 bg-base/40 px-3 py-2"
									>
										<span class="eyebrow-strong w-[4.5rem] shrink-0">{kindLabel(entry)}</span>
										{#if entry.missing}
											<span class="text-sm text-warning truncate flex-1" title="The referenced item was deleted; this entry is dropped on save">
												{entry.displayName} (removed)
											</span>
										{:else}
											<span class="text-sm text-text truncate flex-1">{entry.displayName}</span>
										{/if}
										<button
											type="button"
											class="icon-button-row p-1 shrink-0"
											aria-label="Remove from the session"
											title="Remove"
											disabled={model.saving}
											onclick={() => model.removeEntry(i)}
										>&times;</button>
									</li>
								{/each}
							</ul>
						{/if}

						<!-- The catalogue, reached through its planet: pick one, then
							 what that planet offers appears, as a list rather than a
							 field of chips. Depth is carried by indentation, weight and
							 hairlines; nothing here is boxed. -->
						{#if model.catalogPlanets.length > 0}
							<div class="flex items-center gap-2">
								<label for="session-activity-planet" class="eyebrow-strong shrink-0">Planet</label>
								<Select
									id="session-activity-planet"
									class="max-w-[14rem]"
									bind:value={model.catalogPlanet}
									disabled={model.saving}
								>
									<option value={null}>Choose a planet</option>
									{#each model.catalogPlanets as option (option)}
										<option value={option}>{option}</option>
									{/each}
								</Select>
							</div>
						{:else if !model.sourcesLoading}
							<p class="text-sm text-text-secondary px-1">
								No quests or quest families yet; they are authored on the Quests page.
							</p>
						{/if}

						{#if model.catalogPlanet !== null}
							<SearchInput
								bind:value={model.catalogFilter}
								placeholder="Filter {model.catalogPlanet}..."
								loading={model.sourcesLoading}
							/>

							{#if model.catalogFamilies.length > 0}
								<div class="flex flex-col">
									<span class="eyebrow-strong py-1">Quest families</span>
									{#each model.catalogFamilies as fam (fam.id)}
										{@const added = model.hasRosterRef('quest_family', fam.id)}
										<button
											type="button"
											class="catalogue-row"
											disabled={added || model.saving}
											title={added ? 'Already in this session' : 'Add to this session'}
											onclick={() => model.addFamily(fam)}
										>
											<span class="flex-1 truncate text-sm {added ? 'text-text-tertiary' : 'text-text'}">
												{fam.name}
											</span>
											<span class="row-action">{added ? 'Added' : 'Add'}</span>
										</button>
									{/each}
								</div>
							{/if}

							{#if model.catalogCategories.length > 0}
								<div class="flex flex-col">
									<span class="eyebrow-strong py-1">Quests</span>
									{#each model.catalogCategories as group (group.category ?? '\u0000loose')}
										{#if group.category === null}
											{#each group.quests as quest (quest.id)}
												{@const added = model.hasRosterRef('quest', quest.id)}
												<button
													type="button"
													class="catalogue-row"
													disabled={added || model.saving}
													title={added ? 'Already in this session' : 'Add to this session'}
													onclick={() => model.addQuest(quest)}
												>
													<span class="flex-1 truncate text-sm {added ? 'text-text-tertiary' : 'text-text'}">
														{quest.name}
													</span>
													<span class="row-action">{added ? 'Added' : 'Add'}</span>
												</button>
											{/each}
										{:else}
											{@const open = isUnfolded(group.category)}
											{@const allAdded = categoryFullyAdded(group)}
											<div
												class="flex items-center gap-2 {open
													? 'category-head-open'
													: 'border-b border-border/40'}"
											>
												<button
													type="button"
													class="flex flex-1 items-center gap-2 py-2 text-left cursor-pointer min-w-0
														transition-colors duration-[var(--duration-base)]
														hover:text-text"
													aria-expanded={open}
													disabled={filtering}
													onclick={() => toggleCategory(group.category)}
												>
													<svg
														class="h-3 w-3 shrink-0 transition-[transform,color]
															duration-[var(--duration-base)]
															{open ? 'text-accent' : '-rotate-90 text-text-secondary'}"
														fill="none"
														stroke="currentColor"
														viewBox="0 0 24 24"
														aria-hidden="true"
													>
														<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
													</svg>
													<span class="truncate text-sm font-semibold tracking-tight {allAdded
														? 'text-text-tertiary'
														: 'text-text'}">
														{group.category}
													</span>
													<span class="text-xs text-text-tertiary tabular-nums">{group.quests.length}</span>
												</button>
												<button
													type="button"
													class="row-action shrink-0"
													disabled={allAdded || model.saving}
													title={allAdded
														? 'Every quest here is already in this session'
														: `Add all ${group.quests.length} to this session`}
													onclick={() => model.addQuests(group.quests)}
												>
													{allAdded ? 'Added' : 'Add all'}
												</button>
											</div>
											{#if open}
												<div class="category-children">
												{#each group.quests as quest (quest.id)}
													{@const added = model.hasRosterRef('quest', quest.id)}
													<button
														type="button"
														class="catalogue-row catalogue-row-child"
														disabled={added || model.saving}
														title={added ? 'Already in this session' : 'Add to this session'}
														onclick={() => model.addQuest(quest)}
													>
														<span class="flex-1 truncate text-[13px] {added
															? 'text-text-tertiary'
															: 'text-text-secondary'}">
															{quest.name}
														</span>
														<span class="row-action">{added ? 'Added' : 'Add'}</span>
													</button>
												{/each}
												</div>
											{/if}
										{/if}
									{/each}
								</div>
							{/if}

							{#if !model.sourcesLoading && model.catalogFamilies.length === 0 && model.catalogCategories.length === 0}
								<p class="text-sm text-text-secondary px-1">
									{filtering
										? 'Nothing matches the filter.'
										: `Nothing on ${model.catalogPlanet} yet; quests are authored on the Quests page.`}
								</p>
							{/if}
						{/if}

						<!-- Segments are the activities the player names themselves, so
							 they are one capability with one switch: off, the session has
							 no segment concept at all; on, they are named in the overlay as
							 they happen and join this list from there. Nothing is
							 pre-ordered here: a slice you have not played yet is not one
							 you know the name of. -->
						<div class="flex items-center justify-between gap-4 border-t border-border/50 pt-3">
							<div class="flex flex-col gap-0.5">
								<span class="text-sm text-text">Name segments on the fly</span>
								<span class="text-sm text-text-secondary leading-relaxed max-w-sm">
									Name what you are doing as you play; each name you use joins this
									list for next time.
								</span>
							</div>
							<Toggle
								checked={model.adHocSegments}
								disabled={model.saving}
								label="Name segments on the fly"
								onchange={(checked) => (model.adHocSegments = checked)}
							/>
						</div>
					</div>
				{/if}
			</section>

			{#if model.authoringError}
				<ErrorNotice message={model.authoringError} />
			{/if}

			<div class="mt-auto flex items-center justify-between gap-3 pt-2">
				<div>
					<!-- A protected session is the one tracking always has to run
						 under, so it offers no archive; everything else about it is
						 editable. The service refuses the archive too. -->
					{#if editing && !model.editingProtected}
						{#if model.archiveArmed}
							<div class="flex items-center gap-2">
								<span class="max-w-72 text-xs leading-snug text-text-secondary">
									Remove from play choices? History and activities stay intact. Restore from the
									session review menu.
								</span>
								<Button
									size="sm"
									variant="ghost"
									disabled={model.saving}
									onclick={() => (model.archiveArmed = false)}
								>
									{#snippet children()}Cancel{/snippet}
								</Button>
								<Button
									size="sm"
									variant="danger"
									disabled={model.saving}
									onclick={() => model.archiveEditing()}
								>
									{#snippet children()}Archive{/snippet}
								</Button>
							</div>
						{:else}
							<Button
								size="sm"
								variant="ghost"
								disabled={model.saving}
								onclick={() => model.archiveEditing()}
							>
								{#snippet children()}
									<svg
										xmlns="http://www.w3.org/2000/svg"
										fill="none"
										viewBox="0 0 24 24"
										stroke-width="1.5"
										stroke="currentColor"
										class="h-3.5 w-3.5"
										aria-hidden="true"
									>
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											d="M20.25 7.5l-.625 10.632a2.25 2.25 0 0 1-2.247 2.118H6.622a2.25 2.25 0 0 1-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z"
										/>
									</svg>
									Archive
								{/snippet}
							</Button>
						{/if}
					{/if}
				</div>
				<div class="flex items-center gap-2">
					<Button size="sm" variant="secondary" disabled={model.saving} onclick={() => model.close()}>
						{#snippet children()}Cancel{/snippet}
					</Button>
					<Button size="sm" disabled={model.saving || !model.name.trim()} onclick={() => model.save()}>
						{#snippet children()}{model.saving ? 'Saving...' : editing ? 'Save changes' : 'Create'}{/snippet}
					</Button>
				</div>
			</div>
		</div>
	</div>
{/if}

<style>
	/* A catalogue line: full-width, hairline-separated, no border box.
	   Hierarchy comes from indentation, weight and tone. */
	.catalogue-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		width: 100%;
		padding: 0.5rem 0.25rem 0.5rem 0;
		text-align: left;
		cursor: pointer;
		border-bottom: 1px solid color-mix(in oklab, var(--color-border) 40%, transparent);
		transition: background-color var(--duration-base) var(--ease-out);
	}
	.catalogue-row:hover:not(:disabled) {
		background: color-mix(in oklab, var(--color-surface-hover) 30%, transparent);
	}
	.catalogue-row:disabled {
		cursor: default;
	}
	/* Containment without a container: an open category grows a rail from
	   under its own chevron, each child reaches it with a short elbow, and
	   the rail stops at the last child, closing the group in an L. Child
	   rules are inset to the rail, so the separator rhythm itself says
	   "these belong to the row above". */
	.category-head-open {
		border-bottom: 1px solid transparent;
	}
	.category-children {
		position: relative;
		padding-left: 1.5rem;
	}
	.category-children::before {
		content: '';
		position: absolute;
		left: 0.375rem;
		top: 0;
		/* Half a row: the rail lands on the last elbow, not past it. */
		bottom: 1.15rem;
		width: 1px;
		background: color-mix(in oklab, var(--color-border-bright) 55%, transparent);
	}
	.catalogue-row-child {
		position: relative;
	}
	.catalogue-row-child::before {
		content: '';
		position: absolute;
		left: -1.125rem;
		top: 50%;
		width: 0.75rem;
		height: 1px;
		background: color-mix(in oklab, var(--color-border-bright) 45%, transparent);
	}

	.row-action {
		font-size: 0.75rem;
		font-weight: 500;
		color: var(--color-accent);
		padding: 0.25rem 0.5rem;
		cursor: pointer;
		background: transparent;
		transition: color var(--duration-base) var(--ease-out);
	}
	.catalogue-row:disabled .row-action,
	.row-action:disabled {
		color: var(--color-text-tertiary);
		cursor: default;
	}
</style>
