<script lang="ts">
	import { quintOut } from 'svelte/easing';
	import { Badge, Button, ErrorNotice, Menu } from '$lib/components';
	import SessionDetailView from '$lib/components/SessionDetail.svelte';
	import { shouldSettleInstantly } from '$lib/motion/testMotion';
	import { formatDate, formatDuration, formatPed } from '$lib/utils/format';
	import { PAGE_SIZE } from './instancesModel.svelte';
	import ReviewDefinitionPicker from './ReviewDefinitionPicker.svelte';
	import type { ReviewModel } from './reviewModel.svelte';

	let {
		model
	}: {
		model: ReviewModel;
	} = $props();

	const instances = $derived(model.instances);
	const table = $derived(model.instances.table);

	/** The surface's entrance, matching the authoring environment's own:
	 * it arrives after the page content has animated away, and leaves
	 * first on close. Reduced motion and the e2e freeze collapse both to
	 * an instant settle. */
	function surface(_node: HTMLElement, { entering }: { entering: boolean }) {
		if (shouldSettleInstantly()) return { duration: 0 };
		return {
			duration: entering ? 260 : 170,
			delay: entering ? 160 : 0,
			easing: quintOut,
			css: (t: number, u: number) => `opacity: ${t}; transform: translateY(${12 * u}px);`
		};
	}

	// Escape leaves. Tab is deliberately not trapped, matching the
	// authoring environment: the surface takes over the page's region,
	// not the window, so navigating away is itself a way out.
	function handleKeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		// An armed delete is the inner layer: Escape disarms it first, so
		// it never discards more than was asked. An open move menu is
		// inner again and stops the press reaching here at all.
		if (instances.confirmDeleteId !== null) {
			instances.confirmDeleteId = null;
			return;
		}
		model.close();
	}

	const archived = $derived(model.definition !== null && !model.definition.isActive);

	/** Some row on this page nets without an armour cost still to be recorded. */
	const armourPendingOnPage = $derived(table.pageRows.some((session) => instances.armourPending(session.id)));

	// An armour recording from the overlay, or a healing correction in a
	// session's detail, moves session figures: follow both while open.
	$effect(() => {
		if (!model.open) return;
		const current = instances;
		let stop: (() => void) | undefined;
		let disposed = false;
		void current.subscribeCostChanges().then((unlisten) => {
			if (disposed) unlisten();
			else stop = unlisten;
		});
		return () => {
			disposed = true;
			stop?.();
		};
	});
</script>

<svelte:window onkeydown={model.open ? handleKeydown : undefined} />

{#if model.open}
	<div
		class="absolute inset-0 z-30 overflow-y-auto bg-base focus:outline-hidden"
		role="region"
		aria-label="Review sessions"
		tabindex="-1"
		data-testid="session-review"
		in:surface={{ entering: true }}
		out:surface={{ entering: false }}
	>
		<div class="mx-auto flex min-h-full w-full max-w-4xl flex-col gap-6 px-6 py-10">
			<!-- Header: which session, switchable in place, plus the exit -->
			<div class="flex items-start justify-between gap-4">
				<div class="flex min-w-0 flex-col gap-1.5">
					<h2 class="text-2xl font-semibold tracking-tight text-text">Review sessions</h2>
					<div class="flex items-center gap-2 min-w-0">
						<ReviewDefinitionPicker {model} />

						{#if archived}
							<span class="text-xs text-text-tertiary">Archived</span>
							<Button
								size="sm"
								variant="secondary"
								loading={model.restoring}
								onclick={() => model.restoreCurrent()}
							>
								{#snippet children()}
									<svg
										xmlns="http://www.w3.org/2000/svg"
										fill="none"
										viewBox="0 0 24 24"
										stroke-width="1.5"
										stroke="currentColor"
										class="h-3.5 w-3.5"
										aria-hidden="true"
									>
										<path
											stroke-linecap="round"
											stroke-linejoin="round"
											d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3"
										/>
									</svg>
									Restore
								{/snippet}
							</Button>
						{/if}
					</div>
				</div>

				<button
					class="h-8 w-8 flex items-center justify-center rounded-md text-text-secondary
						cursor-pointer transition-colors duration-[var(--duration-fast)]
						hover:text-text hover:bg-surface-hover shrink-0"
					onclick={() => model.close()}
					aria-label="Close review"
					title="Close review (Esc)"
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4.5 w-4.5">
						<path
							d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"
						/>
					</svg>
				</button>
			</div>

			<ErrorNotice
				message={model.error ?? instances.error}
				onDismiss={() => {
					model.error = null;
					instances.error = null;
				}}
			/>

			{#if model.definitionId === null}
				<div class="panel p-6">
					<p class="text-center text-sm text-text-tertiary">
						Choose a session above to review what has been recorded under it.
					</p>
				</div>
			{:else if instances.loading}
				<p class="text-sm text-text-secondary">Loading sessions...</p>
			{:else if instances.sessions.length === 0}
				<div class="panel p-6">
					<p class="text-center text-sm text-text-tertiary">
						No sessions recorded under this one yet.
					</p>
				</div>
			{:else}
				<div class="overflow-x-auto rounded-md border border-border">
					<table class="w-full border-collapse text-left text-sm">
						<thead class="bg-surface-hover/50 text-xs uppercase tracking-wider text-text-secondary">
							<tr>
								<th class="border-b border-border px-4 py-3 font-medium">Started</th>
								<th class="border-b border-border px-4 py-3 font-medium">Duration</th>
								<th class="border-b border-border px-4 py-3 font-medium">Mobs</th>
								<th class="border-b border-border px-4 py-3 text-right font-medium">Net</th>
								<th class="border-b border-border px-4 py-3 text-right font-medium">Badges</th>
								<th class="w-10 border-b border-border px-4 py-3"></th>
							</tr>
						</thead>
						<tbody class="bg-surface">
							{#each table.pageRows as session (session.id)}
								{@const isExpanded = instances.expandedSessionId === session.id}
								<tr
									class="cursor-pointer transition-colors hover:bg-surface-hover/50
										{isExpanded ? 'bg-surface-hover' : ''}"
									tabindex="0"
									aria-expanded={isExpanded}
									onclick={() => instances.toggleSession(session.id)}
									onkeydown={(e) => {
										if (e.key === 'Enter' || e.key === ' ') {
											e.preventDefault();
											instances.toggleSession(session.id);
										}
									}}
								>
									<td class="border-b border-border/50 px-4 py-3 tabular-nums">
										{session.startTime ? formatDate(session.startTime) : '\u2014'}
									</td>
									<td class="border-b border-border/50 px-4 py-3 text-text-secondary">
										{formatDuration(session.duration)}
									</td>
									<td class="border-b border-border/50 px-4 py-3">
										<div class="max-w-[200px] truncate" title={session.primaryMobs.join(', ')}>
											{#if session.primaryMobs.length > 0}
												{session.primaryMobs.join(', ')}
											{:else}
												<span class="italic text-text-tertiary">None</span>
											{/if}
										</div>
									</td>
									<td
										class="border-b border-border/50 px-4 py-3 text-right font-semibold tabular-nums
											{session.net >= 0 ? 'text-positive' : 'text-negative'}"
									>
										{session.net >= 0 ? '+' : ''}{formatPed(session.net)}{#if instances.armourPending(session.id)}<span
												class="ml-0.5 align-super text-[10px] font-normal text-warning"
												title="Armour cost not recorded yet: this net leaves it out"
												aria-hidden="true">*</span
											><span class="sr-only">, armour cost not recorded yet</span>{/if}
									</td>
									<td class="border-b border-border/50 px-4 py-3">
										<div class="flex items-center justify-end gap-1">
											{#if session.globals > 0}
												<Badge variant="warning">{session.globals}G</Badge>
											{/if}
											{#if session.hofs > 0}
												<Badge variant="accent">{session.hofs}H</Badge>
											{/if}
										</div>
									</td>
									<td
										class="border-b border-border/50 px-4 py-3 text-right"
										onclick={(e) => e.stopPropagation()}
									>
										<div class="flex items-center justify-end gap-2">
											{#if instances.confirmDeleteId === session.id}
												<div class="flex items-center gap-1">
													<Button
														size="sm"
														variant="danger"
														disabled={instances.deleting}
														onclick={() => model.remove(session.id)}
													>
														{#snippet children()}Delete{/snippet}
													</Button>
													<Button
														size="sm"
														variant="ghost"
														onclick={() => (instances.confirmDeleteId = null)}
													>
														{#snippet children()}Cancel{/snippet}
													</Button>
												</div>
											{:else}
												<!-- Move: the correction for a session recorded under
													 whichever session the picker happened to hold. -->
												<!-- The table scrolls, so this one floats over the
													 viewport: laid out inside the scroll box it is
													 clipped, and the box grows to fit it, which reads
													 as the list being squashed to make room. -->
												<Menu
													overlay
													align="right"
													ariaLabel="Move to another session"
													panelClass="w-56 p-1"
												>
													{#snippet trigger({ open, toggle })}
														<button
															type="button"
															class="icon-button-row p-1"
															aria-haspopup="menu"
															aria-expanded={open}
															aria-label="Move to another session"
															title="Move to another session"
															disabled={instances.reassigning}
															onclick={toggle}
														>
															<svg
																xmlns="http://www.w3.org/2000/svg"
																viewBox="0 0 20 20"
																fill="currentColor"
																class="h-4 w-4"
															>
																<path
																	fill-rule="evenodd"
																	d="M10.293 3.293a1 1 0 011.414 0l5 5a1 1 0 01-1.414 1.414L12 6.414V16a1 1 0 11-2 0V6.414L6.707 9.707a1 1 0 01-1.414-1.414l5-5z"
																	clip-rule="evenodd"
																	transform="rotate(90 10 10)"
																/>
															</svg>
														</button>
													{/snippet}

													{#snippet children({ close })}
														{#if model.moveTargets.length === 0}
															<p class="px-2 py-1.5 text-xs text-text-tertiary">
																No other session to move this to.
															</p>
														{:else}
															<p class="eyebrow px-2 py-1">Move to</p>
															{#each model.moveTargets as target (target.id)}
																<button
																	type="button"
																	role="menuitem"
																	class="mt-0.5 w-full truncate rounded px-2 py-1.5 text-left text-sm
																		cursor-pointer text-text-secondary
																		hover:bg-surface-hover hover:text-text"
																	onclick={() => {
																		close();
																		void model.reassign(session.id, target.id);
																	}}
																>
																	{target.name}
																</button>
															{/each}
														{/if}
													{/snippet}
												</Menu>

												<button
													type="button"
													class="icon-button-row p-1"
													onclick={() => (instances.confirmDeleteId = session.id)}
													aria-label="Delete session"
													title="Delete session"
												>
													<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" class="h-4 w-4">
														<path fill-rule="evenodd" d="M8.75 1A2.75 2.75 0 006 3.75v.443c-.795.077-1.584.176-2.365.298a.75.75 0 10.23 1.482l.149-.022.841 10.518A2.75 2.75 0 007.596 19h4.807a2.75 2.75 0 002.742-2.53l.841-10.519.149.023a.75.75 0 00.23-1.482A41.03 41.03 0 0014 4.193V3.75A2.75 2.75 0 0011.25 1h-2.5zM10 4c.84 0 1.673.025 2.5.075V3.75c0-.69-.56-1.25-1.25-1.25h-2.5c-.69 0-1.25.56-1.25 1.25v.325C8.327 4.025 9.16 4 10 4zM8.58 7.72a.75.75 0 00-1.5.06l.3 7.5a.75.75 0 101.5-.06l-.3-7.5zm4.34.06a.75.75 0 10-1.5-.06l-.3 7.5a.75.75 0 101.5.06l.3-7.5z" clip-rule="evenodd" />
													</svg>
												</button>
											{/if}
											<svg
												class="h-4 w-4 text-text-tertiary transition-transform duration-200
													{isExpanded ? 'rotate-180' : ''}"
												xmlns="http://www.w3.org/2000/svg"
												viewBox="0 0 20 20"
												fill="currentColor"
											>
												<path fill-rule="evenodd" d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z" clip-rule="evenodd" />
											</svg>
										</div>
									</td>
								</tr>
								{#if isExpanded}
									<tr>
										<td colspan="6" class="border-b border-border/50 p-0">
											<div class="bg-surface-hover/30 p-4">
												{#if instances.loadingDetail}
													<p class="animate-pulse text-xs text-text-tertiary">Loading detail...</p>
												{:else if instances.expandedDetail}
													<SessionDetailView bind:detail={instances.expandedDetail} />
												{:else}
													<p class="text-xs text-text-tertiary">No detail available.</p>
												{/if}
											</div>
										</td>
									</tr>
								{/if}
							{/each}
						</tbody>
					</table>
				</div>

				{#if armourPendingOnPage}
					<p class="px-2 text-xs text-text-tertiary">
						<span class="text-warning">*</span> Armour cost not recorded yet. It is added when you record a repair or reading from the overlay's Cost button.
					</p>
				{/if}

				{#if instances.totalPages > 1}
					<div class="flex items-center justify-between px-2">
						<span class="text-xs tabular-nums text-text-tertiary">
							Showing {table.page * PAGE_SIZE + 1}{'\u2013'}{Math.min(
								(table.page + 1) * PAGE_SIZE,
								instances.total
							)} of {instances.total}
						</span>
						<div class="flex items-center gap-2">
							<Button size="sm" variant="ghost" disabled={table.page === 0} onclick={() => instances.prevPage()}>
								{#snippet children()}Previous{/snippet}
							</Button>
							<span class="px-2 text-xs font-medium">{table.page + 1} / {instances.totalPages}</span>
							<Button
								size="sm"
								variant="ghost"
								disabled={table.page >= instances.totalPages - 1 || instances.loadingMore}
								onclick={() => instances.nextPage()}
							>
								{#snippet children()}Next{/snippet}
							</Button>
						</div>
					</div>
				{/if}
			{/if}
		</div>
	</div>
{/if}
