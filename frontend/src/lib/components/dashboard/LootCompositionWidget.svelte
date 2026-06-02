<script lang="ts">
	import { getSessionDetail } from '$lib/api';
	import type { SessionDetail } from '$lib/types/tracking';
	import { formatPed } from '$lib/utils/format';
	import { useVisiblePoll } from '$lib/realtime/useVisiblePoll';

	let { sessionId }: { sessionId: string | null } = $props();

	let detail = $state<SessionDetail | null>(null);
	let loading = $state(false);
	let hovered = $state<string | null>(null);

	const POLL_MS = 3000;
	const MAX_VISIBLE = 7;

	// Curated palette tuned to the dark theme — distinct hues that read
	// cleanly against panel/surface backgrounds and stay cohesive with
	// existing accent (sky) and positive (teal) tokens.
	const PALETTE = [
		'#38bdf8', // sky (accent)
		'#2dd4bf', // teal (positive)
		'#fbbf24', // amber
		'#a78bfa', // violet
		'#f472b6', // rose
		'#34d399', // emerald
		'#60a5fa', // blue
	];
	const OTHER_COLOR = '#475569'; // slate

	type Segment = {
		key: string;
		name: string;
		quantity: number;
		ttValue: number;
		sharePct: number;
		color: string;
		rank: number;
		isOther: boolean;
		count: number;
	};

	let segments = $derived.by<Segment[]>(() => {
		if (!detail || detail.lootBreakdown.length === 0) return [];
		const sorted = [...detail.lootBreakdown].sort((a, b) => b.ttValue - a.ttValue);
		const total = sorted.reduce((s, item) => s + item.ttValue, 0);
		if (total <= 0) return [];

		const top = sorted.slice(0, MAX_VISIBLE);
		const rest = sorted.slice(MAX_VISIBLE);
		const out: Segment[] = top.map((item, i) => ({
			key: item.name,
			name: item.name,
			quantity: item.quantity,
			ttValue: item.ttValue,
			sharePct: (item.ttValue / total) * 100,
			color: PALETTE[i % PALETTE.length],
			rank: i + 1,
			isOther: false,
			count: 1,
		}));
		if (rest.length > 0) {
			const restTt = rest.reduce((s, item) => s + item.ttValue, 0);
			const restQty = rest.reduce((s, item) => s + item.quantity, 0);
			out.push({
				key: '__other__',
				name: `+${rest.length} more`,
				quantity: restQty,
				ttValue: restTt,
				sharePct: (restTt / total) * 100,
				color: OTHER_COLOR,
				rank: MAX_VISIBLE + 1,
				isOther: true,
				count: rest.length,
			});
		}
		return out;
	});

	$effect(() => {
		if (!sessionId) {
			detail = null;
			return;
		}
		const id = sessionId;
		let disposed = false;

		async function fetchOnce() {
			try {
				const d = await getSessionDetail(id);
				if (!disposed && sessionId === id) detail = d;
			} catch {
				/* ignore — keep previous snapshot */
			}
		}

		loading = detail === null;
		void fetchOnce().finally(() => {
			if (!disposed) loading = false;
		});
		const stop = useVisiblePoll(fetchOnce, { intervalMs: POLL_MS, immediate: false });

		return () => {
			disposed = true;
			stop();
		};
	});
</script>

<div class="flex-1 min-h-0 flex flex-col gap-4">
	{#if !sessionId}
		<!-- Idle: no active session -->
		<div class="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
			<div class="relative h-12 w-12 rounded-full border border-border/60 flex items-center justify-center
				before:absolute before:inset-[-6px] before:rounded-full before:border before:border-accent/15
				after:absolute after:inset-[-14px] after:rounded-full after:border after:border-accent/[0.06]">
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor"
					class="h-5 w-5 text-text-tertiary">
					<path fill-rule="evenodd" d="M10 1.5a.75.75 0 0 1 .75.75V4.5a.75.75 0 0 1-1.5 0V2.25A.75.75 0 0 1 10 1.5ZM5.404 4.343a.75.75 0 0 1 1.06 0l1.591 1.59a.75.75 0 1 1-1.06 1.061l-1.591-1.59a.75.75 0 0 1 0-1.061ZM14.596 4.343a.75.75 0 0 1 0 1.06l-1.591 1.591a.75.75 0 1 1-1.06-1.06l1.59-1.591a.75.75 0 0 1 1.061 0ZM2.25 9.25a.75.75 0 0 0 0 1.5H4.5a.75.75 0 0 0 0-1.5H2.25ZM15.5 10a.75.75 0 0 1 .75-.75h2.25a.75.75 0 0 1 0 1.5H16.25A.75.75 0 0 1 15.5 10ZM10 6a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z" clip-rule="evenodd" />
				</svg>
			</div>
			<div class="flex flex-col gap-1 max-w-xs">
				<p class="text-sm text-text-secondary tracking-tight">No active session</p>
				<p class="text-xs text-text-tertiary leading-relaxed">
					Start tracking to watch your loot composition in real time.
				</p>
			</div>
		</div>
	{:else if loading && !detail}
		<div class="flex-1 flex items-center justify-center">
			<p class="text-xs text-text-tertiary animate-pulse tracking-wider uppercase">Loading…</p>
		</div>
	{:else if segments.length === 0}
		<!-- Active but no loot yet -->
		<div class="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
			<span class="signal-dot positive animate-pulse"></span>
			<div class="flex flex-col gap-1 max-w-xs">
				<p class="text-sm text-text-secondary tracking-tight">Tracking: no loot yet</p>
				<p class="text-xs text-text-tertiary leading-relaxed">
					Items will appear here as they enter your session ledger.
				</p>
			</div>
		</div>
	{:else}
		<!-- Composition bar -->
		<div class="relative h-2.5 rounded-full overflow-hidden bg-base/60 border border-border/60 flex mt-3">
			{#each segments as seg (seg.key)}
				<button
					type="button"
					class="relative h-full transition-[opacity,filter] duration-[var(--duration-base)] ease-[var(--ease-out)]
						first:rounded-l-full last:rounded-r-full
						{hovered && hovered !== seg.key ? 'opacity-35' : ''}
						{hovered === seg.key ? '[filter:brightness(1.15)]' : ''}"
					style="width: {seg.sharePct}%; background-color: {seg.color};"
					aria-label="{seg.name}: {seg.sharePct.toFixed(1)}% of loot TT"
					onmouseenter={() => (hovered = seg.key)}
					onmouseleave={() => (hovered = null)}
					onfocus={() => (hovered = seg.key)}
					onblur={() => (hovered = null)}
				></button>
			{/each}
		</div>

		<!-- Column headers -->
		<div class="flex items-center gap-3 px-2.5 -mb-1">
			<div class="flex items-center gap-2.5 shrink-0">
				<span class="w-4"></span>
				<span class="w-2.5"></span>
			</div>
			<span class="eyebrow flex-1 min-w-0">Item</span>
			<span class="hidden sm:block w-20 shrink-0"></span>
			<span class="eyebrow w-20 text-right shrink-0">TT</span>
			<span class="eyebrow w-12 text-right shrink-0">Share</span>
		</div>

		<!-- Ranked list -->
		<div class="flex-1 min-h-0 overflow-y-auto -mr-2 pr-2">
			<ul class="flex flex-col gap-1">
				{#each segments as seg (seg.key)}
					<li>
						<div
							class="group relative flex items-center gap-3 rounded-md px-2.5 py-2
								transition-[background-color,border-color] duration-[var(--duration-base)] ease-[var(--ease-out)]
								border border-transparent
								{hovered === seg.key
									? 'bg-surface-hover/50 border-border/50'
									: 'hover:bg-surface-hover/30 hover:border-border/40'}"
							onmouseenter={() => (hovered = seg.key)}
							onmouseleave={() => (hovered = null)}
							role="presentation"
						>
							<!-- Rank + swatch -->
							<div class="flex items-center gap-2.5 shrink-0">
								<span class="text-[10.5px] font-medium tabular-nums text-text-tertiary tracking-[0.12em] w-4 text-right">
									{seg.rank}
								</span>
								<span
									class="block h-2.5 w-2.5 rounded-sm shrink-0
										transition-[box-shadow] duration-[var(--duration-base)] ease-[var(--ease-out)]"
									style="background-color: {seg.color};
										box-shadow: 0 0 0 1px color-mix(in oklab, {seg.color} 40%, transparent),
											{hovered === seg.key
												? `0 0 10px color-mix(in oklab, ${seg.color} 60%, transparent)`
												: 'none'};"
								></span>
							</div>

							<!-- Name + qty -->
							<div class="flex-1 min-w-0 flex items-center gap-2">
								<span class="text-sm font-medium truncate tracking-tight
									{seg.isOther ? 'text-text-tertiary italic' : 'text-text'}">
									{seg.name}
								</span>
								{#if !seg.isOther}
									<span class="text-xs text-text-tertiary tabular-nums shrink-0">
										×{seg.quantity}
									</span>
								{/if}
							</div>

							<!-- Mini share bar -->
							<div class="hidden sm:block w-20 h-1 rounded-full bg-base/60 overflow-hidden shrink-0">
								<div
									class="h-full rounded-full transition-[width] duration-[var(--duration-slow)] ease-[var(--ease-out)]"
									style="width: {seg.sharePct}%; background-color: {seg.color};"
								></div>
							</div>

							<!-- TT value -->
							<span class="text-sm tabular-nums font-medium text-text shrink-0 w-20 text-right">
								{formatPed(seg.ttValue)}
							</span>

							<!-- Share % — promoted to primary emphasis -->
							<span class="text-sm tabular-nums font-semibold shrink-0 w-12 text-right tracking-tight
								{seg.isOther ? 'text-text-tertiary' : 'text-accent'}">
								{seg.sharePct.toFixed(1)}%
							</span>
						</div>
					</li>
				{/each}
			</ul>
		</div>
	{/if}
</div>
