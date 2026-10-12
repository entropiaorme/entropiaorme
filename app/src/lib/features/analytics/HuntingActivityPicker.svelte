<script lang="ts">
	import Menu from '$lib/components/Menu.svelte';
	import { filterByQuery } from '$lib/view/search';
	import DefinitionCataloguePanel from '$lib/features/sessions/DefinitionCataloguePanel.svelte';
	import { NO_DATA, formatPed, formatPercent } from '$lib/utils/format';
	import type { HuntingActivitySection } from './huntingModel.svelte';

	let {
		activities,
		selected,
		onselect,
	}: {
		activities: HuntingActivitySection[];
		selected: HuntingActivitySection;
		onselect: (key: string) => void;
	} = $props();

	type ActivityEntry = { activity: HuntingActivitySection; depth: number };

	function flatten(rows: HuntingActivitySection[], depth = 0): ActivityEntry[] {
		const ordered = [
			...rows.filter((activity) => !activity.isUnscoped),
			...rows.filter((activity) => activity.isUnscoped),
		];
		return ordered.flatMap((activity) => [
			{ activity, depth },
			...flatten(activity.variants, depth + 1),
		]);
	}

	let query = $state('');
	const entries = $derived(flatten(activities));
	const filtered = $derived(filterByQuery(entries, query, ({ activity }) => activity.label));
	const COL_NAME = 'min-w-0 flex-[1_1_9rem]';
	const COL_CYCLED = 'min-w-0 flex-[0_1_4.5rem]';
	const COL_TT = 'min-w-0 flex-[0_1_4.5rem]';
	const COL_REWARDED = 'min-w-0 flex-[0_1_6.5rem]';
</script>

<Menu
	ariaLabel="Switch session activity"
	overlay
	align="left"
	initialFocus="first-input"
	overlayOverflow="hidden"
	panelClass="w-[min(38rem,calc(100vw-1rem))] p-0"
	class="w-full min-w-0"
>
	{#snippet trigger({ open, toggle, keydown })}
		<button
			type="button"
			class="group -ml-1.5 flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left
				transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover focus:outline-none
				focus:bg-surface-hover focus:[box-shadow:var(--shadow-glow)]"
			aria-haspopup="menu"
			aria-expanded={open}
			aria-label={`Switch session activity (currently ${selected.label})`}
			onclick={() => {
				if (!open) query = '';
				toggle();
			}}
			onkeydown={(event) => {
				if (!open && event.key === 'ArrowDown') query = '';
				keydown(event);
			}}
		>
			<span class="min-w-0 flex-1">
				<span class="eyebrow block text-text-tertiary">Activity</span>
				<span class="mt-0.5 block break-words text-xl font-semibold tracking-tight text-text" title={selected.label}>
					{selected.label}
				</span>
			</span>
			<span class="shrink-0 text-text-secondary transition-colors group-hover:text-text" aria-hidden="true">
				<svg class="h-4 w-4 transition-transform {open ? 'rotate-180' : ''}" viewBox="0 0 20 20" fill="currentColor">
					<path fill-rule="evenodd" d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z" clip-rule="evenodd" />
				</svg>
			</span>
		</button>
	{/snippet}

	{#snippet children({ close })}
		<DefinitionCataloguePanel
			title="Choose activity"
			count={entries.length}
			bind:filter={query}
			hasMatches={filtered.length > 0}
			filterLabel="Filter session activities"
			filterPlaceholder="Filter activities..."
			emptyNoun="activities"
			resultsTestId="hunting-activity-results"
		>
			{#snippet results()}
				<div class="sticky top-0 z-10 flex items-center gap-2 border-b border-border/50 bg-surface-raised px-2.5 py-2 text-text-tertiary">
					<span class="eyebrow {COL_NAME}">Activity</span>
					<span class="eyebrow {COL_CYCLED} text-right">Cycled</span>
					<span class="eyebrow {COL_TT} text-right">TT Rate</span>
					<span class="eyebrow {COL_REWARDED} text-right">Rewarded Rate</span>
				</div>
				<div class="flex flex-col gap-0.5 p-1">
					{#each filtered as entry (entry.activity.key)}
						{@const activity = entry.activity}
						{@const current = activity.key === selected.key}
						<button
							type="button"
							role="menuitem"
							aria-current={current ? 'true' : undefined}
							class="flex w-full items-center gap-2 rounded-md border px-2.5 py-2.5 text-left
								transition-[background-color,border-color] duration-[var(--duration-fast)]
								{current
									? 'border-accent/35 bg-accent/[0.09]'
									: 'border-transparent hover:border-border/40 hover:bg-surface-hover'}"
							onclick={() => {
								if (!current) onselect(activity.key);
								close();
							}}
						>
							<span
								class="{COL_NAME} truncate text-sm font-medium tracking-tight
									{activity.isUnscoped ? 'text-text-tertiary' : current ? 'text-accent' : 'text-text'}"
								style={`padding-left: ${entry.depth * 0.75}rem`}
								title={activity.label}
							>
								{#if entry.depth > 0}<span class="mr-1 text-text-tertiary" aria-hidden="true">↳</span>{/if}
								{activity.label}
							</span>
							{#if activity.isUnscoped}
								<span class="sr-only">Activity metrics not applicable</span>
								<span class={COL_CYCLED} aria-hidden="true"></span>
								<span class={COL_TT} aria-hidden="true"></span>
								<span class="{COL_REWARDED} text-right text-xs text-text-tertiary">Not ranked</span>
							{:else}
								<span class="{COL_CYCLED} truncate text-right text-xs tabular-nums text-text">{formatPed(activity.cycled)}</span>
								<span class="{COL_TT} truncate text-right text-xs tabular-nums text-text">{formatPercent(activity.lootRate)}</span>
								<span class="{COL_REWARDED} truncate text-right text-xs tabular-nums text-text">
									{activity.rewardStatus === 'unverified' ? NO_DATA : formatPercent(activity.rewardedRate)}
								</span>
							{/if}
						</button>
					{/each}
				</div>
			{/snippet}
		</DefinitionCataloguePanel>
	{/snippet}
</Menu>
