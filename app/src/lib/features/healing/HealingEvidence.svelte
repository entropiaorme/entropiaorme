<script lang="ts">
	import { Button, Divider, ErrorNotice, Menu, SegmentedControl } from '$lib/components';
	import type { SessionDetail } from '$lib/types/tracking';
	import { formatPed } from '$lib/utils/format';
	import {
		activationStanding,
		describeOutput,
		evidenceTally,
		formatClock,
		formatHeal,
		toolOption,
		uncostedCount,
		type UncostedClassification,
	} from './healingReview';
	import type { HealingReviewModel } from './healingReviewModel.svelte';

	let {
		detail,
		model,
	}: {
		detail: SessionDetail;
		model: HealingReviewModel;
	} = $props();

	const healing = $derived(detail.healing);
	const correctable = $derived(healing.correctable);
	const uncosted = $derived(uncostedCount(healing));
	const filterOptions = $derived(
		model.filters.map((filter) => ({
			id: filter.id,
			label: `${filter.label} ${filter.count}`,
			ariaLabel: `${filter.label}: ${filter.count}`,
		})),
	);
</script>

<Divider />
<section aria-labelledby="healing-evidence-heading" data-testid="healing-evidence">
	<div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
		<h3 id="healing-evidence-heading" class="eyebrow">Healing evidence</h3>
		<div class="text-xs text-text-tertiary">{evidenceTally(healing)}</div>
	</div>

	<ErrorNotice message={model.error} onDismiss={model.dismissError} class="mb-3" />

	{#if healing.activations.length > 0}
		<div class="divide-y divide-border/40">
			{#each healing.activations as activation (activation.id)}
				{@const standing = activationStanding(activation)}
				{@const acting = model.busy !== null && (model.busy === activation.id || model.busy === activation.correction?.id)}
				<div class="flex items-center gap-4 py-2 text-sm" data-testid="healing-activation">
					<span class="w-20 shrink-0 text-xs tabular-nums text-text-tertiary">{formatClock(activation.observedAt)}</span>
					<span class="flex min-w-0 flex-1 items-baseline gap-2">
						<span class="truncate {standing === 'notPaid' ? 'text-text-tertiary' : 'text-text'}">{activation.toolName}</span>
						{#if activation.amount !== null}
							<span class="shrink-0 text-xs tabular-nums text-text-tertiary">{formatHeal(activation.amount)}</span>
						{/if}
					</span>
					{#if standing === 'notPaid'}
						<span class="text-xs text-text-tertiary">Not a paid use</span>
					{:else}
						{#if standing === 'corrected'}
							<span class="text-xs text-accent">Marked as paid</span>
						{/if}
						{#if activation.effectUntil !== null}
							<span class="text-xs text-positive">Effect window</span>
						{/if}
						<span class="text-xs text-text-tertiary">{activation.outputCount} output{activation.outputCount === 1 ? '' : 's'}</span>
					{/if}
					<span class="tabular-nums {standing === 'notPaid' ? 'text-text-tertiary line-through' : 'text-text'}">{formatPed(activation.cost)} PED</span>
					{#if correctable}
						<span class="flex w-16 shrink-0 justify-end">
							{#if activation.correction}
								{@const correctionId = activation.correction.id}
								<Button
									variant="ghost"
									size="sm"
									loading={acting}
									disabled={model.busy !== null}
									aria-label={`Undo the correction to ${activation.toolName} at ${formatClock(activation.observedAt)}`}
									onclick={() => model.undo(correctionId)}
								>
									{#snippet children()}Undo{/snippet}
								</Button>
							{:else}
								<Menu
									ariaLabel={`Correct ${activation.toolName} at ${formatClock(activation.observedAt)}`}
									overlay
									items={[
										{
											label: 'Not a paid use',
											onSelect: () => void model.markNotPaid(activation.id),
										},
									]}
								/>
							{/if}
						</span>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	{#if correctable && uncosted > 0}
		<div class="mt-3">
			<button
				type="button"
				class="linklet inline-flex items-center gap-1 text-xs"
				aria-expanded={model.reviewOpen}
				aria-controls="healing-uncosted"
				onclick={() => void model.toggleReview()}
			>
				<svg
					class="h-3 w-3 transition-transform {model.reviewOpen ? 'rotate-90' : ''}"
					viewBox="0 0 20 20"
					fill="currentColor"
					aria-hidden="true"
				>
					<path
						fill-rule="evenodd"
						d="M7.21 14.77a.75.75 0 01.02-1.06L11.168 10 7.23 6.29a.75.75 0 111.04-1.08l4.5 4.25a.75.75 0 010 1.08l-4.5 4.25a.75.75 0 01-1.06-.02z"
						clip-rule="evenodd"
					/>
				</svg>
				Review uncosted heals ({uncosted})
			</button>

			{#if model.reviewOpen}
				<div id="healing-uncosted" class="mt-3 flex flex-col gap-2">
					{#if filterOptions.length > 1}
						<SegmentedControl
							options={filterOptions}
							active={model.filter}
							onchange={(id) => void model.setFilter(id as UncostedClassification)}
						/>
					{/if}
					<ErrorNotice message={model.outputsError} />
					{#if model.outputs.length > 0}
						<div class="divide-y divide-border/40">
							{#each model.outputs as output (output.id)}
								{@const tools = model.toolsFor(output.id)}
								<div class="flex items-center gap-4 py-1.5 text-sm" data-testid="healing-uncosted-output">
									<span class="w-20 shrink-0 text-xs tabular-nums text-text-tertiary">{formatClock(output.observedAt)}</span>
									<span class="w-16 shrink-0 tabular-nums text-text">{formatHeal(output.amount)}</span>
									<span class="min-w-0 flex-1 truncate text-xs text-text-tertiary">{describeOutput(output)}</span>
									<span class="flex w-24 shrink-0 justify-end">
										{#if output.correctable}
											<Menu overlay panelClass="w-64 p-1">
												{#snippet trigger({ open, toggle, keydown })}
													<button
														type="button"
														class="linklet text-xs disabled:opacity-40"
														aria-haspopup="menu"
														aria-expanded={open}
														aria-label={`Mark the ${formatHeal(output.amount)} heal at ${formatClock(output.observedAt)} as a paid use`}
														disabled={model.busy !== null}
														onclick={() => {
															if (!open) void model.loadTools(output.id);
															toggle();
														}}
														onkeydown={keydown}
													>
														{model.busy === output.id ? 'Saving…' : 'Paid use…'}
													</button>
												{/snippet}
												{#snippet children({ close })}
													<div class="eyebrow px-2 pb-1 pt-1.5 text-text-tertiary">Paid use of</div>
													{#if !tools || tools.status === 'loading'}
														<div class="px-2 py-1.5 text-xs text-text-tertiary">Loading healing items…</div>
													{:else if tools.status === 'error'}
														<div class="px-2 py-1.5 text-xs text-negative">{tools.message}</div>
													{:else if tools.tools.length === 0}
														<div class="px-2 py-1.5 text-xs text-text-tertiary">No healing items in Equipment</div>
													{:else}
														{#each tools.tools as tool (tool.equipmentId)}
															<button
																type="button"
																role="menuitem"
																class="flex w-full items-baseline justify-between gap-2 rounded-md px-2 py-1.5 text-left text-sm
																	transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover focus:bg-surface-hover focus:outline-none"
																aria-label={toolOption(tool, formatPed)}
																onclick={() => {
																	close();
																	void model.markPaid(output.id, tool.equipmentId);
																}}
															>
																<span class="truncate {tool.fits ? 'text-text' : 'text-text-secondary'}">{tool.name}</span>
																<span class="shrink-0 text-xs tabular-nums text-text-tertiary">
																	{#if tool.fits}<span class="mr-1.5 text-positive">Fits</span>{/if}{formatPed(tool.costPerUsePed)} PED
																</span>
															</button>
														{/each}
													{/if}
												{/snippet}
											</Menu>
										{/if}
									</span>
								</div>
							{/each}
						</div>
					{:else if model.loadingOutputs}
						<div class="py-1.5 text-xs text-text-tertiary">Loading…</div>
					{/if}
					{#if model.hasMore}
						<div>
							<Button variant="ghost" size="sm" loading={model.loadingOutputs} onclick={() => void model.loadMore()}>
								{#snippet children()}Show more ({model.total - model.outputs.length}){/snippet}
							</Button>
						</div>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</section>
