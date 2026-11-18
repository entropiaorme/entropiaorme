<script lang="ts">
	import Badge from '$lib/components/Badge.svelte';
	import Button from '$lib/components/Button.svelte';
	import {
		correction,
		createSkillScanReviewModel,
		type ReviewRow,
	} from '$lib/features/skillScan/skillScanReviewModel.svelte';

	let { onComplete, saving = false }: { onComplete: () => void; saving?: boolean } = $props();

	const review = createSkillScanReviewModel(() => onComplete());
	let rows = $derived(review.rows);
	let notes = $derived(review.notes);
	let loading = $derived(review.loading);
	let error = $derived(review.error);
	// Nothing settles a review while one request about it is in flight, or
	// while it is being saved.
	let locked = $derived(review.busy || saving);

	$effect(() => {
		void review.load();
	});

	// `runningGain` is anchor → believed: how much the chatlog tracker accumulated
	// since the last scan. Useful as context next to the believed value.
	function runningGain(row: ReviewRow): number | null {
		if (row.anchor === null || row.believed === null) return null;
		return row.believed - row.anchor;
	}

	function formatLevel(value: number | null): string {
		if (value === null) return '—';
		return value.toFixed(2);
	}

	function formatSignedDelta(value: number | null): string {
		if (value === null) return '—';
		if (Math.abs(value) < 0.005) return '0.00';
		return (value > 0 ? '+' : '') + value.toFixed(2);
	}

	function deltaColor(value: number | null): string {
		if (value === null || Math.abs(value) < 0.005) return 'text-text-tertiary';
		return value > 0 ? 'text-success' : 'text-warning';
	}
</script>

<div class="flex h-full flex-col">
	<div class="mb-2 flex items-baseline justify-between">
		<h3 class="text-sm font-semibold uppercase tracking-wide text-text-secondary">
			Skills review
		</h3>
		<span class="text-xs text-text-tertiary tabular-nums">{rows.length} rows</span>
	</div>

	{#if !loading && notes.length > 0}
		<div class="mb-3 space-y-2 text-xs">
			{#each notes as note (note.text)}
				<div>
					<p class="text-warning">{note.text}</p>
					{#if note.items.length > 0}
						<p class="text-text-secondary">{note.items.join(', ')}</p>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	{#if loading}
		<p class="py-8 text-center text-sm text-text-tertiary">Loading review…</p>
	{:else if error && rows.length === 0}
		<p class="py-8 text-center text-sm text-warning">{error}</p>
	{:else if rows.length === 0}
		<p class="py-8 text-center text-sm text-text-tertiary">Nothing in this scan can be saved.</p>
	{:else}
		<div class="flex-1 overflow-y-auto rounded border border-border">
			<table class="w-full text-sm">
				<thead class="sticky top-0 bg-surface">
					<tr class="border-b border-border">
						<th class="py-2 px-3 text-left text-xs font-medium uppercase tracking-wide text-text-secondary">Name</th>
						<th class="py-2 px-3 text-right text-xs font-medium uppercase tracking-wide text-text-secondary">Anchor</th>
						<th class="w-4"></th>
						<th class="py-2 px-3 text-right text-xs font-medium uppercase tracking-wide text-text-secondary">Believed</th>
						<th class="w-4"></th>
						<th class="py-2 px-3 text-right text-xs font-medium uppercase tracking-wide text-text-secondary">Scanned</th>
						<th class="py-2 px-3 text-right text-xs font-medium uppercase tracking-wide text-text-secondary" title="Scanned − Believed (running sum vs. ground truth)">Δ</th>
					</tr>
				</thead>
				<tbody>
					{#each rows as row (row.name)}
						{@const corr = correction(row)}
						{@const gain = runningGain(row)}
						{@const unchanged = row.anchor !== null && Math.abs(row.scanned - row.anchor) < 0.005}
						<tr class="border-b border-border/50 transition-colors hover:bg-surface-hover/40" class:opacity-50={unchanged}>
							<td class="py-2 px-3">
								<div class="flex items-center gap-2">
									<span class="text-text">{row.name}</span>
									{#if row.isAttribute}
										<Badge variant="neutral">Attribute</Badge>
									{/if}
									{#if row.anchor === null}
										<Badge variant="accent">New</Badge>
									{/if}
								</div>
							</td>
							<td class="py-2 px-3 text-right tabular-nums text-text-secondary">{formatLevel(row.anchor)}</td>
							<td class="text-center text-text-tertiary text-xs">→</td>
							<td class="py-2 px-3 text-right tabular-nums">
								<div class="flex flex-col items-end leading-tight">
									<span class="text-text">{formatLevel(row.believed)}</span>
									{#if gain !== null && Math.abs(gain) >= 0.005}
										<span class="text-[10px] {deltaColor(gain)}">{formatSignedDelta(gain)}</span>
									{/if}
								</div>
							</td>
							<td class="text-center text-text-tertiary text-xs">→</td>
							<td class="py-2 px-3 text-right tabular-nums font-medium text-accent">{formatLevel(row.scanned)}</td>
							<td class="py-2 px-3 text-right tabular-nums {deltaColor(corr)}">
								{row.believed === null ? '—' : formatSignedDelta(corr)}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	{#if error && rows.length > 0}
		<p class="mt-2 text-xs text-warning">{error}</p>
	{/if}

	<div class="mt-3 flex items-center justify-end gap-2">
		{#if review.needsConfirmation}
			<button
				type="button"
				class="mr-auto text-xs text-text-tertiary underline-offset-2 hover:text-text-secondary hover:underline disabled:opacity-50 cursor-pointer"
				onclick={review.saveAnyway}
				disabled={locked || loading || rows.length === 0}
			>
				Save these figures anyway
			</button>
			<Button variant="ghost" onclick={review.reject} disabled={locked}>
				{#snippet children()}Reject{/snippet}
			</Button>
			<Button onclick={review.scanAgain} disabled={locked}>
				{#snippet children()}Scan again{/snippet}
			</Button>
		{:else}
			<Button variant="ghost" onclick={review.reject} disabled={locked}>
				{#snippet children()}Reject{/snippet}
			</Button>
			<Button onclick={review.accept} disabled={locked || loading || rows.length === 0}>
				{#snippet children()}Accept{/snippet}
			</Button>
		{/if}
	</div>
</div>
