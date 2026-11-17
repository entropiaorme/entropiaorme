<script lang="ts">
	// A session's measure: how well the readings cover it, then where its
	// measured costs came from, item by item, collapsed until asked. Given
	// the session's tracked figures (the review of measured sessions), it
	// also compares them category by category, so the player can see how
	// far the chat log's estimate was off; a measure of part of a session
	// is never differenced against the whole session's figures. Nothing
	// here sums a cost, a net or a return of its own.
	//
	// Given the session's measured cost lines (its own detail), the items
	// are listed by the bucket each was billed to, as the session's figures
	// are; without them (the review) they are listed as the readings saw
	// them: ammo, wear by role, and units used up.
	import type { AccountSessionMeasure, MeasuredCostDetail } from '$lib/api/commands.gen';
	import { DataTable } from '$lib/components';
	import { NO_DATA } from '$lib/utils/format';
	import {
		armourAside,
		comparisonRows,
		coversWhole,
		describeCoverage,
		formatMeasured,
		formatSigned,
		formatTracked,
		lootAsides,
		type TrackedFigures,
		toppedUpAmmoAsides,
		unsettledAmmoAsides,
	} from './measure';
	import {
		ammoLines,
		consumptionLines,
		eventLines,
		hasDetails,
		listsWholePec,
		measureClock,
		repairLines,
		wearGroups,
	} from './measureDetails';
	import { costsListWholePec, cycledGroups } from './costLines';
	import type { CostPlacementsModel } from './costPlacementsModel.svelte';
	import MeasuredCostGroups from './MeasuredCostGroups.svelte';

	let {
		measure,
		tracked = null,
		ended = null,
		costs = null,
		acrossGapMinutes = null,
		placements = null,
	}: {
		measure: AccountSessionMeasure;
		/** The session's tracked figures to compare with; `null` for no
		 * comparison. */
		tracked?: TrackedFigures | null;
		/** Whether the session itself has ended; `null` when not known. */
		ended?: boolean | null;
		/** The session's measured cost lines, by bucket; `null` to list the
		 * items as the readings saw them. */
		costs?: MeasuredCostDetail | null;
		/** How long a gap the session's costs were measured across, in
		 * minutes; `null` when they were not. */
		acrossGapMinutes?: number | null;
		/** The placements, so a placed item shows where it was placed. */
		placements?: CostPlacementsModel | null;
	} = $props();

	const coverage = $derived(
		describeCoverage(measure.coverage, measure.sealed, ended, acrossGapMinutes),
	);
	const whole = $derived(coversWhole(measure));
	const rows = $derived(
		(tracked ? comparisonRows(measure.totals, tracked, whole) : []).map((row) => ({
			...row,
			asides:
				row.category === 'weapon'
					? [...unsettledAmmoAsides(measure), ...toppedUpAmmoAsides(measure)]
					: row.category === 'armour'
						? [armourAside(row.tracked, row.measured)].filter((line) => line !== null)
						: row.category === 'loot'
							? lootAsides(measure)
							: [],
		})),
	);

	let open = $state(false);
	const detailsId = `measure-details-${crypto.randomUUID()}`;
	const clock = $derived(measureClock(measure));
	const ammo = $derived(ammoLines(measure.ammo, measure.events));
	const wear = $derived(wearGroups(measure.decay));
	const repairs = $derived(repairLines(measure.repairs, clock));
	const consumed = $derived(consumptionLines(measure.otherConsumption));
	const departed = $derived(consumptionLines(measure.unexplainedDepartures));
	const events = $derived(eventLines(measure.events, clock));
	// Whether the figures this view lists rest on values the site rounds
	// down: by bucket, Cycled's lines and what is listed beside them.
	const roundedDown = $derived(
		costs
			? costsListWholePec(costs) ||
					measure.repairs.some((repair) => repair.restored.precision === 'wholePec') ||
					measure.unexplainedDepartures.some((line) => line.amount.precision === 'wholePec')
			: listsWholePec(measure),
	);
	const anyDetails = $derived(
		costs
			? cycledGroups(costs).length > 0 ||
					costs.armourParts.length > 0 ||
					repairs.length > 0 ||
					departed.length > 0 ||
					events.length > 0
			: hasDetails(measure),
	);
	// An item just placed in one of Cycled's buckets is listed here, so the
	// list opens to show where it went.
	// Once per placement: closing the list afterwards keeps it closed.
	let shownPlacement = 0;
	$effect(() => {
		const placed = placements?.lastPlacement;
		if (!placed || !costs || placed.seq === shownPlacement) return;
		const listed = cycledGroups(costs).some((group) =>
			group.lines.some((line) => line.item === placed.item),
		);
		if (listed) {
			shownPlacement = placed.seq;
			open = true;
		}
	});

	const PRECISION_TITLE = {
		exact: 'Exact',
		wholePec: 'Read from values the site rounds down to the PEC',
	};
</script>

<div class="space-y-3" data-testid="measure-block">
	<p
		class="text-xs {coverage.state === 'complete' ? 'text-text-secondary' : 'text-warning'}"
		data-testid="measure-coverage"
	>
		{coverage.text}
	</p>

	{#if tracked}
		<DataTable
			fixedLayout
			columns={[
				{ key: 'label', label: 'Category' },
				{ key: 'tracked', label: 'Tracked estimate', align: 'right', widthClass: 'w-32' },
				{
					key: 'measured',
					label: whole ? 'Measured' : 'Measured, part of the session',
					align: 'right',
					widthClass: 'w-48',
				},
				{ key: 'difference', label: 'Difference', align: 'right', widthClass: 'w-28' },
			]}
			{rows}
		>
			{#snippet cell({ row, column })}
				{#if column.key === 'label'}
					<span class="text-text">{row.label}</span>
				{:else if column.key === 'tracked'}
					<span class="text-text-secondary">
						{formatTracked(
							row.tracked,
							row.measured.kind === 'amount' ? row.measured.amount.precision : 'wholePec',
						)}
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
					{#each row.asides as aside, index (`${index}-${aside}`)}
						<span class="mt-0.5 block text-[11px] leading-snug text-text-tertiary">{aside}</span>
					{/each}
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
	{/if}

	{#if anyDetails}
		<div>
			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-sm text-xs text-text-tertiary hover:text-text
					transition-colors duration-[var(--duration-fast)]
					focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-accent"
				aria-expanded={open}
				aria-controls={open ? detailsId : undefined}
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
				Item by item
			</button>

			{#if open}
				<div id={detailsId} class="mt-3 space-y-5 text-xs" data-testid="measure-details">
					{#if costs}
						<MeasuredCostGroups {costs} {placements} />
					{:else}
						{#if ammo.length > 0}
							<div>
								<h4 class="eyebrow mb-1.5">Ammo</h4>
								{#each ammo as line, index (`${index}-${line.name}`)}
									<div class="flex items-baseline justify-between gap-4 py-0.5">
										<span class="min-w-0 break-words text-text-secondary">
											<span class="text-text">{line.name}</span>: {line.parts.join('; ')}
										</span>
										<span
											class="shrink-0 tabular-nums text-text"
											title={line.burned ? PRECISION_TITLE[line.burned.precision] : undefined}
										>
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
											{#each group.items as item, index (`${index}-${item.name}`)}
												<div class="flex items-baseline justify-between gap-4 py-0.5">
													<span class="min-w-0 truncate text-text-secondary" title={item.name}>{item.name}</span>
													<span class="shrink-0 tabular-nums text-text" title={PRECISION_TITLE[item.amount.precision]}>{formatMeasured(item.amount)}</span>
												</div>
											{/each}
										</div>
									{/each}
								</div>
							</div>
						{/if}

						{#if consumed.length > 0}
							<div>
								<h4 class="eyebrow mb-1.5">Used up</h4>
								{#each consumed as line, index (`${index}-${line.name}`)}
									<div class="flex items-baseline justify-between gap-4 py-0.5">
										<span class="min-w-0 truncate text-text-secondary" title={line.name}>
											{line.name} <span class="text-text-tertiary">{line.detail}</span>
										</span>
										<span class="shrink-0 tabular-nums text-text" title={PRECISION_TITLE[line.amount.precision]}>{formatMeasured(line.amount)}</span>
									</div>
								{/each}
							</div>
						{/if}
					{/if}

					{#if repairs.length > 0}
						<div>
							<h4 class="eyebrow mb-1">Repairs</h4>
							<p class="mb-1.5 text-text-tertiary">
								A repair settles wear already counted, so it is not a cost.
							</p>
							{#each repairs as repair, index (`${index}-${repair.what}`)}
								<div class="flex items-baseline justify-between gap-4 py-0.5">
									<span class="min-w-0 truncate text-text-secondary" title={repair.what}>
										<span class="tabular-nums text-text-tertiary">{repair.at}</span>
										{repair.what}{#if repair.paid}<span class="text-text-tertiary">; {repair.paid}</span>{/if}
									</span>
									<span class="shrink-0 tabular-nums text-text-secondary">restored {repair.restored}</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if departed.length > 0}
						<div>
							<h4 class="eyebrow mb-1.5">Left with nothing to explain it</h4>
							{#each departed as line, index (`${index}-${line.name}`)}
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
							{#each events as event, index (`${index}-${event.text}`)}
								<div class="flex items-baseline gap-3 py-0.5">
									<span class="shrink-0 tabular-nums text-text-tertiary">{event.at}</span>
									<span class="min-w-0 break-words text-text-secondary">{event.text}</span>
								</div>
							{/each}
						</div>
					{/if}

					{#if roundedDown}
						<p class="text-text-tertiary" data-testid="measure-rounded-note">
							Figures to two decimals rest on values the site rounds down to the PEC, so each may be up to a PEC out.
						</p>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</div>
