<script lang="ts">
	import { flip } from 'svelte/animate';
	import { shouldSettleInstantly } from '$lib/motion/testMotion';
	import { quintOut } from 'svelte/easing';
	import { get } from 'svelte/store';
	import { ALL_STAT_IDS, STAT_DEFS } from '$lib/statsRegistry';
	import {
		dashboardStats,
		overlayStats,
		setDashboardStats,
		setOverlayStats,
		type StatPref,
	} from '$lib/statsCustomisation';

	type Surface = 'dashboard' | 'overlay';

	function handlePillClick(surface: Surface, index: number, prefs: StatPref[]) {
		const next = prefs.map((p, i) =>
			i === index ? { ...p, enabled: !p.enabled } : p
		);
		if (surface === 'dashboard') void setDashboardStats(next);
		else void setOverlayStats(next);
	}

	const surfaces: { surface: Surface; title: string }[] = [
		{ surface: 'dashboard', title: 'Dashboard' },
		{ surface: 'overlay', title: 'Overlay' },
	];

	function restoreOrder() {
		const restore = (prefs: StatPref[]): StatPref[] => {
			const enabledMap = new Map(prefs.map((p) => [p.id, p.enabled]));
			return ALL_STAT_IDS.map((id) => ({
				id,
				enabled: enabledMap.get(id) ?? STAT_DEFS[id].defaultEnabled,
			}));
		};
		void setDashboardStats(restore(get(dashboardStats)));
		void setOverlayStats(restore(get(overlayStats)));
	}
</script>

<div class="flex-1 min-h-0 flex flex-col gap-3 overflow-y-auto" data-guide-anchor="customise-stats-area">
	<div class="flex items-center justify-between gap-3 pb-3 border-b border-border/60">
		<p class="text-xs text-text-tertiary">
			Click a pill to show or hide it on that surface. Reorder by dragging directly on the dashboard.
		</p>
		<button
			type="button"
			class="shrink-0 inline-flex items-center gap-1.5 rounded-md border border-border/55
				px-2.5 py-1 text-xs font-medium text-text-secondary
				hover:bg-accent-muted/20 hover:border-accent/40 hover:text-accent
				focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent
				transition-[background-color,border-color,color] duration-[var(--duration-base)] ease-[var(--ease-out)]"
			onclick={restoreOrder}
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				viewBox="0 0 20 20"
				fill="currentColor"
				class="h-3.5 w-3.5"
				aria-hidden="true"
			>
				<path fill-rule="evenodd" d="M15.312 11.424a5.5 5.5 0 0 1-9.201 2.466l-.312-.311h2.433a.75.75 0 0 0 0-1.5H3.989a.75.75 0 0 0-.75.75v4.242a.75.75 0 0 0 1.5 0v-2.43l.31.31a7 7 0 0 0 11.712-3.138.75.75 0 0 0-1.449-.39Zm1.23-3.723a.75.75 0 0 0 .219-.53V2.929a.75.75 0 0 0-1.5 0V5.36l-.31-.31A7 7 0 0 0 3.239 8.188a.75.75 0 1 0 1.448.389A5.5 5.5 0 0 1 13.89 6.11l.311.31h-2.432a.75.75 0 0 0 0 1.5h4.243a.75.75 0 0 0 .53-.219Z" clip-rule="evenodd" />
			</svg>
			Restore order
		</button>
	</div>

	{#each surfaces as { surface, title } (surface)}
		{@const prefs = surface === 'dashboard' ? $dashboardStats : $overlayStats}
		<div class="flex flex-col gap-2" data-customise-surface={surface}>
			<span class="eyebrow">{title}</span>
			<div class="grid grid-cols-[repeat(auto-fill,minmax(120px,1fr))] gap-2">
				{#each prefs as pref, i (pref.id)}
					{@const def = STAT_DEFS[pref.id]}
					<button
						type="button"
						data-pill-id={pref.id}
						animate:flip={{ duration: shouldSettleInstantly() ? 0 : 240, easing: quintOut }}
						class="flex items-center justify-center rounded-md border px-3 py-1.5 text-[13px] font-medium
							select-none
							transition-[background-color,border-color,color] duration-[var(--duration-base)] ease-[var(--ease-out)]
							focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent
							{pref.enabled
								? 'bg-accent-muted/30 border-accent/40 text-accent hover:bg-accent-muted/40'
								: 'bg-transparent border-border/55 text-text-tertiary hover:text-text-secondary hover:border-border-bright/70'}"
						aria-pressed={pref.enabled}
						onclick={() => handlePillClick(surface, i, prefs)}
					>
						{def?.label ?? pref.id}
					</button>
				{/each}
			</div>
		</div>
	{/each}
</div>
