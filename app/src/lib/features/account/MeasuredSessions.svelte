<script lang="ts">
	// The review of measured sessions: the pair summed per category over the
	// complete sessions, given the page's weight, then the sessions newest
	// first, each category's tracked figure above its measured one. A row
	// opens the same measure the session's own record shows.
	import { ErrorNotice, Pagination, Skeleton, StatDisplay, Toggle } from '$lib/components';
	import Button from '$lib/components/Button.svelte';
	import { exclusionMark } from '$lib/features/sessions/exclusion';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { formatDate, formatDuration, formatPed, NO_DATA } from '$lib/utils/format';
	import {
		CATEGORY_LABELS,
		formatMeasured,
		formatShare,
		formatSigned,
		type MeasuredFigure,
		PAIRED,
		trackedFromRow,
	} from './measure';
	import { coverageMark, sessionFigures, sessionSeconds } from './measuredReview';
	import type { MeasuredSessionsModel } from './measuredSessionsModel.svelte';
	import SessionMeasure from './SessionMeasure.svelte';

	let { model }: { model: MeasuredSessionsModel } = $props();

	const COLUMN_COUNT = 3 + PAIRED.length;

	function measuredText(figure: MeasuredFigure): string {
		return figure.kind === 'amount' ? formatMeasured(figure.amount) : 'not settled';
	}

	const summed = $derived(model.summary);
</script>

<div class="space-y-8" data-testid="analytics-measured-area">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div class="flex flex-wrap items-center gap-2">
			<p class="text-sm text-text-secondary">
				Tracked figures beside what the account site's readings measured, session by session.
			</p>
			<InDevelopmentMark id="account-readings" align="left" />
		</div>
		<div class="flex items-center gap-2.5">
			<span class="text-xs text-text-secondary">Complete readings only</span>
			<Toggle
				checked={model.completeOnly}
				onchange={(checked) => (model.completeOnly = checked)}
				label="Complete readings only"
			/>
		</div>
	</div>

	<ErrorNotice message={model.error} onDismiss={() => (model.error = null)} />

	{#if model.loading}
		<div class="space-y-3" aria-busy="true">
			<Skeleton class="h-10 w-full" />
			<Skeleton class="h-40 w-full" />
		</div>
	{:else if model.total === 0 && model.loaded === 0}
		{#if !model.error}
			<p class="py-10 text-center text-sm text-text-tertiary">
				Readings are taken while a session is tracked with the account connected.
			</p>
		{/if}
	{:else}
		<!-- The sum per category: the figure that decides what to trust -->
		<section aria-label="Summed over complete sessions" data-testid="measured-summary">
			{#if summed.sessions === 0}
				<p class="text-sm text-text-tertiary">No complete session to sum yet.</p>
			{:else}
				<div class="grid grid-cols-2 gap-x-8 gap-y-6 sm:grid-cols-5">
					{#each summed.categories as sum (sum.category)}
						<div class="min-w-0" data-testid="measured-sum-{sum.category}">
							{#if sum.category === 'other'}
								<StatDisplay
									label={CATEGORY_LABELS[sum.category]}
									value={formatMeasured(sum.measured)}
									unit="PED"
									comparison="measured only"
								/>
							{:else if sum.difference && sum.tracked !== null}
								<StatDisplay
									label={CATEGORY_LABELS[sum.category]}
									value={formatSigned(sum.difference.amount)}
									unit="PED"
									valueClass={sum.difference.negligible ? 'text-text-secondary' : 'text-text'}
									comparison="{formatShare(sum.share)} of tracked"
								/>
								<p class="mt-1.5 text-xs tabular-nums text-text-tertiary">
									{formatPed(sum.tracked)} tracked, {formatMeasured(sum.measured)} measured
								</p>
							{:else}
								<StatDisplay label={CATEGORY_LABELS[sum.category]} value={NO_DATA} />
							{/if}
							{#if sum.sessions !== summed.sessions}
								<p class="mt-1 text-[11px] text-text-tertiary">
									{sum.sessions} of {summed.sessions} sessions
								</p>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
			<p class="mt-4 text-xs text-text-tertiary">
				Difference is measured minus tracked, summed over {summed.sessions} complete
				{summed.sessions === 1 ? 'session' : 'sessions'} in the PED stats{model.hasOlder
					? `, among the newest ${model.loaded} with readings`
					: ''}.
			</p>
		</section>

		{#if model.listed.length === 0}
			<p class="py-6 text-center text-sm text-text-tertiary">
				No session's readings are complete yet.
			</p>
		{:else}
			<div>
				<p class="mb-2 text-right text-[11px] text-text-tertiary">
					Each pair: tracked above, measured below
				</p>
				<div class="overflow-x-auto">
					<table class="w-full table-fixed text-sm">
						<colgroup>
							<col />
							<col class="w-20" />
							{#each PAIRED as category (category)}<col class="w-24" />{/each}
							<col class="w-24" />
						</colgroup>
						<thead>
							<tr class="border-b border-border/70">
								<th class="eyebrow px-3 py-2.5 text-left">Session</th>
								<th class="eyebrow px-3 py-2.5 text-left">Duration</th>
								{#each PAIRED as category (category)}
									<th class="eyebrow px-3 py-2.5 text-right">{CATEGORY_LABELS[category]}</th>
								{/each}
								<th class="eyebrow px-3 py-2.5 text-right">Other</th>
							</tr>
						</thead>
						<tbody>
							{#each model.table.pageRows as row (row.id)}
								{@const figures = sessionFigures(row)}
								{@const expanded = model.expandedId === row.id}
								{@const mark = coverageMark(row)}
								{@const excluded = exclusionMark(row.exclusion)}
								{@const seconds = sessionSeconds(row)}
								<tr
									class="cursor-pointer border-b border-border/30 align-top transition-colors
										duration-[var(--duration-fast)] hover:bg-surface-hover/40
										focus-visible:outline-none focus-visible:bg-surface-hover/40
										{expanded ? 'bg-surface-hover/30' : ''}"
									tabindex="0"
									aria-expanded={expanded}
									data-testid="measured-row"
									onclick={() => model.toggle(row.id)}
									onkeydown={(event) => {
										if (event.key === 'Enter' || event.key === ' ') {
											event.preventDefault();
											model.toggle(row.id);
										}
									}}
								>
									<td class="px-3 py-2.5">
										<p
											class="truncate {row.name ? 'text-text' : 'italic text-text-tertiary'}"
											title={row.name ?? undefined}
										>
											{row.name ?? 'Unnamed'}
										</p>
										<p class="flex flex-wrap gap-x-2 text-xs text-text-tertiary">
											<span class="tabular-nums">{formatDate(row.startTime)}</span>
											{#if mark}
												<span class={mark === 'Partial' ? 'text-warning' : ''}>{mark}</span>
											{/if}
											{#if excluded}
												<span title={row.exclusion?.note ?? undefined} data-testid="exclusion-mark">
													{excluded}
												</span>
											{/if}
										</p>
									</td>
									<td class="px-3 py-2.5 tabular-nums text-text-secondary">
										{seconds === null ? NO_DATA : formatDuration(seconds)}
									</td>
									{#each PAIRED as category (category)}
										{@const tracked = figures.tracked[category]}
										{@const measured = figures.measured[category]}
										<td class="px-3 py-2.5 text-right tabular-nums">
											<p class="text-text-secondary">
												{tracked === null ? NO_DATA : formatPed(tracked)}
											</p>
											<p class={measured.kind === 'amount' ? 'text-text' : 'text-xs text-text-tertiary'}>
												{measuredText(measured)}
											</p>
										</td>
									{/each}
									<td class="px-3 py-2.5 text-right tabular-nums">
										<p class="text-text-tertiary">{NO_DATA}</p>
										<p class="text-text">{measuredText(figures.measured.other)}</p>
									</td>
								</tr>
								{#if expanded}
									<tr class="border-b border-border/30">
										<td colspan={COLUMN_COUNT} class="px-3 pb-6 pt-3">
											<SessionMeasure
												sessionId={row.id}
												tracked={trackedFromRow(row.tracked)}
												framed={false}
											/>
										</td>
									</tr>
								{/if}
							{/each}
						</tbody>
					</table>
				</div>

				<Pagination
					bind:page={model.table.page}
					totalPages={model.table.totalPages}
					rangeLabel={model.table.rangeLabel}
				/>
			</div>
		{/if}

		{#if model.hasOlder}
			<div class="flex justify-center">
				<Button
					variant="ghost"
					size="sm"
					loading={model.loadingMore}
					disabled={model.loadingMore}
					onclick={() => model.loadOlder()}
				>
					Load older sessions
				</Button>
			</div>
		{/if}
	{/if}
</div>
