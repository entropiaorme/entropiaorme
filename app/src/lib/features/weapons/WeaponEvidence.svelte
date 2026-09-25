<script lang="ts">
	import { Button, Divider, ErrorNotice, Menu, SegmentedControl } from '$lib/components';
	import { formatClock } from '$lib/features/healing/healingReview';
	import { formatPed, NO_DATA } from '$lib/utils/format';
	import {
		attributionTally,
		describeReview,
		describeShot,
		formatDamage,
		shotStanding,
		weaponOption,
	} from './weaponReview';
	import type { WeaponReviewModel } from './weaponReviewModel.svelte';

	let { model }: { model: WeaponReviewModel } = $props();

	const summary = $derived(model.summary);
	const reviewable = $derived(summary.unresolved + summary.evidenceShots);
	const groupOptions = $derived(
		model.groups.map((group) => ({
			id: group.id,
			label: `${group.label} ${group.count}`,
			ariaLabel: `${group.label}: ${group.count}`,
		})),
	);
</script>

<Divider />
<section aria-labelledby="weapon-evidence-heading" data-testid="weapon-evidence">
	<div class="mb-3 flex flex-wrap items-baseline justify-between gap-2">
		<h3 id="weapon-evidence-heading" class="eyebrow">Weapon attribution</h3>
		<div class="text-xs text-text-tertiary">{attributionTally(summary)}</div>
	</div>

	<ErrorNotice message={model.error} onDismiss={model.dismissError} class="mb-3" />

	{#if summary.unpriced > 0}
		{@const one = summary.unpriced === 1}
		<p class="mb-3 text-xs text-text-secondary" data-testid="weapon-unpriced">
			<span class="text-warning">{`${summary.unpriced} ${one ? 'shot' : 'shots'} could not be priced to one weapon.`}</span>
			{`The weapon cost leaves ${one ? 'it' : 'them'} out${summary.correctable ? ' until assigned below' : ''}.`}
		</p>
	{/if}

	{#if summary.reviews.length > 0}
		<div class="divide-y divide-border/40">
			{#each summary.reviews as review (review.id)}
				<div class="flex items-center gap-4 py-2 text-sm" data-testid="weapon-review">
					<span class="w-20 shrink-0 text-xs tabular-nums text-text-tertiary">{formatClock(review.decidedAt)}</span>
					<span class="min-w-0 flex-1 truncate text-text">{describeReview(review)}</span>
					<span class="shrink-0 tabular-nums text-text-secondary">
						{review.costDelta >= 0 ? '+' : ''}{formatPed(review.costDelta)} PED
					</span>
				</div>
			{/each}
		</div>
	{/if}

	{#if reviewable > 0}
		<div class="mt-3">
			<button
				type="button"
				class="linklet inline-flex items-center gap-1 text-xs"
				aria-expanded={model.reviewOpen}
				aria-controls="weapon-shots"
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
				Review shots ({reviewable})
			</button>

			{#if model.reviewOpen}
				<div id="weapon-shots" class="mt-3 flex flex-col gap-2">
					{#if groupOptions.length > 1}
						<SegmentedControl
							options={groupOptions}
							active={model.group}
							onchange={(id) => void model.setGroup(id as 'unresolved' | 'evidence')}
						/>
					{/if}
					<ErrorNotice message={model.shotsError} />
					{#if model.shots.length > 0}
						<div class="divide-y divide-border/40">
							{#each model.shots as shot (shot.id)}
								{@const standing = shotStanding(shot)}
								{@const offered = model.weaponsFor(shot.id)}
								<div class="flex items-center gap-4 py-1.5 text-sm" data-testid="weapon-shot">
									<span class="w-20 shrink-0 text-xs tabular-nums text-text-tertiary">{formatClock(shot.observedAt)}</span>
									<span class="w-16 shrink-0 tabular-nums text-text">
										{#if shot.amount == null}
											<span class="text-text-tertiary">{NO_DATA}</span>
										{:else}
											{formatDamage(shot.amount)}{#if shot.critical}<span
													class="ml-1 text-[10px] uppercase tracking-wider text-accent">crit</span
												>{/if}
										{/if}
									</span>
									<span class="min-w-0 flex-1 truncate text-xs text-text-tertiary" title={describeShot(shot)}>
										{describeShot(shot)}
									</span>
									<span class="shrink-0 text-xs tabular-nums">
										{#if standing.kind === 'unpriced'}
											<span class="text-warning">Unpriced</span>
										{:else}
											<span class="text-text-secondary">{standing.tool}</span>
											<span class="text-text-tertiary">· {formatPed(standing.cost)} PED</span>
										{/if}
									</span>
									<span class="flex w-24 shrink-0 justify-end">
										{#if standing.kind === 'assigned'}
											{@const correctionId = standing.correctionId}
											<Button
												variant="ghost"
												size="sm"
												loading={model.busy === correctionId}
												disabled={model.busy !== null}
												aria-label={`Undo the assignment of the shot at ${formatClock(shot.observedAt)}`}
												onclick={() => model.undo(correctionId)}
											>
												{#snippet children()}Undo{/snippet}
											</Button>
										{:else if shot.correctable}
											<Menu overlay panelClass="w-64 p-1">
												{#snippet trigger({ open, toggle, keydown })}
													<button
														type="button"
														class="linklet text-xs disabled:opacity-40"
														aria-haspopup="menu"
														aria-expanded={open}
														aria-label={`Assign the shot at ${formatClock(shot.observedAt)} to a weapon`}
														disabled={model.busy !== null}
														onclick={() => {
															if (!open) void model.loadWeapons(shot.id);
															toggle();
														}}
														onkeydown={keydown}
													>
														{model.busy === shot.id ? 'Saving…' : 'Assign…'}
													</button>
												{/snippet}
												{#snippet children({ close })}
													<div class="eyebrow px-2 pb-1 pt-1.5 text-text-tertiary">Fired by</div>
													{#if !offered || offered.status === 'loading'}
														<div class="px-2 py-1.5 text-xs text-text-tertiary">Loading weapons…</div>
													{:else if offered.status === 'error'}
														<div class="px-2 py-1.5 text-xs text-negative">{offered.message}</div>
													{:else if offered.weapons.length === 0}
														<div class="px-2 py-1.5 text-xs text-text-tertiary">
															None of the weapons carried then is still in Equipment
														</div>
													{:else}
														{#each offered.weapons as weapon (weapon.equipmentId)}
															<button
																type="button"
																role="menuitem"
																class="flex w-full items-baseline justify-between gap-2 rounded-md px-2 py-1.5 text-left text-sm
																	transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover focus:bg-surface-hover focus:outline-none"
																aria-label={weaponOption(weapon, formatPed)}
																onclick={() => {
																	close();
																	void model.assign(shot.id, weapon.equipmentId);
																}}
															>
																<span class="truncate {weapon.fits ? 'text-text' : 'text-text-secondary'}">{weapon.name}</span>
																<span class="shrink-0 text-xs tabular-nums text-text-tertiary">
																	{#if weapon.fits}<span class="mr-1.5 text-positive">Fits</span>{/if}{formatPed(weapon.costPerShotPed)} PED
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
					{:else if model.loadingShots}
						<div class="py-1.5 text-xs text-text-tertiary">Loading…</div>
					{/if}
					{#if model.hasMore}
						<div>
							<Button variant="ghost" size="sm" loading={model.loadingShots} onclick={() => void model.loadMore()}>
								{#snippet children()}Show more ({model.total - model.shots.length}){/snippet}
							</Button>
						</div>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</section>
