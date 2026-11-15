<script lang="ts">
	/**
	 * What the keys paid back as, in one bar: the ammo that returns the key,
	 * what goes to Nanocubes, and what is kept to sell. The share kept is
	 * where any markup lives, so it alone carries the accent.
	 */
	import { formatPed, formatQuantity } from '$lib/utils/format';

	let {
		ammoPed,
		cubedTt,
		keptTt,
		nanocubes,
	}: {
		ammoPed: number;
		cubedTt: number;
		keptTt: number;
		nanocubes: number;
	} = $props();

	const total = $derived(Math.max(ammoPed + cubedTt + keptTt, 1e-9));
	const segments = $derived([
		{
			id: 'ammo',
			label: 'Ammo',
			value: ammoPed,
			detail: null as string | null,
			swatch: 'bg-text-tertiary/35',
		},
		{
			id: 'cubes',
			label: 'To Nanocubes',
			value: cubedTt,
			detail: nanocubes > 0 ? `${formatQuantity(nanocubes)} cubes` : null,
			swatch: 'bg-text-secondary/70',
		},
		{
			id: 'kept',
			label: 'Kept',
			value: keptTt,
			detail: null,
			swatch: 'bg-accent',
		},
	]);
</script>

<div class="space-y-3">
	<div
		class="flex h-2 w-full overflow-hidden rounded-full bg-surface-raised/60"
		role="img"
		aria-label={segments.map((s) => `${s.label} ${formatPed(s.value)} PED`).join(', ')}
	>
		{#each segments as segment (segment.id)}
			{#if segment.value > 0}
				<span
					class="segment h-full {segment.swatch} first:rounded-l-full last:rounded-r-full"
					style="width: {(segment.value / total) * 100}%"
				></span>
			{/if}
		{/each}
	</div>
	<dl class="flex flex-wrap gap-x-8 gap-y-2">
		{#each segments as segment (segment.id)}
			<div class="flex items-baseline gap-2">
				<span class="h-2 w-2 shrink-0 translate-y-[-1px] rounded-full {segment.swatch}"></span>
				<dt class="text-xs text-text-tertiary">{segment.label}</dt>
				<dd class="text-sm tabular-nums text-text">
					{formatPed(segment.value)}
					{#if segment.detail}
						<span class="ml-1 text-xs text-text-tertiary">{segment.detail}</span>
					{/if}
				</dd>
			</div>
		{/each}
	</dl>
</div>

<style>
	.segment {
		transition: width 320ms var(--ease-out);
		animation: grow 520ms var(--ease-out) both;
		transform-origin: left center;
	}
	@keyframes grow {
		from {
			transform: scaleX(0);
		}
		to {
			transform: scaleX(1);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.segment {
			transition: none;
			animation: none;
		}
	}
</style>
