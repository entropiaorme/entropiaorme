<script lang="ts">
	// The review of measured sessions: tracked beside measured per category
	// over the whole history (the figures that decide what to trust), then
	// the sessions newest first, each category's tracked figure above its
	// measured one. A row opens the same measure the session's own record
	// shows.
	import { ErrorNotice, Pagination, Skeleton, Toggle } from '$lib/components';
	import Button from '$lib/components/Button.svelte';
	import { exclusionMark } from '$lib/features/sessions/exclusion';
	import { InDevelopmentMark } from '$lib/inDevelopment';
	import { formatDate, formatDuration, NO_DATA } from '$lib/utils/format';
	import {
		CATEGORY_LABELS,
		formatMeasured,
		formatShare,
		formatSigned,
		formatTracked,
		type MeasuredFigure,
		PAIRED,
		trackedFromRow,
	} from './measure';
	import {
		coverageMark,
		includedSentence,
		leftOutSentence,
		noCompleteSentence,
		sessionFigures,
		sessionSeconds,
		summaryPairs,
	} from './measuredReview';
	import type { MeasuredSessionsModel } from './measuredSessionsModel.svelte';
	import SessionMeasure from './SessionMeasure.svelte';

	let { model }: { model: MeasuredSessionsModel } = $props();

	const COLUMN_COUNT = 3 + PAIRED.length;

	function measuredText(figure: MeasuredFigure): string {
		return figure.kind === 'amount' ? formatMeasured(figure.amount) : 'not settled';
	}

	function precisionOf(figure: MeasuredFigure) {
		return figure.kind === 'amount' ? figure.amount.precision : 'wholePec';
	}

	const pairs = $derived(model.summary ? summaryPairs(model.summary) : []);
	const leftOut = $derived(model.summary ? leftOutSentence(model.summary) : null);
	const empty = $derived(!model.loading && model.total === 0 && model.loaded === 0);
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

	<ErrorNotice message={model.error} onDismiss={() => model.dismissError()} />

	{#if model.loading}
		<div class="space-y-3" aria-busy="true">
			<Skeleton class="h-24 w-full" />
			<Skeleton class="h-40 w-full" />
		</div>
	{:else if empty}
		{#if !model.error}
			<p class="py-10 text-center text-sm text-text-tertiary">
				Readings are taken while a session is tracked with the account connected.
			</p>
		{/if}
	{:else}
		<!-- The whole history, per category: the figures that decide what to trust -->
		<section aria-label="Every complete session" data-testid="measured-summary">
			{#if model.summary}
				{@const summary = model.summary}
				{#if summary.included > 0}
					<div class="overflow-x-auto">
						<table class="w-full table-fixed tabular-nums">
							<colgroup>
								<col class="w-48" />
								{#each pairs as pair (pair.category)}<col />{/each}
							</colgroup>
							<thead>
								<tr>
									<th class="pb-2"><span class="sr-only">Figure</span></th>
									{#each pairs as pair (pair.category)}
										<th class="eyebrow pb-2 text-right">{pair.label}</th>
									{/each}
								</tr>
							</thead>
							<tbody>
								<tr>
									<th scope="row" class="py-1 text-left text-xs font-normal text-text-tertiary">
										Tracked
									</th>
									{#each pairs as pair (pair.category)}
										<td class="py-1 text-right text-lg font-medium text-text" data-testid="sum-tracked-{pair.category}">
											{pair.measured ? formatTracked(pair.tracked, pair.measured.precision) : NO_DATA}
										</td>
									{/each}
								</tr>
								<tr>
									<th scope="row" class="py-1 text-left text-xs font-normal text-text-tertiary">
										Measured
									</th>
									{#each pairs as pair (pair.category)}
										<td class="py-1 text-right text-lg font-medium text-text" data-testid="sum-measured-{pair.category}">
											{pair.measured ? formatMeasured(pair.measured) : NO_DATA}
										</td>
									{/each}
								</tr>
								<tr class="border-t border-border/40">
									<th scope="row" class="py-1 text-left text-xs font-normal text-text-tertiary">
										Measured minus tracked
									</th>
									{#each pairs as pair (pair.category)}
										<td
											class="py-1 text-right text-lg font-medium
												{pair.difference && !pair.difference.negligible ? 'text-text' : 'text-text-tertiary'}"
											data-testid="sum-difference-{pair.category}"
										>
											{pair.difference ? formatSigned(pair.difference.amount) : NO_DATA}
										</td>
									{/each}
								</tr>
								<tr>
									<th scope="row" class="py-1 text-left text-xs font-normal text-text-tertiary">
										Share of tracked
									</th>
									{#each pairs as pair (pair.category)}
										<td class="py-1 text-right text-sm text-text-secondary" data-testid="sum-share-{pair.category}">
											{formatShare(pair.share)}
										</td>
									{/each}
								</tr>
								<tr>
									<th scope="row" class="py-1 text-left text-xs font-normal text-text-tertiary">
										Sessions
									</th>
									{#each pairs as pair (pair.category)}
										<td class="py-1 text-right text-xs text-text-tertiary">{pair.sessions}</td>
									{/each}
								</tr>
							</tbody>
						</table>
					</div>
					<p class="mt-4 text-sm text-text-secondary" data-testid="sum-other">
						Other, measured only: <span class="tabular-nums text-text">{formatMeasured(summary.other.measured)}</span>
						PED over {summary.other.sessions}
						{summary.other.sessions === 1 ? 'session' : 'sessions'}
					</p>
				{/if}
				<p class="mt-3 text-xs text-text-tertiary">{includedSentence(summary)}</p>
				{#if leftOut}
					<p class="mt-1 text-xs text-text-tertiary" data-testid="sum-left-out">{leftOut}</p>
				{/if}
			{:else if model.summaryError}
				<ErrorNotice message={model.summaryError} />
			{:else}
				<Skeleton class="h-24 w-full" />
			{/if}
		</section>

		{#if model.listed.length === 0}
			<p class="py-6 text-center text-sm text-text-tertiary">
				{noCompleteSentence(model.loaded, model.hasOlder)}
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
								{@const regionId = `measured-session-${row.id}`}
								<tr
									class="border-b border-border/30 align-top transition-colors
										duration-[var(--duration-fast)] hover:bg-surface-hover/40
										{expanded ? 'bg-surface-hover/30' : ''}"
									data-testid="measured-row"
								>
									<td class="px-3 py-2.5">
										<button
											type="button"
											class="block w-full min-w-0 cursor-pointer text-left focus-visible:outline-none
												focus-visible:ring-1 focus-visible:ring-accent rounded-sm"
											aria-expanded={expanded}
											aria-controls={expanded ? regionId : undefined}
											onclick={() => model.toggle(row.id)}
										>
											<span
												class="block truncate {row.name ? 'text-text' : 'italic text-text-tertiary'}"
												title={row.name ?? undefined}
											>
												{row.name ?? 'Unnamed'}
											</span>
										</button>
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
										<td class="px-3 py-2.5 text-right tabular-nums" data-testid="cell-{category}">
											<p class="text-text-secondary">
												{formatTracked(tracked, precisionOf(measured))}
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
											<div id={regionId}>
												<SessionMeasure
													sessionId={row.id}
													tracked={trackedFromRow(row.tracked)}
													ended={row.endTime !== null}
													framed={false}
												/>
											</div>
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
					loading={model.loadingOlder}
					disabled={model.loadingOlder}
					onclick={() => model.loadOlder()}
				>
					Load older sessions
				</Button>
			</div>
		{/if}
	{/if}
</div>
