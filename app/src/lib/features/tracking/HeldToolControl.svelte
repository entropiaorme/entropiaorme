<script lang="ts">
	// The strip's tool in hand while it is read off the game screen: what the
	// screen shows, quietly, in the shared expanding shell. Expanding it, by
	// hover or by its toggle, says what the state means and offers the action
	// that settles it: declaring an entry, which stands until the screen
	// shows what is held, or the equipment picker.
	import HoverFacet from '$lib/components/overlay/HoverFacet.svelte';
	import type { HoverPanel } from '$lib/components/overlay/hoverPanel.svelte';
	import { InDevelopmentMark } from '$lib/inDevelopment';
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

	function declare(equipmentId: number) {
		panel.close();
		void onDeclare(equipmentId);
	}
</script>

{#snippet shows()}
	<span class="truncate text-xs {TONES[display.tone]}" data-testid="held-tool-label" data-tone={display.tone}>
		{display.label}
	</span>
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
		{#each lines as line (line)}
			<p class="text-[11px] leading-snug text-white/70" data-testid="held-tool-line">{line}</p>
		{/each}
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
		<div class="mt-2">
			<InDevelopmentMark id="held-tool-reading" inSurface />
		</div>
	{/snippet}
</HoverFacet>
