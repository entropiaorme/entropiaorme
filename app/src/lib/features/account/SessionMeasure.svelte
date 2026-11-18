<script lang="ts">
	// One session's measure, read for the session shown and re-read on each
	// account frame.
	//
	// In the session detail (`framed`) it is a review block saying where the
	// session's measured costs came from, and it appears only once the
	// session is known to have a measure: a session without readings, a
	// first read still on its way, or a first read that failed all render
	// nothing, so the detail of a session the account never read looks
	// exactly as it did. In the review of measured sessions, where the
	// session is known to have readings, it also compares the session's
	// tracked estimates with the measure, and shows its loading and error
	// states in place. In the detail, the session is also read minute by
	// minute below its measure.
	import { onMount } from 'svelte';
	import { ErrorNotice, Skeleton } from '$lib/components';
	import SessionReviewBlock from '$lib/features/tracking/SessionReviewBlock.svelte';
	import type { CostBasis, MeasuredCostDetail } from '$lib/api/commands.gen';
	import { restsOnReadings } from './costBasis';
	import type { CostPlacementsModel } from './costPlacementsModel.svelte';
	import type { TrackedFigures } from './measure';
	import MeasureBlock from './MeasureBlock.svelte';
	import type { MinuteLaneKind } from './minutes';
	import SessionMinutes from './SessionMinutes.svelte';
	import { createSessionMeasureModel } from './sessionMeasureModel.svelte';

	let {
		sessionId,
		tracked = null,
		costBasis = null,
		ended = null,
		framed = true,
		costs = null,
		acrossGapMinutes = null,
		placements = null,
		onremoveloot = null,
		reviews = null,
		minutesRefreshKey = 0,
	}: {
		sessionId: string;
		/** What the session's costs rest on, as the detail resolved them;
		 * the block claims to be where the costs came from only when they
		 * rest on the readings. */
		costBasis?: CostBasis | null;
		/** The session's tracked figures to compare with; `null` for no
		 * comparison. */
		tracked?: TrackedFigures | null;
		/** Whether the session itself has ended; `null` when not known. */
		ended?: boolean | null;
		framed?: boolean;
		/** The session's measured cost lines, by bucket: in the session's
		 * own detail, the items are listed as its figures bill them. */
		costs?: MeasuredCostDetail | null;
		/** How long a gap the session's costs were measured across, in
		 * minutes; `null` when they were not. */
		acrossGapMinutes?: number | null;
		/** The placements, so a placed item shows where it was placed. */
		placements?: CostPlacementsModel | null;
		/** Take every line of an item out of the session's loot, for loot
		 * the readings never saw arrive; `null` where it is not offered. */
		onremoveloot?: ((itemName: string) => Promise<void>) | null;
		/** Show the session's own review of a lane, where it has one. */
		reviews?: Partial<Record<MinuteLaneKind, (() => void) | null>> | null;
		/** Changes when the session's loot changed elsewhere in its detail,
		 * so its minutes are read again. */
		minutesRefreshKey?: number;
	} = $props();

	const model = createSessionMeasureModel(() => sessionId);
	onMount(() => model.connect());
	$effect(() => {
		void sessionId;
		void model.refresh();
	});
</script>

{#if framed}
	{#if model.measure}
		<SessionReviewBlock
			id="account-measure"
			heading={costBasis && restsOnReadings(costBasis) ? 'Costs from your account' : 'Your account readings'}
			mark="account-readings"
			error={model.error}
			onDismissError={() => model.dismissError()}
			notice={null}
		>
			<MeasureBlock
				measure={model.measure}
				{tracked}
				{ended}
				{costs}
				{acrossGapMinutes}
				{placements}
			/>
			<SessionMinutes {sessionId} {onremoveloot} {reviews} refreshKey={minutesRefreshKey} />
		</SessionReviewBlock>
	{/if}
{:else if model.measure}
	<ErrorNotice message={model.error} onDismiss={() => model.dismissError()} class="mb-3" />
	<MeasureBlock measure={model.measure} {tracked} {ended} />
{:else if model.error}
	<ErrorNotice message={model.error} />
{:else if model.answered}
	<p class="text-xs text-text-tertiary">No readings for this session.</p>
{:else}
	<div class="space-y-2" aria-busy="true">
		<Skeleton class="h-3 w-56" />
		<Skeleton class="h-24 w-full" />
	</div>
{/if}
