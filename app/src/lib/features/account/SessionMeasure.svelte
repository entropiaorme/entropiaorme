<script lang="ts">
	// One session's measure, read for the session shown and re-read on each
	// account frame.
	//
	// In the session detail (`framed`) it is a review block that appears
	// only once the session is known to have a measure: a session without
	// readings, a first read still on its way, or a first read that failed
	// all render nothing, so the detail of a session the account never read
	// looks exactly as it did. In the review of measured sessions, where the
	// session is known to have readings, it shows its loading and error
	// states in place.
	import { onMount } from 'svelte';
	import { ErrorNotice, Skeleton } from '$lib/components';
	import SessionReviewBlock from '$lib/features/tracking/SessionReviewBlock.svelte';
	import type { TrackedFigures } from './measure';
	import MeasureBlock from './MeasureBlock.svelte';
	import { createSessionMeasureModel } from './sessionMeasureModel.svelte';

	let {
		sessionId,
		tracked,
		ended = null,
		framed = true,
	}: {
		sessionId: string;
		tracked: TrackedFigures;
		/** Whether the session itself has ended; `null` when not known. */
		ended?: boolean | null;
		framed?: boolean;
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
			heading="Measured from the account"
			mark="account-readings"
			error={model.error}
			onDismissError={() => model.dismissError()}
			notice={null}
		>
			<MeasureBlock measure={model.measure} {tracked} {ended} />
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
