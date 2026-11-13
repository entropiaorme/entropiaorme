<script lang="ts">
	// The strip's tool in hand while it is read off the game screen: what the
	// screen shows, quietly, in the shared expanding shell, with a mark per
	// attachment fitted. Expanding it, by hover or by its toggle, names the
	// set-up in full with what a use is priced at, says what the state means,
	// and offers the action that settles it: declaring an entry, which stands
	// until the screen shows what is held, or the equipment picker.
	import HoverFacet from '$lib/components/overlay/HoverFacet.svelte';
	import type { HoverPanel } from '$lib/components/overlay/hoverPanel.svelte';
	import type { HeldToolDisplay, HeldToolFeedback, HeldToolTone } from './heldTool';

	const noop = () => {};

	let {
		display,
		panel,
		feedback = null,
		declaring = false,
		onDeclare = noop,
		onChoose = noop,
	}: {
		display: HeldToolDisplay;
		panel: HoverPanel;
		feedback?: HeldToolFeedback | null;
		declaring?: boolean;
		onDeclare?: (equipmentId: number) => void | Promise<void>;
		onChoose?: (anchor: HTMLElement) => void | Promise<void>;
	} = $props();

	const panelId = 'held-tool-panel';

	// The strip's own tones: the tool as it always read, the informational
	// tone of the strip's quiet cues for a tool known only from the game's
	// data, the muted tone of a tool not declared, and the attention tone of
	// the undeclared-weapon cue.
	const TONES: Record<HeldToolTone, string> = {
		normal: 'text-white/70',
		provisional: 'text-sky-300/90',
		muted: 'text-white/45',
		quiet: 'text-white/35',
		attention: 'text-amber-300/90',
	};
	// A hit cue stands ahead of the state's marker, as it stands ahead of the
	// activity under the hotbar's tool.
	const hitCue = $derived(feedback?.testId === 'weapon-hit-cue' ? feedback : null);
	const under = $derived(
		hitCue ??
			(display.marker
				? { text: display.marker, tone: 'text-white/35', detail: null, testId: 'held-tool-marker' }
				: feedback),
	);
	const lines = $derived(hitCue?.detail ? [...display.lines, hitCue.detail] : display.lines);
	const setup = $derived(display.setup);
	const marks = $derived(Array.from({ length: setup?.fitted ?? 0 }, (_, index) => index));
	// The name in full reads in the strip's tone, a step brighter.
	const NAME_TONES: Record<HeldToolTone, string> = {
		normal: 'text-white/85',
		provisional: 'text-sky-300/90',
		muted: 'text-white/55',
		quiet: 'text-white/45',
		attention: 'text-amber-300/90',
	};

	function declare(equipmentId: number) {
		panel.close();
		void onDeclare(equipmentId);
	}
</script>

{#snippet shows()}
	<span class="truncate text-xs {TONES[display.tone]}" data-testid="held-tool-label" data-tone={display.tone}>
		{display.label}
	</span>
	{#if setup?.fitted}
		<!-- One mark per attachment fitted: the set-up at a glance, named in
			 full when expanded. Never cut: the name gives way instead. -->
		<span
			class="flex shrink-0 items-center gap-[3px] {display.tone === 'muted' ? 'text-white/25' : 'text-white/40'}"
			aria-hidden="true"
			data-testid="held-tool-fitted"
			data-count={setup.fitted}
		>
			{#each marks as mark (mark)}
				<span class="h-[3px] w-[3px] rounded-full bg-current"></span>
			{/each}
		</span>
		<span class="sr-only">, {setup.fittedText}</span>
	{/if}
{/snippet}

<HoverFacet
	{panel}
	label="Tool in hand"
	{panelId}
	toggleTitle="The tool in hand"
	toggleTestId="held-tool"
	panelTestId="held-tool-panel"
	testId="held-tool-facet"
	class="flex flex-col w-32 min-w-0"
>
	{#snippet control(toggle)}
		{@render toggle(shows)}
		<!-- Always laid out, so a state with nothing to say here does not
			 move the strip. -->
		<div class="h-3 px-1 overflow-hidden">
			{#if under}
				<div
					class="text-[9px] leading-tight uppercase tracking-wider whitespace-nowrap truncate {under.tone}"
					title={under.detail ?? undefined}
					data-testid={under.testId}
				>
					{under.text}
				</div>
			{/if}
		</div>
	{/snippet}
	{#snippet content()}
		<span class="facet-label mb-2 block">Tool</span>
		{#if setup}
			<p class="text-sm leading-snug break-words {NAME_TONES[display.tone]}" data-testid="held-tool-name">
				{setup.name}
			</p>
		{/if}
		{#each lines as line (line)}
			<p class="text-[11px] leading-snug text-white/70" data-testid="held-tool-line">{line}</p>
		{/each}
		{#if setup}
			{#if setup.rows.length > 0}
				<dl
					class="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-[11px] leading-snug"
					data-testid="held-tool-setup"
				>
					{#each setup.rows as row (row.label)}
						<dt class="text-white/40">{row.label}</dt>
						<dd class="break-words text-white/75">{row.value}</dd>
					{/each}
				</dl>
			{/if}
			<div class="mt-2 border-t border-white/10 pt-2 tabular-nums" data-testid="held-tool-cost">
				<p class="leading-snug">
					<span class="text-sm text-white/85">{setup.cost}</span>
					<span class="text-[11px] text-white/45">{setup.per}</span>
				</p>
				{#if setup.tt}
					<p class="text-[10px] leading-snug text-white/40">{setup.tt}</p>
				{/if}
			</div>
		{/if}
		{#if display.actions.length > 0}
			<div class="mt-2 flex flex-col gap-1.5">
				{#each display.actions as action, index (index)}
					{#if action.kind === 'declare'}
						<div class="flex items-center gap-2">
							<span class="min-w-0 flex-1 truncate text-sm text-white/85">{action.name}</span>
							<button
								type="button"
								class="facet-chip shrink-0 whitespace-nowrap"
								disabled={declaring}
								aria-label={action.accessibleName}
								title="Take this as the tool in hand until the screen shows what is held"
								onclick={() => declare(action.equipmentId)}
							>
								{action.label}
							</button>
						</div>
					{:else}
						<button
							type="button"
							class="facet-chip self-start whitespace-nowrap"
							disabled={declaring}
							onclick={(event) => void onChoose(event.currentTarget)}
						>
							{action.label}
						</button>
					{/if}
				{/each}
			</div>
		{/if}
	{/snippet}
</HoverFacet>
