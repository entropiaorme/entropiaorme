<script lang="ts">
	// The one marker for a surface that ships ahead of its capability. Sits
	// inside a block already gated on `inDevelopment.visible`, so it does not
	// re-check the channel: its job is to say *why* the surface is inert, not
	// whether it appears.
	//
	// One implementation on purpose. A second one drifts, and a drifted
	// marker is worse than none because it teaches the reader that the mark
	// is decoration.
	//
	// Two ways to say it. By default the explanation is a popover portalled
	// to the page (an InfoTip), free to overhang its container. `inSurface`
	// lays the explanation out in flow, under the badge, inside whatever holds
	// the mark: for an overlay window, which draws only on its marked
	// surfaces, so a popover outside them would be drawn where clicks belong
	// to the game.
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { inDevelopmentSurface } from './registry';

	let {
		id,
		align = 'right',
		inSurface = false,
	}: {
		/** A key registered in `IN_DEVELOPMENT_SURFACES`. */
		id: string;
		align?: 'left' | 'right';
		/** Lay the explanation out in flow, inside the mark's container. */
		inSurface?: boolean;
	} = $props();

	const surface = $derived(inDevelopmentSurface(id));
	const explanationId = $derived(`in-development-${id}`);
	let open = $state(false);
</script>

{#snippet badge()}
	<span
		class="inline-flex items-center rounded border border-warning/40 bg-warning/10
			px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-warning"
	>
		In development
	</span>
{/snippet}

{#if inSurface}
	<div class="w-full" data-testid="in-development-mark">
		<button
			type="button"
			class="inline-flex"
			aria-expanded={open}
			aria-controls={explanationId}
			aria-label="In development: {surface.summary}"
			onclick={() => (open = !open)}
			onpointerenter={() => (open = true)}
			onpointerleave={() => (open = false)}
			onfocus={() => (open = true)}
			onblur={() => (open = false)}
		>
			{@render badge()}
		</button>
		{#if open}
			<div id={explanationId} class="mt-1">
				<p class="text-xs leading-relaxed text-text-secondary">{surface.summary}</p>
				<p class="mt-1 text-xs leading-relaxed text-text-tertiary">{surface.graduates}</p>
			</div>
		{/if}
	</div>
{:else}
	<InfoTip {align} width="w-80" label="In development: {surface.summary}">
		{#snippet trigger()}
			{@render badge()}
		{/snippet}
		<p class="text-xs leading-relaxed text-text-secondary">{surface.summary}</p>
		<p class="mt-2 text-xs leading-relaxed text-text-tertiary">{surface.graduates}</p>
	</InfoTip>
{/if}
