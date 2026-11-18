<script lang="ts">
	/**
	 * The review: one line per item whose quantity differs, the largest
	 * difference by TT first, each set to what the app proposes until the
	 * player says otherwise. A filter and a selection settle many lines at
	 * once, or make several into one conversion. Differences a standing
	 * check accepted, and which stand as they were, are filed apart and
	 * can be brought back into review. Items the comparison leaves out are
	 * listed apart, each with why.
	 */
	import Button from '$lib/components/Button.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import { formatQuantity, formatSeenAt, listNames } from '$lib/utils/format';
	import CheckLine from './CheckLine.svelte';
	import {
		acceptedWhen,
		applySummary,
		type Choice,
		signedQuantity,
		EXCLUSION_LABEL,
		hasExactTt,
		isShrapnel,
		isShortfall,
	} from './reconciliation';
	import type { ReconciliationModel } from './reconciliationModel.svelte';

	let { model }: { model: ReconciliationModel } = $props();

	const draft = $derived(model.draft!);
	const lineError = $derived(
		model.revisionError?.scope.kind === 'line' ? model.revisionError : null,
	);
	const bulkError = $derived(
		model.revisionError?.scope.kind === 'bulk' ? model.revisionError.message : null,
	);
	const shrapnelError = $derived(
		model.revisionError?.scope.kind === 'shrapnel' ? model.revisionError.message : null,
	);
	const summary = $derived(applySummary(draft));
	const fromWhere = $derived(
		draft.source.kind === 'paste' ? 'the inventory you pasted' : 'your account as last read',
	);

	let showExcluded = $state(false);
	let showAccepted = $state(false);
	const accepted = $derived(draft.previouslyAccepted ?? []);
	let list = $state<HTMLUListElement | null>(null);

	/** The selection bar goes once the selection is spent; the focus moves
	 * to the list rather than being lost with it. */
	async function group() {
		await model.groupSelection();
		if (model.selectedLines.length === 0) list?.focus();
	}

	function clear() {
		model.clearSelection();
		list?.focus();
	}

	const bulkChoices: { id: Choice; label: string }[] = [
		{ id: 'removed', label: 'Removed' },
		{ id: 'shrapnelConverted', label: 'Shrapnel converted' },
		{ id: 'unknownOrigin', label: 'Unknown origin' },
		{ id: 'leftAsIs', label: 'Left as is' },
	];
	const offered = $derived(
		bulkChoices.filter(
			(option) =>
				option.id !== 'shrapnelConverted' ||
				model.selectedLines.some((line) => isShrapnel(line) && isShortfall(line)),
		),
	);
	/** A selected line whose TT per unit is not exact cannot balance a
	 * conversion, so none is offered. */
	const inexactSelected = $derived(model.selectedLines.filter((line) => !hasExactTt(line)));

	function lineErrorFor(item: string): string | null {
		return lineError?.scope.kind === 'line' && lineError.scope.item === item
			? lineError.message
			: null;
	}
</script>

<div class="space-y-6">
	<!-- Nothing in the review moves while the check is being applied. -->
	<div class="space-y-6" inert={model.applying} aria-busy={model.applying}>
		<p class="text-sm text-text-secondary">
			Compared with {fromWhere}, {formatSeenAt(draft.observedAt)}.
			{#if draft.lines.length > 0}
				{formatQuantity(draft.lines.length)}
				{draft.lines.length === 1 ? 'item differs' : 'items differ'}; the largest difference first.
			{/if}
			<button type="button" class="ml-1 text-xs font-medium text-accent hover:text-text" onclick={model.startOver}>
				Start again
			</button>
		</p>

		{#if draft.caution?.kind === 'pasteSoonAfterSession'}
			<p class="flex items-start gap-2.5 text-sm text-text-secondary" role="note">
				<span class="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-warning" aria-hidden="true"></span>
				<span>
					You pasted this soon after a session ended: copy your inventory afresh if it changed since.
					<span class="text-text-tertiary">The session ended {formatSeenAt(draft.caution.sessionEndedAt)}.</span>
				</span>
			</p>
		{/if}

		{#if draft.lines.length === 0}
			<div class="space-y-1.5 py-8">
				{#if accepted.length > 0}
					<p class="text-lg font-light text-text">Nothing new differs.</p>
					<p class="text-sm text-text-tertiary">
						{accepted.length === 1
							? 'The one difference you accepted before stands as it was.'
							: `The ${formatQuantity(accepted.length)} differences you accepted before stand as they were.`}
					</p>
				{:else}
					<p class="text-lg font-light text-text">Your inventory matches your account.</p>
					<p class="text-sm text-text-tertiary">Every item compared holds the same quantity in both.</p>
				{/if}
			</div>
		{:else}
			<section aria-label="Items that differ" class="space-y-3">
				<div class="flex flex-wrap items-center gap-x-5 gap-y-2">
					<SearchInput
						class="w-full sm:w-64"
						bind:value={() => model.query, (next) => (model.query = next)}
						placeholder="Filter items"
						aria-label="Filter items"
					/>
					<label class="flex items-center gap-2 text-xs text-text-secondary">
						<input
							type="checkbox"
							class="h-4 w-4 accent-[var(--color-accent)]"
							checked={model.allVisibleSelected}
							disabled={model.visible.length === 0}
							onchange={model.toggleVisible}
						/>
						Select all {model.query.trim() ? 'shown' : ''} ({formatQuantity(model.visible.length)})
					</label>
				</div>

				{#if model.selectedLines.length > 0}
					<div
						class="flex flex-wrap items-center gap-x-3 gap-y-2 border-l-2 border-accent/50 py-1 pl-3"
						role="group"
						aria-label="Change the selected items"
					>
						<span class="text-xs tabular-nums text-text-secondary">
							{formatQuantity(model.selectedLines.length)} selected{#if model.hiddenSelected > 0}, {formatQuantity(model.hiddenSelected)} not shown{/if}
						</span>
						{#each offered as option (option.id)}
							<Button size="sm" variant="ghost" onclick={() => void model.setSelection(option.id)}>
								{option.label}
							</Button>
						{/each}
						<Button
							size="sm"
							variant="secondary"
							disabled={model.selectedLines.length < 2 || inexactSelected.length > 0}
							title="The missing items went into the one that arrived"
							aria-describedby={model.selectedLines.length < 2 || inexactSelected.length > 0
								? 'check-no-conversion'
								: undefined}
							onclick={() => void group()}
						>
							Make one conversion
						</Button>
						<button type="button" class="text-xs text-text-tertiary hover:text-text" onclick={clear}>
							Clear
						</button>
						{#if model.selectedLines.length < 2}
							<p id="check-no-conversion" class="basis-full text-xs text-text-tertiary">
								To make one conversion, select the items that went in and the one that came out.
							</p>
						{:else if inexactSelected.length > 0}
							<p id="check-no-conversion" class="basis-full text-xs text-text-tertiary">
								{listNames(inexactSelected.map((line) => line.itemName))} cannot join a conversion:
								{inexactSelected.length === 1 ? 'its' : 'their'} TT per unit is not known exactly.
							</p>
						{/if}
						{#if bulkError}
							<p class="basis-full text-xs text-error" role="alert">{bulkError}</p>
						{:else if model.bulkNote}
							<p class="basis-full text-xs text-text-secondary" role="status">{model.bulkNote}</p>
						{/if}
					</div>
				{/if}

				<div
					class="hidden grid-cols-[1.25rem_minmax(0,1fr)_5.5rem_5.5rem_7.5rem_17rem] gap-x-4 border-b border-border/50 pb-2 sm:grid"
					aria-hidden="true"
				>
					<span></span>
					<span class="eyebrow">Item</span>
					<span class="eyebrow text-right">In the app</span>
					<span class="eyebrow text-right">Your account</span>
					<span class="eyebrow text-right">Difference</span>
					<span class="eyebrow text-right">What happened</span>
				</div>

				{#if model.visible.length === 0}
					<p class="py-4 text-center text-xs text-text-tertiary">No item matches that filter.</p>
				{:else}
					<ul
						bind:this={list}
						tabindex="-1"
						class="divide-y divide-border/50 focus:outline-none"
						aria-label="Items that differ"
						aria-busy={model.revising}
					>
						{#each model.visible as line (line.itemName)}
							<CheckLine
								{line}
								{draft}
								query={model.query}
								selected={model.selected.has(line.itemName)}
								error={lineErrorFor(line.itemName)}
								{shrapnelError}
								ontoggle={() => model.toggle(line.itemName)}
								onchoose={(choice) => void model.setLine(line, choice)}
								onsetapart={() => model.setApart(line)}
								onshrapnelsold={(quantity) => void model.setShrapnelSold(quantity)}
							/>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}

		{#if accepted.length > 0}
			<section aria-label="Previously accepted differences" class="space-y-2">
				<button
					type="button"
					class="flex items-center gap-2 text-xs text-text-secondary hover:text-text"
					aria-expanded={showAccepted}
					aria-controls="check-accepted"
					onclick={() => (showAccepted = !showAccepted)}
				>
					<span class="inline-block transition-transform {showAccepted ? 'rotate-90' : ''}" aria-hidden="true">&rsaquo;</span>
					Previously accepted differences ({formatQuantity(accepted.length)})
				</button>
				{#if showAccepted}
					<p class="text-xs text-text-tertiary">
						Left as they are: each differs exactly as it did when you accepted it. One that moves comes back for review.
					</p>
					<ul id="check-accepted" class="divide-y divide-border/40 border-y border-border/40">
						{#each accepted as row (row.itemName)}
							<li class="grid grid-cols-[minmax(0,1fr)_5.5rem_auto] items-center gap-x-4 py-2 sm:grid-cols-[minmax(0,1fr)_5.5rem_5.5rem_5.5rem_minmax(0,9rem)_auto]">
								<span class="truncate text-sm text-text-secondary" title={row.itemName}>{row.itemName}</span>
								<span class="hidden text-right text-xs tabular-nums text-text-tertiary sm:block">{formatQuantity(row.appQuantity)}</span>
								<span class="hidden text-right text-xs tabular-nums text-text-tertiary sm:block">{formatQuantity(row.observedQuantity)}</span>
								<span class="text-right text-xs tabular-nums text-text-secondary">{signedQuantity(row.difference)}</span>
								<span class="hidden truncate text-xs text-text-tertiary sm:block">{acceptedWhen(row)}</span>
								<button
									type="button"
									class="text-xs font-medium text-accent hover:text-text disabled:opacity-50"
									disabled={model.revising}
									onclick={() => void model.reconsider(row.itemName)}
								>
									Review again
								</button>
								{#if lineErrorFor(row.itemName)}
									<p class="col-span-full text-xs text-error" role="alert">{lineErrorFor(row.itemName)}</p>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}

		{#if draft.excluded.length > 0}
			<section aria-label="Left out of the comparison" class="space-y-2">
				<button
					type="button"
					class="flex items-center gap-2 text-xs text-text-secondary hover:text-text"
					aria-expanded={showExcluded}
					aria-controls="check-excluded"
					onclick={() => (showExcluded = !showExcluded)}
				>
					<span class="inline-block transition-transform {showExcluded ? 'rotate-90' : ''}" aria-hidden="true">&rsaquo;</span>
					Left out of the comparison ({formatQuantity(draft.excluded.length)})
				</button>
				{#if showExcluded}
					<ul id="check-excluded" class="divide-y divide-border/40 border-y border-border/40">
						{#each draft.excluded as row, i (`${row.itemName}-${row.reason}-${i}`)}
							<li class="grid grid-cols-[minmax(0,1fr)_5rem] gap-x-4 py-2 sm:grid-cols-[minmax(0,1fr)_5rem_minmax(0,1.2fr)]">
								<span class="truncate text-sm text-text-secondary" title={row.itemName}>{row.itemName}</span>
								<span class="text-right text-xs tabular-nums text-text-tertiary">&times;{formatQuantity(row.quantity)}</span>
								<span class="col-span-full text-xs text-text-tertiary sm:col-span-1">{EXCLUSION_LABEL[row.reason]}</span>
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}

	</div>

	{#if draft.lines.length > 0}
		<footer class="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-border/50 pt-5">
			<p class="min-w-0 flex-1 text-sm text-text-secondary" aria-live="polite">{summary}.</p>
			{#if model.applyError}
				<p class="basis-full text-xs text-error sm:order-first" role="alert">{model.applyError}</p>
			{/if}
			{#if model.revising && !model.applying}
				<p class="text-xs text-text-tertiary" role="status">Saving your last change...</p>
			{/if}
			<!-- Enabled while a change is saving: applying waits for it, so the
				first click is never lost. -->
			<Button loading={model.applying} onclick={() => void model.apply()}>Apply</Button>
		</footer>
	{/if}
</div>
