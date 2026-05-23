<script lang="ts">
	import Card from '$lib/components/Card.svelte';
	import Button from '$lib/components/Button.svelte';
	import NewsBody from '$lib/components/NewsBody.svelte';
	import {
		newsOptIn,
		newsCache,
		markNewsAsRead,
		type NewsEntry,
		type SlotId,
	} from '$lib/news';
	import { refreshNews } from '$lib/newsFetch';
	import {
		SLOT_DEFAULTS,
		SLOT_ORDER,
		resolvePinSlots,
		pinnedSlugSet,
	} from '$lib/news-pins';
	import { formatDate } from '$lib/utils/format';
	import { externalLinks } from '$lib/utils/openExternal';

	let expandedPin = $state<SlotId | null>(null);
	let expandedRow = $state<string | null>(null);
	let refreshing = $state(false);
	let refreshStatus = $state<string | null>(null);

	let items = $derived($newsCache?.items ?? []);
	let pinSlots = $derived(resolvePinSlots(items));
	let pinned = $derived(
		SLOT_ORDER
			.map((slot) => ({ slot, entry: pinSlots[slot] }))
			.filter((s): s is { slot: SlotId; entry: NewsEntry } => s.entry !== null),
	);
	let pinnedSlugs = $derived(pinnedSlugSet(pinSlots));
	let chronological = $derived(
		items
			.filter((e) => !pinnedSlugs.has(e.slug))
			.slice()
			.sort((a, b) => b.date.localeCompare(a.date)),
	);
	let lastFetched = $derived($newsCache?.fetchedAt ?? null);

	// Acknowledge the cache on entry, and again whenever it changes mid-session
	// (e.g. a refresh completes while the user is on /news). markNewsAsRead is
	// idempotent: only writes when the max article date exceeds the cursor.
	$effect(() => {
		if ($newsCache) void markNewsAsRead();
	});

	function togglePin(slot: SlotId) {
		expandedPin = expandedPin === slot ? null : slot;
		if (expandedPin) expandedRow = null;
	}

	function toggleRow(slug: string) {
		expandedRow = expandedRow === slug ? null : slug;
		if (expandedRow) expandedPin = null;
	}

	function categoryLabel(c: NewsEntry['category']): string {
		return c === 'changelog' ? 'Release' : 'Article';
	}

	async function refresh() {
		if (refreshing) return;
		refreshing = true;
		refreshStatus = null;
		const result = await refreshNews();
		refreshStatus = result.ok ? 'Updated.' : `Refresh failed: ${result.reason}`;
		refreshing = false;
	}

	function pinCtaText(entry: NewsEntry, slot: SlotId): string {
		return entry.pin_cta ?? SLOT_DEFAULTS[slot].cta;
	}

	function pinBlurbText(entry: NewsEntry): string {
		return entry.pin_blurb ?? entry.dek ?? '';
	}

	// Map an N-card strip to a Tailwind grid-cols class. Single card spans
	// full width; two cards split; three cards land in the canonical layout.
	function stripGridClass(n: number): string {
		if (n <= 1) return 'grid-cols-1';
		if (n === 2) return 'grid-cols-1 md:grid-cols-2';
		return 'grid-cols-1 md:grid-cols-3';
	}
</script>

<div class="news-page">
	<div class="px-6 pb-10 space-y-7 max-w-[1180px] mx-auto">
		<header class="flex flex-col gap-1.5 pt-1">
			<h1 class="text-xl font-semibold text-text tracking-tight">News &amp; Updates</h1>
			<span class="block h-px w-12 bg-gradient-to-r from-accent/60 to-transparent"></span>
			<p class="text-sm text-text-secondary mt-0.5">Articles and release notices</p>
		</header>

		{#if !$newsOptIn}
			<Card class="p-6">
				<p class="text-sm text-text-secondary">
					News &amp; Updates is currently disabled. Enable it from the Settings page if you want the
					app to fetch articles and release notices from its public source.
				</p>
			</Card>
		{:else}
			<!-- Pinned strip: rendered for whatever subset of slots is populated -->
			{#if pinned.length > 0}
				<section
					aria-label="Pinned articles"
					class="pinned-strip grid gap-3 {stripGridClass(pinned.length)}"
				>
					{#each pinned as { slot, entry } (slot)}
						{@const isOpen = expandedPin === slot}
						<button
							type="button"
							class="pin-card panel text-left px-5 py-4 flex flex-col gap-3.5 group cursor-pointer"
							class:is-open={isOpen}
							data-slot={slot}
							onclick={() => togglePin(slot)}
							aria-expanded={isOpen}
						>
							<!-- Header row: per-slot icon + slot identity label -->
							<div class="flex items-start justify-between gap-3">
								<div class="pin-glyph" aria-hidden="true">
									{#if entry.pin_icon === 'discord'}
										<!-- Per-article override: Discord-flavoured community glyph -->
										<svg
											viewBox="0 0 24 24"
											fill="none"
											aria-hidden="true"
										>
											<path
												d="M7.05 7.2c1.5-.68 3.15-1.02 4.95-1.02s3.45.34 4.95 1.02c1.3 1.88 1.88 4.56 1.74 8.04a10.5 10.5 0 0 1-3.64 1.72l-.72-1.18c.55-.18 1.07-.41 1.55-.7-1.12.43-2.42.64-3.88.64s-2.76-.21-3.88-.64c.48.29 1 .52 1.55.7l-.72 1.18a10.5 10.5 0 0 1-3.64-1.72c-.14-3.48.44-6.16 1.74-8.04Z"
												fill="currentColor"
											/>
											<path
												d="M9.12 12.25c0-.64.43-1.15.96-1.15s.96.51.96 1.15-.43 1.16-.96 1.16-.96-.52-.96-1.16Zm3.84 0c0-.64.43-1.15.96-1.15s.96.51.96 1.15-.43 1.16-.96 1.16-.96-.52-.96-1.16Z"
												fill="var(--color-base)"
											/>
										</svg>
									{:else if slot === 'community'}
										<!-- Slot default: three-node network -->
										<svg
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="1.4"
											stroke-linecap="round"
										>
											<line x1="7" y1="7" x2="17" y2="7" />
											<line x1="7.5" y1="8.5" x2="11.5" y2="16" />
											<line x1="16.5" y1="8.5" x2="12.5" y2="16" />
											<circle cx="6" cy="7" r="2" fill="var(--color-base)" />
											<circle cx="18" cy="7" r="2" fill="var(--color-base)" />
											<circle cx="12" cy="17" r="2" fill="var(--color-base)" />
										</svg>
									{:else if slot === 'release'}
										<!-- Slot default: version-tag glyph -->
										<svg
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="1.4"
											stroke-linecap="round"
											stroke-linejoin="round"
										>
											<path d="M3 4h7l11 11-7 7L3 11V4z" />
											<circle cx="7.5" cy="8.5" r="1.4" fill="currentColor" stroke="none" />
											<line x1="11" y1="13" x2="15" y2="17" stroke-dasharray="0.5 1.5" />
										</svg>
									{:else}
										<!-- Slot default: 4-point cardinal spark -->
										<svg
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="1.3"
											stroke-linecap="round"
											stroke-linejoin="round"
										>
											<path d="M12 3.5L13.4 10.6 20.5 12 13.4 13.4 12 20.5 10.6 13.4 3.5 12 10.6 10.6z" />
											<circle cx="12" cy="12" r="0.9" fill="currentColor" stroke="none" />
										</svg>
									{/if}
								</div>
								<span class="pin-slot-label">{SLOT_DEFAULTS[slot].label}</span>
							</div>

							<!-- Title + blurb -->
							<div class="flex-1 flex flex-col gap-1.5 min-h-[88px]">
								<h2 class="pin-title">{entry.title}</h2>
								<p class="pin-blurb">{pinBlurbText(entry)}</p>
							</div>

							<!-- Foot: meta + cta -->
							<div class="pin-foot">
								<span class="pin-meta tabular-nums">{formatDate(entry.date)}</span>
								<span class="pin-cta">
									<span>{pinCtaText(entry, slot)}</span>
									<svg
										viewBox="0 0 20 20"
										fill="currentColor"
										class="h-3 w-3 pin-cta-arrow"
										aria-hidden="true"
									>
										<path
											fill-rule="evenodd"
											d="M3 10a.75.75 0 01.75-.75h10.69L11.97 6.78a.75.75 0 111.06-1.06l3.75 3.75a.75.75 0 010 1.06l-3.75 3.75a.75.75 0 11-1.06-1.06l2.47-2.47H3.75A.75.75 0 013 10z"
											clip-rule="evenodd"
										/>
									</svg>
								</span>
							</div>
						</button>
					{/each}
				</section>

				<!-- Inline expanded pin body, full-width below strip -->
				{#if expandedPin}
					{@const card = pinSlots[expandedPin]}
					{#if card}
						<section class="pin-expanded panel px-6 py-5">
							<div class="flex items-start justify-between gap-4">
								<div class="flex-1">
									<div class="flex items-center gap-2 mb-1">
										<span class="eyebrow">{SLOT_DEFAULTS[expandedPin].label}</span>
										<span class="text-[10.5px] text-text-tertiary tabular-nums">
											{formatDate(card.date)}
										</span>
									</div>
									<h3 class="text-[17px] font-semibold text-text tracking-tight mt-1">
										{card.title}
									</h3>
								</div>
								<button
									type="button"
									class="text-text-tertiary hover:text-text transition-colors flex-shrink-0 cursor-pointer"
									onclick={() => (expandedPin = null)}
									aria-label="Close"
								>
									<svg viewBox="0 0 20 20" fill="currentColor" class="h-4 w-4">
										<path
											fill-rule="evenodd"
											d="M4.28 4.22a.75.75 0 011.06 0L10 8.94l4.66-4.72a.75.75 0 111.06 1.06L11.06 10l4.66 4.72a.75.75 0 11-1.06 1.06L10 11.06l-4.66 4.72a.75.75 0 01-1.06-1.06L8.94 10 4.28 5.28a.75.75 0 010-1.06z"
											clip-rule="evenodd"
										/>
									</svg>
								</button>
							</div>
							<div class="pin-expanded-body mt-4">
								{#if card.body}
									<NewsBody markdown={card.body} />
								{:else}
									<p class="text-xs text-text-tertiary italic">No body available.</p>
								{/if}
								{#if card.link}
									<a
										class="inline-flex items-center gap-1.5 text-xs text-accent hover:text-accent-hover mt-3"
										href={card.link}
										target="_blank"
										rel="noopener noreferrer"
										use:externalLinks
									>
										<span>Open canonical link</span>
										<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-3 w-3">
											<path
												fill-rule="evenodd"
												d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
												clip-rule="evenodd"
											/>
										</svg>
									</a>
								{/if}
							</div>
						</section>
					{/if}
				{/if}
			{/if}

			<!-- Latest header row -->
			<div class="flex items-center justify-between pt-2">
				<div class="flex items-center gap-2.5">
					<span class="eyebrow-strong">{pinned.length > 0 ? 'Latest' : ''}</span>
				</div>
				<div class="flex items-center gap-4 text-[11.5px]">
					{#if refreshStatus}
						<span class="text-text-tertiary">{refreshStatus}</span>
					{/if}
					<span class="text-text-tertiary tabular-nums">
						{#if lastFetched}
							Last updated {formatDate(lastFetched)}
						{:else}
							Not yet fetched
						{/if}
					</span>
					<Button size="sm" variant="ghost" onclick={refresh} disabled={refreshing}>
						{refreshing ? 'Refreshing...' : 'Refresh'}
					</Button>
				</div>
			</div>

			<!-- Chronological feed -->
			{#if chronological.length === 0}
				<Card class="p-6">
					<p class="text-sm text-text-tertiary text-center">
						{#if items.length === 0}
							No entries yet. Press Refresh to check for the latest news.
						{:else}
							All entries are currently pinned.
						{/if}
					</p>
				</Card>
			{:else}
				<div class="space-y-1.5">
					{#each chronological as item (item.slug)}
						{@const isOpen = expandedRow === item.slug}
						<div class="feed-row panel" class:is-open={isOpen}>
							<button
								type="button"
								class="w-full text-left px-4 py-3 flex items-center gap-3 hover:bg-surface-hover/40 transition-colors cursor-pointer"
								onclick={() => toggleRow(item.slug)}
								aria-expanded={isOpen}
							>
								<span
									class="row-cat tabular-nums"
									class:is-release={item.category === 'changelog'}
								>
									{categoryLabel(item.category)}
								</span>
								<div class="flex-1 min-w-0">
									<div class="text-[13.5px] font-medium text-text truncate">{item.title}</div>
									{#if item.dek}
										<div class="text-[12px] text-text-tertiary truncate mt-0.5">{item.dek}</div>
									{/if}
								</div>
								<span class="text-[11px] text-text-tertiary tabular-nums flex-shrink-0">
									{formatDate(item.date)}
								</span>
								<svg
									class="h-3.5 w-3.5 text-text-tertiary transition-transform shrink-0"
									class:rotate-180={isOpen}
									viewBox="0 0 20 20"
									fill="currentColor"
									aria-hidden="true"
								>
									<path
										fill-rule="evenodd"
										d="M5.23 7.21a.75.75 0 011.06.02L10 11.168l3.71-3.938a.75.75 0 111.08 1.04l-4.25 4.5a.75.75 0 01-1.08 0l-4.25-4.5a.75.75 0 01.02-1.06z"
										clip-rule="evenodd"
									/>
								</svg>
							</button>
							{#if isOpen}
								<div class="border-t border-border/40 bg-surface-hover/20 px-4 py-3.5">
									{#if item.body}
										<NewsBody markdown={item.body} />
									{:else}
										<p class="text-xs text-text-tertiary italic">No body available.</p>
									{/if}
									{#if item.link}
										<a
											class="inline-flex items-center gap-1.5 text-xs text-accent hover:text-accent-hover mt-3"
											href={item.link}
											target="_blank"
											rel="noopener noreferrer"
											use:externalLinks
										>
											<span>Open canonical link</span>
											<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-3 w-3">
												<path
													fill-rule="evenodd"
													d="M5.22 14.78a.75.75 0 001.06 0l7.22-7.22v5.69a.75.75 0 001.5 0v-7.5a.75.75 0 00-.75-.75h-7.5a.75.75 0 000 1.5h5.69l-7.22 7.22a.75.75 0 000 1.06z"
													clip-rule="evenodd"
												/>
											</svg>
										</a>
									{/if}
								</div>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		{/if}
	</div>
</div>

<style>
	.news-page {
		min-height: 100%;
	}

	/* Pinned card */
	.pin-card {
		min-height: 188px;
		transition:
			border-color var(--duration-base) var(--ease-out),
			transform var(--duration-base) var(--ease-out),
			background-color var(--duration-base) var(--ease-out);
	}
	.pin-card:hover {
		border-color: color-mix(in oklab, var(--color-border-bright) 90%, transparent);
		background: color-mix(in oklab, var(--color-surface) 50%, transparent);
	}
	.pin-card.is-open {
		border-color: color-mix(in oklab, var(--color-accent) 50%, transparent);
		background: color-mix(in oklab, var(--color-surface) 55%, transparent);
		box-shadow: 0 0 0 1px color-mix(in oklab, var(--color-accent) 25%, transparent) inset;
	}

	.pin-glyph {
		color: var(--color-accent);
		filter: drop-shadow(0 0 6px color-mix(in oklab, var(--color-accent) 35%, transparent));
		transition: transform var(--duration-base) var(--ease-out);
	}
	.pin-glyph :global(svg) {
		width: 22px;
		height: 22px;
	}
	.pin-card:hover .pin-glyph {
		transform: translateY(-1px);
	}

	.pin-slot-label {
		font-size: 10px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.2em;
		color: var(--color-text-tertiary);
		padding-top: 4px;
	}

	.pin-title {
		font-size: 15.5px;
		font-weight: 600;
		letter-spacing: -0.01em;
		line-height: 1.25;
		color: var(--color-text);
		transition: color var(--duration-base) var(--ease-out);
		margin: 0;
	}
	.pin-card:hover .pin-title {
		color: var(--color-accent-hover);
	}

	.pin-blurb {
		font-size: 12.5px;
		line-height: 1.55;
		color: var(--color-text-secondary);
		margin: 0;
	}

	.pin-foot {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding-top: 10px;
		border-top: 1px solid color-mix(in oklab, var(--color-border) 50%, transparent);
		gap: 12px;
	}

	.pin-meta {
		font-size: 10.5px;
		color: var(--color-text-tertiary);
		letter-spacing: 0.04em;
	}

	.pin-cta {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: 11.5px;
		font-weight: 500;
		color: var(--color-accent);
		transition: color var(--duration-base) var(--ease-out);
	}
	.pin-card:hover .pin-cta {
		color: var(--color-accent-hover);
	}
	.pin-cta-arrow {
		transition: transform var(--duration-base) var(--ease-out);
	}
	.pin-card:hover .pin-cta-arrow {
		transform: translateX(2px);
	}

	/* Pin-expanded body */
	.pin-expanded {
		border-color: color-mix(in oklab, var(--color-accent) 35%, transparent);
		animation: pin-expand var(--duration-enter) var(--ease-out);
	}
	@keyframes pin-expand {
		from {
			opacity: 0;
			transform: translateY(-4px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	/* Feed row */
	.feed-row {
		overflow: hidden;
		transition: border-color var(--duration-base) var(--ease-out);
	}
	.feed-row:hover {
		border-color: color-mix(in oklab, var(--color-border-bright) 80%, transparent);
	}
	.feed-row.is-open {
		border-color: color-mix(in oklab, var(--color-accent) 30%, transparent);
	}

	.row-cat {
		flex-shrink: 0;
		font-size: 9.5px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.16em;
		color: var(--color-text-tertiary);
		min-width: 54px;
	}
	.row-cat.is-release {
		color: var(--color-accent);
	}
</style>
