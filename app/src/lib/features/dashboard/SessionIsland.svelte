<script lang="ts">
	import { flip } from 'svelte/animate';
	import { quintOut } from 'svelte/easing';
	import { startTracking, toggleOverlay } from '$lib/api';
	import type { TrackingSnapshot } from '$lib/api';
	import { Button, ErrorNotice, Skeleton } from '$lib/components';
	import DefinitionPicker from '$lib/features/sessions/DefinitionPicker.svelte';
	import type { DefinitionsModel } from '$lib/features/sessions/definitionsModel.svelte';
	import { shouldSettleInstantly } from '$lib/motion/testMotion';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';
	import { hydrate } from '$lib/stores/trackingStore.svelte';
	import { getStatDef } from '$lib/statsRegistry';
	import { setStatsScope } from '$lib/statsScope.svelte';
	import { describeError } from '$lib/view/errorState';
	import type { StatsGridModel } from './statsGridModel.svelte';

	let {
		status,
		statsGrid,
		definitions,
		onReview
	}: {
		status: TrackingSnapshot | null;
		statsGrid: StatsGridModel;
		/** The sessions model; the route hosts it (and the authoring
		 * environment) so the surface can replace the whole dashboard. */
		definitions: DefinitionsModel;
		/** Open the review surface on a definition. Null means no
		 * selection has landed yet: the surface reads nothing and asks
		 * for one rather than showing every session ever recorded. */
		onReview: (definitionId: string | null) => void;
	} = $props();

	let elapsedSeconds = $state(0);

	// The family behind the flip. Present on every frame, idle included,
	// over the definition a start would stamp; absent means this session
	// belongs to no family, and the control is not offered at all.
	const lifetime = $derived(status?.lifetime ?? null);
	const showingLifetime = $derived(statsGrid.scope === 'lifetime' && lifetime !== null);

	/**
	 * The radio-group keyboard contract the control's own role promises:
	 * arrows move the selection, and only the selected option sits in
	 * the tab order (the `tabindex` binding below). With two options an
	 * arrow in any direction is a flip.
	 */
	function handleScopeKeydown(event: KeyboardEvent) {
		if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
		event.preventDefault();
		const next = showingLifetime ? 'instance' : 'lifetime';
		void setStatsScope(next).then(() => {
			// Selection moves focus with it, or the roving tabindex would
			// strand focus on an option that is no longer reachable by Tab.
			const group = (event.currentTarget as HTMLElement | null)?.closest(
				'[data-testid="stats-scope-toggle"]'
			);
			group?.querySelector<HTMLButtonElement>('[tabindex="0"]')?.focus();
		});
	}

	const SCOPE_CHOICES = [
		{
			value: 'instance' as const,
			label: 'This session',
			hint: 'The session in play on its own.'
		},
		{
			value: 'lifetime' as const,
			label: 'Lifetime',
			hint: 'Every session recorded under this definition, added together.'
		}
	];

	/** The span the lifetime figures cover, stated so a thin aggregate
	 * cannot read as a deep one. */
	const spanLabel = $derived.by(() => {
		const count = lifetime?.instanceCount ?? 0;
		if (count === 0) return 'no sessions yet';
		return count === 1 ? 'across 1 session' : `across ${count} sessions`;
	});

	function openAuthoring(definitionId: string | null) {
		const editing =
			definitionId === null
				? null
				: definitions.definitions.find((definition) => definition.id === definitionId);
		if (editing) definitions.openEdit(editing);
		else definitions.openCreate();
	}

	const isActive = $derived(status?.status === 'active');
	// No snapshot read has answered yet (the first read, which the transport
	// holds while the backend starts). Until one does, the island cannot say
	// whether a session is running, so it shows neither state.
	const pending = $derived(status === null);
	const selectedDefinitionId = $derived(status?.sessionDefinitionId ?? null);

	let starting = $state(false);
	let startError = $state<string | null>(null);

	async function handleStart() {
		starting = true;
		startError = null;
		try {
			await startTracking();
			await hydrate();
		} catch (e) {
			startError = describeError(e, 'Failed to start the session');
		} finally {
			starting = false;
		}
	}

	// Elapsed timer when tracking is active
	$effect(() => {
		if (status?.status === 'active' && status.started_at) {
			const startMs = new Date(status.started_at).getTime();
			elapsedSeconds = Math.max(0, Math.floor((Date.now() - startMs) / 1000));
			return useVisiblePoll(() => {
				elapsedSeconds = Math.max(0, Math.floor((Date.now() - startMs) / 1000));
			}, { intervalMs: 1000, immediate: false });
		} else {
			elapsedSeconds = 0;
		}
	});

	function formatElapsed(seconds: number): string {
		const h = Math.floor(seconds / 3600);
		const m = Math.floor((seconds % 3600) / 60);
		return h > 0 ? `${h}h ${m}m` : `${m}m`;
	}
</script>

{#snippet reviewChip()}
	<!-- Sits with the authoring affordances rather than the run controls:
		 reviewing what was recorded is management of the session list, not
		 an action on the session about to run. -->
	<span class="h-4 w-px shrink-0 bg-border/70" aria-hidden="true"></span>
	<button
		type="button"
		class="filter-chip shrink-0"
		title="Review recorded sessions"
		onclick={() => onReview(selectedDefinitionId)}
		data-testid="dashboard-review"
	>
		Review
	</button>
{/snippet}

<!-- Island: Session -->
<section class="panel p-4 flex flex-col gap-3 flex-shrink-0" aria-busy={pending}>
	<!-- Session strip -->
	<div class="relative flex items-center justify-between">
		{#if status?.status === 'active'}
			<div class="flex items-center gap-3 min-w-0">
				<span class="signal-dot positive animate-pulse"></span>
				<span class="text-sm font-medium text-text tracking-tight">Tracking active</span>
				{#if status.sessionName}
					<span
						class="text-xs text-text-secondary truncate"
						title="{status.sessionName} (fixed for this session)"
					>
						{status.sessionName}
					</span>
				{/if}
				<!-- Under the lifetime scope this reads the family's summed
					 duration, so the time beside the figures is the time
					 those figures cover. -->
				<span
					class="text-xs text-text-tertiary tabular-nums tracking-wider"
					data-testid="session-elapsed"
				>
					{formatElapsed(showingLifetime && lifetime ? lifetime.durationSeconds : elapsedSeconds)}
				</span>
				{#if showingLifetime}
					<span class="text-xs text-text-tertiary truncate" data-testid="lifetime-span">
						{spanLabel}
					</span>
				{/if}
				{@render reviewChip()}
			</div>
		{:else if pending}
			<div class="flex items-center min-w-0" data-testid="session-strip-pending">
				<Skeleton class="h-4 w-44" />
			</div>
		{:else}
			<!-- At rest the island is titled by the session it will run as;
				 the stats below already read @Rest, so nothing states it twice. -->
			<div class="flex items-center gap-3 min-w-0">
				<DefinitionPicker
					model={definitions}
					selectedId={selectedDefinitionId}
					onOpenAuthoring={openAuthoring}
				/>
				{@render reviewChip()}
				{#if showingLifetime && lifetime}
					<!-- Duration is a figure like any other, so it reads only
						 once there are instances behind it; below that the
						 span label carries the empty case on its own. -->
					{#if lifetime.instanceCount > 0}
						<span class="text-xs text-text-tertiary tabular-nums tracking-wider">
							{formatElapsed(lifetime.durationSeconds)}
						</span>
					{/if}
					<span class="text-xs text-text-tertiary truncate" data-testid="lifetime-span">
						{spanLabel}
					</span>
				{/if}
			</div>
		{/if}

		<div class="flex items-center gap-2">
			<!-- The instance/family flip. Both choices stay on screen so
				 the control reads as a choice rather than as a button
				 whose label might be its state or its action. Offered
				 only when there is a family to flip to: a session
				 belonging to no definition gets no control at all. -->
			{#if lifetime}
				<div class="flex items-center gap-2">
					<span class="text-xs text-text-secondary whitespace-nowrap" id="stats-scope-label">
						Show stats for:
					</span>
					<div
						role="radiogroup"
						aria-labelledby="stats-scope-label"
						data-testid="stats-scope-toggle"
						class="inline-flex items-center rounded-md border border-border/60 p-0.5 gap-0.5"
					>
						{#each SCOPE_CHOICES as choice (choice.value)}
							{@const selected = showingLifetime === (choice.value === 'lifetime')}
							<button
								type="button"
								role="radio"
								aria-checked={selected}
								tabindex={selected ? 0 : -1}
								title={choice.hint}
								onclick={() => setStatsScope(choice.value)}
								onkeydown={handleScopeKeydown}
								class="text-xs px-2 py-1 rounded whitespace-nowrap
									transition-colors duration-[var(--duration-base)]
									{selected
									? 'bg-accent/10 text-accent font-medium'
									: 'text-text-secondary hover:text-text hover:bg-base/60'}"
							>
								{choice.label}
							</button>
						{/each}
					</div>
				</div>
			{/if}
			<!-- The overlay is the surface play actually happens on, so it
				 carries the emphasis; starting is the once-per-session act
				 beside it. -->
			{#if !isActive && !pending}
				<Button size="sm" variant="secondary" disabled={starting} onclick={handleStart}>
					{#snippet children()}{starting ? 'Starting...' : 'Start tracking'}{/snippet}
				</Button>
			{/if}
			<span class="inline-flex" data-guide-anchor="dashboard-overlay-btn">
				<Button size="sm" variant="primary" onclick={() => toggleOverlay().catch(() => {})}>
					{#snippet children()}Overlay{/snippet}
				</Button>
			</span>
		</div>
	</div>

	<ErrorNotice
		message={startError ?? definitions.error}
		onDismiss={() => {
			startError = null;
			definitions.error = null;
		}}
	/>

	<!-- Session stats -->
	<div
		class="dashboard-stat-grid relative grid gap-2"
		data-guide-anchor="dashboard-stats-grid"
	>
		{#each statsGrid.enabledStats as pref, i (pref.id)}
			{@const def = getStatDef(pref.id)}
			{@const r = def
				? showingLifetime && def.renderLifetime && lifetime
					? def.renderLifetime(lifetime)
					: def.render(status)
				: { value: '\u2014', color: 'text-text-tertiary' }}
			{@const isDragged = statsGrid.dragFilteredIndex === i}
			<div
				animate:flip={{ duration: shouldSettleInstantly() ? 0 : 240, easing: quintOut }}
				data-stat-cell={i}
				role="group"
				aria-label={def?.label ?? pref.id}
				class="relative rounded-md border border-border/60 bg-base/40 px-3 py-2.5 flex flex-col gap-1
					min-w-0 select-none touch-none
					{statsGrid.reorderable ? 'cursor-grab' : ''}
					transition-[opacity,box-shadow,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
					before:pointer-events-none before:absolute before:inset-0 before:rounded-[inherit]
					before:[box-shadow:inset_0_1px_0_0_rgba(255,255,255,0.03)]
					{isDragged ? 'opacity-40 shadow-lg ring-1 ring-accent/60 z-10' : ''}"
				onpointerdown={(e) => statsGrid.handlePointerDown(e, i)}
				onpointermove={statsGrid.handlePointerMove}
				onpointerup={statsGrid.handlePointerUp}
				onpointercancel={statsGrid.handlePointerCancel}
			>
				<span class="flex min-w-0 items-center gap-1 eyebrow">
					<span class="truncate">{def?.shortLabel ?? def?.label ?? pref.id}</span>
				</span>
				{#if pending}
					<Skeleton class="h-[17px] w-14" />
				{:else}
					<span class="truncate text-[17px] font-semibold tabular-nums leading-none tracking-tight
						{r.value === '\u2014' ? 'text-text-tertiary' : r.color}">
						{r.value}
					</span>
				{/if}
			</div>
		{/each}
	</div>

	{#if status?.status === 'active'}
		{#if status.weaponAttribution === 'hotbar' && status.hotbarListenerActive === false}
			<div class="relative flex items-start gap-3 px-3.5 py-3 rounded-md border border-warning/30 bg-warning/[0.06]">
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4.5 w-4.5 mt-0.5 text-warning shrink-0">
					<path fill-rule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clip-rule="evenodd" />
				</svg>
				<div class="flex flex-col gap-0.5">
					<p class="text-sm font-medium text-warning tracking-tight">Hotbar key listener not active</p>
					<p class="text-xs text-text-secondary leading-relaxed">
						Cost attribution is using the hotbar but the listener isn't running. Check that the hotbar key listener is enabled in Settings.
					</p>
				</div>
			</div>
		{/if}
	{/if}
</section>

<style>
	.dashboard-stat-grid {
		grid-template-columns: repeat(
			auto-fill,
			minmax(clamp(112px, calc((100% - 2rem) / 5), 140px), 1fr)
		);
	}

	:global(body.stat-drag-active),
	:global(body.stat-drag-active *) {
		cursor: grabbing !important;
		user-select: none;
	}
</style>
