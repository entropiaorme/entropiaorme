<script lang="ts">
	// What one cost bucket's figure rests on, set after the figure: in a
	// session that mixes bases, whether it came from the account or was
	// tracked, and on a bucket the readings did not measure in a session
	// that has readings, that it was tracked (its meaning, and why the
	// readings could not measure it, on hover or focus); and, where the readings
	// measured it, what the app had tracked when that differs. A note that
	// qualifies the tracked estimate (`estimateNote`) follows it, and shows
	// only with it.
	import type { Snippet } from 'svelte';
	import type { CostBasis, CostBreakdown } from '$lib/api/commands.gen';
	import InfoTip from '$lib/components/InfoTip.svelte';
	import { formatPed } from '$lib/utils/format';
	import {
		BASIS_MARKS,
		bucketMark,
		bucketReason,
		type CostBucket,
		trackedEstimate,
	} from './costBasis';

	let {
		breakdown,
		sessionBasis,
		bucket,
		estimateNote,
	}: {
		breakdown: CostBreakdown;
		sessionBasis: CostBasis;
		bucket: CostBucket;
		estimateNote?: Snippet;
	} = $props();

	const mark = $derived(
		bucketMark(sessionBasis, breakdown.basis[bucket], breakdown.reasons[bucket]),
	);
	const reason = $derived(bucketReason(breakdown, bucket));
	const estimate = $derived(trackedEstimate(breakdown, bucket));
</script>

{#if mark}
	{' '}<span class="text-text-tertiary" data-testid="basis-{bucket}"
		><InfoTip label={BASIS_MARKS[mark].label} align="left" width="w-64">
			{#snippet trigger()}<span class="underline decoration-dotted underline-offset-2 hover:text-text-secondary"
					>{BASIS_MARKS[mark].label}</span
				>{/snippet}
			<span class="block text-xs leading-relaxed text-text-secondary">{BASIS_MARKS[mark].meaning}</span>
			{#if reason}<span class="mt-1.5 block text-xs leading-relaxed text-text-secondary" data-testid="reason-{bucket}"
					>{reason}</span
				>{/if}
		</InfoTip></span
	>{/if}{#if estimate !== null}<span class="text-text-tertiary" data-testid="estimate-{bucket}"
		>; tracked estimate was <span class="tabular-nums">{formatPed(estimate)}</span></span
	>{@render estimateNote?.()}{/if}
