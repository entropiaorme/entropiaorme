<script lang="ts">
	// A session's measure beside its tracked figures: how well the readings
	// cover it, the comparison by category, and the details behind it,
	// collapsed. Measured figures stand beside the tracked ones; nothing
	// here sums them into a cost, a net or a return.
	import type { AccountSessionMeasure } from '$lib/api/commands.gen';
	import { DataTable } from '$lib/components';
	import { formatPed, NO_DATA } from '$lib/utils/format';
	import {
		armourAside,
		comparisonRows,
		describeCoverage,
		formatMeasured,
		formatSigned,
		lootAsides,
		measureTotals,
		type TrackedFigures,
		unsettledAmmoAsides,
	} from './measure';
	import {
		ammoLines,
		consumptionLines,
		eventLines,
		hasDetails,
		repairLines,
		wearGroups,
	} from './measureDetails';

	let {
		measure,
		tracked,
	}: {
		measure: AccountSessionMeasure;
		tracked: TrackedFigures;
	} = $props();

	const coverage = $derived(describeCoverage(measure.coverage, measure.sealed));
	const rows = $derived(
		comparisonRows(measureTotals(measure), tracked).map((row) => ({
			...row,
			asides:
				row.category === 'weapon'
					? unsettledAmmoAsides(measure)
					: row.category === 'armour'
						? [armourAside(row.tracked, row.measured)].filter((line) => line !== null)
						: row.category === 'loot'
							? lootAsides(measure)
							: [],
		})),
	);

	let open = $state(false);
	const detailsId = `measure-details-${crypto.randomUUID()}`;
	const ammo = $derived(ammoLines(measure.ammo));
	const wear = $derived(wearGroups(measure.decay));
	const repairs = $derived(repairLines(measure.repairs));
	const consumed = $derived(consumptionLines(measure.otherConsumption));
	const departed = $derived(consumptionLines(measure.unexplainedDepartures));
	const events = $derived(eventLines(measure.events));

	const PRECISION_TITLE = {
		exact: 'Exact',
		wholePec: 'Read from values the site rounds down to the PEC',
	};
</script>

<div class="space-y-3" data-testid="measure-block">
	<p
		class="text-xs {coverage.state === 'partial' ? 'text-warning' : 'text-text-secondary'}"
		data-testid="measure-coverage"
	>
		{coverage.text}
	</p>

	<DataTable
		fixedLayout
		columns={[
			{ key: 'label', label: 'Category' },
			{ key: 'tracked', label: 'Tracked', align: 'right', widthClass: 'w-28' },
			{ key: 'measured', label: 'Measured', align: 'right', widthClass: 'w-28' },
			{ key: 'difference', label: 'Difference', align: 'right', widthClass: 'w-28' },
		]}
		{rows}
	>
		{#snippet cell({ row, column })}
			{#if column.key === 'label'}
				<span class="text-text">{row.label}</span>
				{#each row.asides as aside (aside)}
					<span class="mt-0.5 block text-[11px] leading-snug text-text-tertiary">{aside}</span>
				{/each}
			{:else if column.key === 'tracked'}
				<span class="text-text-secondary">
					{row.tracked === null ? NO_DATA : formatPed(row.tracked)}
				</span>
			{:else if column.key === 'measured'}
				{#if row.measured.kind === 'amount'}
					<span class="text-text" title={PRECISION_TITLE[row.measured.amount.precision]}>
						{formatMeasured(row.measured.amount)}
					</span>
				{:else}
					<span class="text-text-tertiary" title="The ammo burned could not be settled">
						not settled
					</span>
				{/if}
			{:else if column.key === 'difference'}
				{#if row.difference}
					<span class={row.difference.negligible ? 'text-text-tertiary' : 'text-text'}>
						{formatSigned(row.difference.amount)}
					</span>
				{:else}
					<span class="text-text-tertiary">{NO_DATA}</span>
				{/if}
			{/if}
		{/snippet}
	</DataTable>

	{#if hasDetails(measure)}
		<div>
			<button
				type="button"
				class="inline-flex items-center gap-1 text-xs text-text-tertiary hover:text-text
					transition-colors duration-[var(--duration-fast)]"
				aria-expanded={open}
				aria-controls={detailsId}
				onclick={() => (open = !open)}
			>
				<svg
					viewBox="0 0 20 20"
					fill="currentColor"
					class="h-3 w-3 transition-transform duration-[var(--duration-fast)] {open ? 'rotate-90' : ''}"
					aria-hidden="true"
				>
					<path
						fill-rule="evenodd"
						d="M7.21 14.77a.75.75 0 01.02-1.06L11.17 10 7.23 6.29a.75.75 0 111.04-1.08l4.5 4.25a.75.75 0 010 1.08l-4.5 4.25a.75.75 0 01-1.06-.02z"
						clip-rule="evenodd"
					/>
				</svg>
				Details
			</button>

			{#if open}
				<div id={detailsId} class="mt-3 space-y-5 text-xs" data-testid="measure-details">
					{#if ammo.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Ammo</h4>
							{#each ammo as line (line.name)}
								<div class="flex items-baseline justify-between gap-4 py-0.5">
									<span class="min-w-0 break-words text-text-secondary">
										<span class="text-text">{line.name}</span>: {line.parts.join('; ')}
									</span>
									<span class="shrink-0 tabular-nums text-text">
										{line.burned ? formatMeasured(line.burned) : NO_DATA}
									</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if wear.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Wear</h4>
							<div class="space-y-2">
								{#each wear as group (group.role)}
									<div>
										<p class="text-text-tertiary">{group.label}</p>
										{#each group.items as item, index (`${item.name}-${index}`)}
											<div class="flex items-baseline justify-between gap-4 py-0.5">
												<span class="min-w-0 truncate text-text-secondary" title={item.name}>{item.name}</span>
												<span class="shrink-0 tabular-nums text-text">{formatMeasured(item.amount)}</span>
											</div>
										{/each}
									</div>
								{/each}
							</div>
						</div>
					{/if}

					{#if repairs.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Repairs</h4>
							{#each repairs as repair, index (index)}
								<div class="flex items-baseline justify-between gap-4 py-0.5">
									<span class="min-w-0 truncate text-text-secondary" title="{repair.what}: restored {repair.restored}">
										<span class="tabular-nums text-text-tertiary">{repair.at}</span>
										{repair.what}: restored {repair.restored}
									</span>
									<span class="shrink-0 tabular-nums {repair.cost === 'cost unknown' ? 'text-text-tertiary' : 'text-text'}">
										{repair.cost}
									</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if consumed.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Other consumption</h4>
							{#each consumed as line, index (`${line.name}-${index}`)}
								<div class="flex items-baseline justify-between gap-4 py-0.5">
									<span class="min-w-0 truncate text-text-secondary" title={line.name}>
										{line.name} <span class="text-text-tertiary">{line.detail}</span>
									</span>
									<span class="shrink-0 tabular-nums text-text">{formatMeasured(line.amount)}</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if departed.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Left with nothing to explain it</h4>
							{#each departed as line, index (`${line.name}-${index}`)}
								<div class="flex items-baseline justify-between gap-4 py-0.5">
									<span class="min-w-0 truncate text-text-secondary" title={line.name}>
										{line.name} <span class="text-text-tertiary">{line.detail}</span>
									</span>
									<span class="shrink-0 tabular-nums text-text-secondary">{formatMeasured(line.amount)}</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if events.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Events</h4>
							{#each events as event, index (index)}
								<div class="flex items-baseline gap-3 py-0.5">
									<span class="shrink-0 tabular-nums text-text-tertiary">{event.at}</span>
									<span class="min-w-0 break-words text-text-secondary">{event.text}</span>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</div>
