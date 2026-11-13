<script lang="ts">
	// The overlay strip's mob control: the session's one source for its
	// kills' creature. Without screen reading it is the typed mob, as it
	// always was: an input while none is typed, else the name with a clear.
	// While the screen is read (or a creature is pinned) it shows the one
	// creature standing highest, with one action beside it (pin it, or
	// release the pin); expanding it, by hover or by its toggle, reveals the
	// others and lets the backstop be set and a creature be typed to pin. The
	// expanded panel is a drawn overlay surface only while it is open, so
	// clicks reach the game everywhere else (the shared expanding shell).
	import type { MobSourceView } from '$lib/api';
	import HoverFacet from '$lib/components/overlay/HoverFacet.svelte';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { NO_DATA } from '$lib/utils/format';
	import { expandedLines, primaryAction, standing } from './mobSource';
	import type { MobSourceModel } from './mobSourceModel.svelte';

	const noop = () => {};

	let {
		view,
		model,
		inputAvailable = true,
		selectingMob = false,
		releasing = false,
		mobQuery = $bindable(''),
		mobInput = $bindable(null),
		onMobFocus = noop,
		onMobBlur = noop,
		onMobKeydown = noop,
		onReleaseMob = noop,
	}: {
		view: MobSourceView | null;
		model: MobSourceModel;
		/** The strip is in a state that takes a typed creature. */
		inputAvailable?: boolean;
		selectingMob?: boolean;
		releasing?: boolean;
		mobQuery?: string;
		mobInput?: HTMLInputElement | null;
		onMobFocus?: () => void;
		onMobBlur?: () => void;
		onMobKeydown?: (event: KeyboardEvent) => void | Promise<void>;
		/** Clear the typed mob (the strip's own clear, in typed mode). */
		onReleaseMob?: () => void | Promise<void>;
	} = $props();

	const shown = $derived(standing(view));
	const action = $derived(primaryAction(view));
	const lines = $derived(expandedLines(view));
	const saving = $derived(model.busy !== null);
	const panelId = 'mob-source-panel';
</script>

{#snippet creature()}
	{#if shown.creature}
		<span class="truncate text-white/90">{shown.creature.name}</span>
	{:else}
		<span class="text-white/20">{NO_DATA}</span>
	{/if}
{/snippet}

{#snippet typedInput(placeholder: string)}
	<input
		bind:this={mobInput}
		class="w-full bg-transparent border-b border-white/10 focus:border-accent text-sm text-white/90 px-1 py-0.5 outline-none placeholder:text-white/20 transition-colors"
		bind:value={mobQuery}
		{placeholder}
		aria-label="Creature"
		disabled={selectingMob || saving}
		onfocus={() => {
			model.hold();
			onMobFocus();
		}}
		onblur={onMobBlur}
		onkeydown={onMobKeydown}
	/>
{/snippet}

{#if !model.steered}
	<!-- The typed mob: the source of each kill's mob stamp, changeable
		 mid-session. -->
	<div class="flex items-center gap-2 shrink-0 border-r border-white/10 pr-3" data-testid="mob-facet">
		<div class="w-32 flex flex-col shrink-0">
			<span class="facet-label">Mob</span>
			<div class="flex items-center">
				{#if inputAvailable && model.inputShown}
					{@render typedInput('Mob...')}
				{:else if view?.typed}
					<div class="text-sm font-medium text-white/90 truncate px-1 w-full">{view.typed.name}</div>
				{:else}
					<div class="text-sm font-medium text-white/20 px-1">{NO_DATA}</div>
				{/if}
			</div>
		</div>
		{#if view?.typed}
			<button
				type="button"
				class="release-btn shrink-0"
				aria-label="Release mob"
				onclick={onReleaseMob}
				title="Release mob"
			>
				{releasing ? '...' : 'x'}
			</button>
		{/if}
	</div>
{:else}
	<!-- The steered control, in the shared expanding shell. -->
	<HoverFacet
		panel={model}
		label="Mob"
		panelLabel="Where the creature comes from"
		{panelId}
		toggleTitle="Where the creature comes from"
		toggleTestId="mob-shown"
		toggleData={{ standing: shown.kind }}
		panelTestId="mob-source-panel"
		testId="mob-facet"
		class="mob-facet flex items-center gap-2 shrink-0 border-r border-white/10 pr-3"
	>
		{#snippet control(toggle)}
			<div class="w-32 flex flex-col shrink-0">
				<span class="facet-label">Mob</span>
				{@render toggle(creature)}
			</div>
			{#if action === 'pin'}
				<button
					type="button"
					class="facet-chip shrink-0 whitespace-nowrap"
					disabled={saving}
					title="Every kill takes this creature until you release it"
					onclick={() => void model.pinShown()}
				>
					Pin
				</button>
			{:else if action === 'release'}
				<button
					type="button"
					class="facet-chip facet-chip-open shrink-0 whitespace-nowrap"
					disabled={saving}
					title="Go back to reading the creature off the screen"
					onclick={() => void model.release()}
				>
					Release
				</button>
			{/if}
		{/snippet}
		{#snippet content()}
			<span class="facet-label mb-2 block">Mob</span>
			{#each lines as line (line.kind)}
				<div class="mob-line" data-testid="mob-line" data-kind={line.kind}>
					<div class="flex items-baseline justify-between gap-3">
						<span class="text-[10px] text-white/40">{line.label}</span>
						{#if line.kind === 'backstop' && line.creature}
							<button
								type="button"
								class="text-[10px] text-white/40 hover:text-white/80 disabled:opacity-40"
								disabled={saving}
								onclick={() => void model.clearTyped()}
							>
								Clear
							</button>
						{/if}
					</div>
					<div class="text-sm {line.creature ? 'text-white/85' : 'text-white/25'} truncate">
						{line.creature?.name ?? NO_DATA}
					</div>
					{#if line.note}
						<p class="text-[10px] leading-snug text-white/35">{line.note}</p>
					{/if}
				</div>
			{/each}
			<div class="mob-line">
				{#if inputAvailable}
					{@render typedInput('Type a creature...')}
				{/if}
				{#if model.draft}
					{@const picked = model.draft}
					<div class="mt-2 flex items-center gap-2" data-testid="mob-draft">
						<span class="min-w-0 flex-1 truncate text-sm text-white/85">{picked.display}</span>
						<button type="button" class="facet-chip whitespace-nowrap" disabled={saving} onclick={() => void model.pinDraft()}>
							Pin
						</button>
						<!-- While pinned, a typed creature replaces the pin: it is
							 never set as a backstop behind it. -->
						{#if !view?.pin}
							<button type="button" class="facet-chip whitespace-nowrap" disabled={saving} onclick={() => void model.backstopDraft()}>
								Use as backstop
							</button>
						{/if}
					</div>
				{/if}
			</div>
			<div class="mob-line">
				<InDevelopmentMark id="creature-detection" inSurface />
			</div>
		{/snippet}
	</HoverFacet>
{/if}

<style>
	.mob-line + .mob-line {
		margin-top: 8px;
	}

	.release-btn {
		width: 18px;
		height: 18px;
		border-radius: 4px;
		border: 1px solid rgba(255, 255, 255, 0.15);
		background: rgba(255, 255, 255, 0.05);
		color: rgba(255, 255, 255, 0.4);
		font-size: 10px;
		line-height: 1;
		cursor: pointer;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.release-btn:hover {
		background: rgba(255, 255, 255, 0.1);
		color: rgba(255, 255, 255, 0.7);
	}
</style>
