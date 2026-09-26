<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import Divider from '$lib/components/Divider.svelte';
	import ErrorNotice from '$lib/components/ErrorNotice.svelte';
	import SegmentedControl from '$lib/components/SegmentedControl.svelte';
	import { ANALYTICS_RANGES } from '$lib/features/analytics/analyticsRange';
	import {
		createOverviewModel,
		labelFor,
		PIE_C,
		PIE_R,
		PROGRESSION_GAIN_TAGS
	} from '$lib/features/analytics/overviewModel.svelte';
	import { formatDate, formatPed, formatPercent } from '$lib/utils/format';

	const model = createOverviewModel();

	let hoveredIdx = $state(-1);

	$effect(() => {
		void model.loadData(model.period);
	});
</script>

{#if model.loading}
	<p class="text-sm text-text-secondary">Loading overview...</p>
{:else if model.error}
	<ErrorNotice message={model.error} />
{:else if model.data}
	{@const data = model.data}
	{@const config = model.config}
	{@const pieView = model.pieView}
	<div class="space-y-6" data-guide-anchor="analytics-overview-area">

		<!-- Returns breakdown: donut + legend | gains/losses -->
		{#if pieView}
			<div>
				<div class="flex items-center justify-between gap-4 mb-3">
					<h3 class="eyebrow">
						Global Returns
					</h3>

					<SegmentedControl
						class="flex-shrink-0"
						options={ANALYTICS_RANGES.map((r) => ({ id: r, label: r }))}
						active={model.activeRange}
						onchange={(id) => (model.activeRange = id)}
					/>
				</div>
				<div class="flex flex-col gap-2 min-w-0 mb-4">
					<div class="flex items-center gap-1.5 flex-wrap">
						<span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary w-12 flex-shrink-0">
							Returns
						</span>
						<span
							class="px-2.5 py-1 text-xs font-medium rounded-md bg-accent/15 text-accent cursor-default"
							title="Always included"
						>
							TT Loot
						</span>
						{#each Object.keys(config.gainTags) as tag}
							<button
								type="button"
								class="filter-chip {config.gainTags[tag] ? 'is-active' : ''}"
								onclick={() => (config.gainTags[tag] = !config.gainTags[tag])}
							>
								{labelFor(tag)}
							</button>
						{/each}
					</div>
					<div class="flex items-center gap-1.5 flex-wrap">
						<span class="text-[10px] font-semibold uppercase tracking-wider text-text-tertiary w-12 flex-shrink-0">
							Costs
						</span>
						<span
							class="px-2.5 py-1 text-xs font-medium rounded-md bg-accent/15 text-accent cursor-default"
							title="Always included: weapon, healing, enhancers, armour"
						>
							Cycled
						</span>
						{#each Object.keys(config.lossTags) as tag}
							<button
								type="button"
								class="filter-chip {config.lossTags[tag] ? 'is-active' : ''}"
								onclick={() => (config.lossTags[tag] = !config.lossTags[tag])}
							>
								{labelFor(tag)}
							</button>
						{/each}
					</div>
				</div>
				<div class="flex items-center justify-center gap-12">
				<!-- Donut chart + legend -->
				<div class="flex flex-col items-center gap-3 flex-shrink-0">
					<div class="relative">
						<svg
							role="img"
							aria-label="Cost breakdown donut chart"
							viewBox="0 0 120 120"
							class="w-40 h-40"
							onmousemove={(e) => {
								const svg = e.currentTarget;
								const rect = svg.getBoundingClientRect();
								const x = ((e.clientX - rect.left) / rect.width) * 120 - 60;
								const y = ((e.clientY - rect.top) / rect.height) * 120 - 60;
								const dist = Math.sqrt(x * x + y * y);
								if (dist < PIE_R - 7 || dist > PIE_R + 7) { hoveredIdx = -1; return; }
								let angle = Math.atan2(y, x) + Math.PI / 2;
								if (angle < 0) angle += Math.PI * 2;
								const pos = (angle / (Math.PI * 2)) * PIE_C;
								if (!pieView) { hoveredIdx = -1; return; }
								for (let i = 0; i < pieView.arcs.length; i++) {
									const a = pieView.arcs[i];
									if (pos >= a.offset && pos < a.offset + a.length) { hoveredIdx = i; return; }
								}
								hoveredIdx = -1;
							}}
							onmouseleave={() => (hoveredIdx = -1)}
						>
							{#each pieView.arcs as arc, i}
								<circle
									cx="60"
									cy="60"
									r={PIE_R}
									fill="none"
									stroke={arc.color}
									stroke-width={hoveredIdx === i ? 13 : 10}
									stroke-opacity={hoveredIdx >= 0 && hoveredIdx !== i ? 0.35 : 1}
									stroke-dasharray="{arc.length} {PIE_C - arc.length}"
									stroke-dashoffset={-arc.offset}
									transform="rotate(-90 60 60)"
									class="transition-all duration-150"
								/>
							{/each}
						</svg>
						<!-- Center label -->
						<div class="absolute inset-0 flex flex-col items-center justify-center pointer-events-none">
							{#if hoveredIdx >= 0 && pieView.arcs[hoveredIdx]}
								{@const seg = pieView.arcs[hoveredIdx]}
								<span class="text-[10px] font-medium text-text-secondary">{seg.label}</span>
								<span class="text-base font-bold tabular-nums text-text">
									{formatPed(seg.ped)}
								</span>
								<span class="text-[10px] tabular-nums text-text-tertiary">
									{formatPercent(seg.pct)} of costs
								</span>
							{:else}
								<span class="text-xl font-bold tabular-nums text-text">
									{formatPercent(pieView.rate)}
								</span>
								<span class="text-[10px] text-text-tertiary">
									return rate
								</span>
							{/if}
						</div>
					</div>
					<!-- Colour legend -->
					<div class="flex flex-wrap justify-center gap-x-4 gap-y-1">
						{#each pieView.arcs as seg, i}
							<button
								class="flex items-center gap-1.5 text-xs cursor-pointer transition-opacity duration-150
									{hoveredIdx >= 0 && hoveredIdx !== i ? 'opacity-40' : 'opacity-100'}"
								onmouseenter={() => (hoveredIdx = i)}
								onmouseleave={() => (hoveredIdx = -1)}
							>
								<span
									class="w-2 h-2 rounded-full flex-shrink-0"
									style="background: {seg.color}"
								></span>
								<span class="text-text-secondary">{seg.label}</span>
							</button>
						{/each}
					</div>
				</div>

				<!-- Returns / Costs -->
				<div class="flex flex-col gap-4 min-w-0">
					<div class="flex flex-col gap-0.5">
						<span class="text-xs text-text-tertiary font-medium uppercase tracking-wide">
							Returns
						</span>
						<span class="text-2xl font-bold tabular-nums text-text">
							{formatPed(pieView.gains)}
							<span class="text-sm font-normal text-text-tertiary">PED</span>
						</span>
					</div>
					<div class="flex flex-col gap-0.5">
						<span class="text-xs text-text-tertiary font-medium uppercase tracking-wide">
							Costs
						</span>
						<span class="text-2xl font-bold tabular-nums text-text">
							{formatPed(pieView.losses)}
							<span class="text-sm font-normal text-text-tertiary">PED</span>
						</span>
					</div>
					<div>
						<span
							class="text-lg font-semibold tabular-nums {pieView.gains - pieView.losses >= 0
								? 'text-positive'
								: 'text-negative'}"
						>
							{pieView.gains - pieView.losses >= 0 ? '+' : ''}{formatPed(pieView.gains - pieView.losses)} PED
						</span>
					</div>
				</div>
			</div>
			</div>

			<Divider />
		{/if}

		<!-- Cumulative P&L timeline -->
		<div>
			<h3 class="eyebrow mb-3">
				Cumulative P&L
			</h3>
			{#if model.chartPoints.length >= 2}
				{@const chartPoints = model.chartPoints}
				{@const zeroY = model.zeroY}
				<Card class="p-4">
					<div class="h-44">
						<svg viewBox="0 0 800 160" class="w-full h-full" preserveAspectRatio="xMidYMid meet">
							<defs>
								<linearGradient id="plGradientPositive" x1="0" y1="0" x2="0" y2="1">
									<stop offset="0%" stop-color="var(--color-positive)" stop-opacity="0.18" />
									<stop offset="100%" stop-color="var(--color-positive)" stop-opacity="0.02" />
								</linearGradient>
								<linearGradient id="plGradientNegative" x1="0" y1="1" x2="0" y2="0">
									<stop offset="0%" stop-color="var(--color-negative)" stop-opacity="0.18" />
									<stop offset="100%" stop-color="var(--color-negative)" stop-opacity="0.02" />
								</linearGradient>
								<clipPath id="plClipAboveZero">
									<rect x="0" y="0" width="800" height={zeroY} />
								</clipPath>
								<clipPath id="plClipBelowZero">
									<rect x="0" y={zeroY} width="800" height={160 - zeroY} />
								</clipPath>
							</defs>

							<!-- Zero line -->
							<line
								x1="40"
								y1={zeroY}
								x2="760"
								y2={zeroY}
								stroke="var(--color-border-bright)"
								stroke-width="1"
								stroke-dasharray="4 4"
							/>
							<text x="4" y={zeroY + 4} fill="var(--color-text-tertiary)" font-size="10">0</text>

							<!-- Fill area: above-zero in green, below-zero in orange. Single
							     polygon closing at the zero line; rendered twice with
							     opposite clipPaths + opposite gradients. -->
							{#if model.chartFillPath}
								<polygon
									points={model.chartFillPath}
									fill="url(#plGradientPositive)"
									clip-path="url(#plClipAboveZero)"
								/>
								<polygon
									points={model.chartFillPath}
									fill="url(#plGradientNegative)"
									clip-path="url(#plClipBelowZero)"
								/>
							{/if}

							<!-- Line: same trick, rendered once green clipped above, once orange clipped below. -->
							{#if model.chartPath}
								<polyline
									points={model.chartPath}
									fill="none"
									stroke="var(--color-positive)"
									stroke-width="2"
									stroke-linejoin="round"
									stroke-linecap="round"
									clip-path="url(#plClipAboveZero)"
								/>
								<polyline
									points={model.chartPath}
									fill="none"
									stroke="var(--color-negative)"
									stroke-width="2"
									stroke-linejoin="round"
									stroke-linecap="round"
									clip-path="url(#plClipBelowZero)"
								/>
							{/if}

							<!-- Data points: per-point colour by sign. -->
							{#each chartPoints as point}
								<circle
									cx={point.x}
									cy={point.y}
									r="3"
									fill={point.value >= 0 ? 'var(--color-positive)' : 'var(--color-negative)'}
								/>
							{/each}

							<!-- End value label: 12px above last dot (with the y-mapping
							     reserving 18px headroom at the chart top). Colour tracks
							     the sign of the current net. -->
							{#if chartPoints.length > 0}
								{@const last = chartPoints[chartPoints.length - 1]}
								<text
									x={last.x}
									y={last.y - 12}
									fill={last.value >= 0 ? 'var(--color-positive)' : 'var(--color-negative)'}
									font-size="11"
									font-weight="600"
									text-anchor="end"
								>
									{formatPed(last.value)} PED
								</text>
							{/if}

							<!-- Date labels -->
							{#if chartPoints.length > 0}
								<text x="40" y="155" fill="var(--color-text-tertiary)" font-size="10">
									{formatDate(chartPoints[0].date)}
								</text>
								<text
									x="760"
									y="155"
									fill="var(--color-text-tertiary)"
									font-size="10"
									text-anchor="end"
								>
									{formatDate(chartPoints[chartPoints.length - 1].date)}
								</text>
							{/if}
						</svg>
					</div>
				</Card>
			{:else}
				<Card class="p-6">
					<p class="text-sm text-text-tertiary text-center">
						Not enough data for a timeline. Complete more sessions to see your P&L trend.
					</p>
				</Card>
			{/if}
		</div>

		<!-- Cumulative breakdown table (collapsed by default) -->
		{#if data}
			{@const lb = data.lossesBreakdown}
			{@const rb = data.returnsBreakdown}
			{@const cb = lb.cycledBreakdown}
			<!-- The table honours the Global Returns tag toggles, so its totals
			     always reconcile with the donut card above. -->
			{@const totalLedgerGains = Object.entries(rb.ledger).reduce((s, [tag, v]) => s + (PROGRESSION_GAIN_TAGS.has(tag) || !config.gainTags[tag] ? 0 : v), 0)}
			{@const totalLedgerLosses = Object.entries(lb.ledger).reduce((s, [tag, v]) => s + (config.lossTags[tag] ? v : 0), 0)}
			{@const totalReturns = rb.lootTt + totalLedgerGains}
			{@const totalCosts = lb.trackingCost + totalLedgerLosses}
			<div class="mt-2">
				<button
					class="flex items-center gap-1.5 eyebrow mb-3 cursor-pointer hover:text-text transition-colors"
					onclick={() => (model.showBreakdown = !model.showBreakdown)}
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor"
						class="h-3.5 w-3.5 transition-transform duration-150 {model.showBreakdown ? 'rotate-180' : ''}">
						<path fill-rule="evenodd" d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z" clip-rule="evenodd" />
					</svg>
					Cumulative Breakdown
				</button>
				{#if model.showBreakdown}
				<Card class="p-4">
					<table class="w-full text-sm">
						<tbody>
							<!-- Returns section -->
							<tr class="border-b border-border">
								<td class="py-2 text-text font-medium">Returns</td>
								<td class="py-2 text-right tabular-nums text-text font-medium">{formatPed(totalReturns)}</td>
							</tr>
							<tr class="border-b border-border/30">
								<td class="py-1.5 pl-5 text-text-secondary">Loot TT</td>
								<td class="py-1.5 text-right tabular-nums text-text-secondary">{formatPed(rb.lootTt)}</td>
							</tr>
							{#each Object.entries(rb.ledger) as [tag, amount]}
								{#if amount > 0 && !PROGRESSION_GAIN_TAGS.has(tag) && config.gainTags[tag]}
									<tr class="border-b border-border/30">
										<td class="py-1.5 pl-5 text-text-secondary">{labelFor(tag)}</td>
										<td class="py-1.5 text-right tabular-nums text-text-secondary">{formatPed(amount)}</td>
									</tr>
								{/if}
							{/each}

							<!-- Spacer -->
							<tr><td class="py-1.5" colspan="2"></td></tr>

							<!-- Costs section -->
							<tr class="border-b border-border">
								<td class="py-2 text-text font-medium">Costs</td>
								<td class="py-2 text-right tabular-nums text-text font-medium">{formatPed(totalCosts)}</td>
							</tr>
							<tr class="border-b border-border/30">
								<td class="py-1.5 pl-5 text-text-secondary">Cycled</td>
								<td class="py-1.5 text-right tabular-nums text-text-secondary">{formatPed(lb.trackingCost)}</td>
							</tr>
							{#if cb.weapon > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Weapon</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.weapon)}</td>
								</tr>
							{/if}
							{#if cb.healing > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Healing</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.healing)}</td>
								</tr>
							{/if}
							{#if cb.consumables > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Consumables</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.consumables)}</td>
								</tr>
							{/if}
							{#if cb.enhancer > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Enhancers</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.enhancer)}</td>
								</tr>
							{/if}
							{#if cb.armour > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Armour</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.armour)}</td>
								</tr>
							{/if}
							{#if cb.dangling > 0}
								<tr class="border-b border-border/20">
									<td class="py-1 pl-10 text-text-tertiary text-xs">Dangling</td>
									<td class="py-1 text-right tabular-nums text-text-tertiary text-xs">{formatPed(cb.dangling)}</td>
								</tr>
							{/if}
							{#each Object.entries(lb.ledger) as [tag, amount]}
								{#if amount > 0 && config.lossTags[tag]}
									<tr class="border-b border-border/30">
										<td class="py-1.5 pl-5 text-text-secondary">{labelFor(tag)}</td>
										<td class="py-1.5 text-right tabular-nums text-text-secondary">{formatPed(amount)}</td>
									</tr>
								{/if}
							{/each}

							<!-- Net -->
							<tr><td class="py-1" colspan="2"></td></tr>
							<tr class="border-t border-border">
								<td class="py-2 text-text font-semibold">Net</td>
								<td class="py-2 text-right tabular-nums font-semibold {totalReturns - totalCosts >= 0 ? 'text-positive' : 'text-negative'}">
									{totalReturns - totalCosts >= 0 ? '+' : ''}{formatPed(totalReturns - totalCosts)}
								</td>
							</tr>
						</tbody>
					</table>
				</Card>
				{/if}
			</div>
		{/if}

		<Divider />

		<!-- Monthly breakdown -->
		<div>
			<h3 class="eyebrow mb-3">
				Monthly Breakdown
			</h3>
			{#if model.monthlyRows.length === 0}
				<p class="text-sm text-text-tertiary">No monthly data yet.</p>
			{:else}
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border">
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-left">Month</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Costs</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Returns</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Loot Rate</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Global Rate</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">PES</th>
							<th class="py-2 px-3 text-xs font-medium text-text-secondary text-right">Net</th>
						</tr>
					</thead>
					<tbody>
						{#each model.monthlyRows as month}
							<tr
								class="border-b border-border/50 hover:bg-surface-hover/50 transition-colors duration-[var(--duration-fast)]"
							>
								<td class="py-2.5 px-3 text-text font-medium">{month.month}</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
									{formatPed(month.cost)}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
									{formatPed(month.returns)}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text">
									{month.lootRate == null ? '—' : formatPercent(month.lootRate)}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text">
									{month.globalRate == null ? '—' : formatPercent(month.globalRate)}
								</td>
								<td class="py-2.5 px-3 text-right tabular-nums text-text-secondary">
									{formatPed(month.pes)}
								</td>
								<td
									class="py-2.5 px-3 text-right tabular-nums font-medium {month.net >= 0
										? 'text-positive'
										: 'text-negative'}"
								>
									{month.net >= 0 ? '+' : ''}{formatPed(month.net)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>
	</div>
{:else}
	<Card class="p-6">
		<p class="text-sm text-text-tertiary text-center">
			No tracking data yet. Complete a session to see your sustainability overview.
		</p>
	</Card>
{/if}
