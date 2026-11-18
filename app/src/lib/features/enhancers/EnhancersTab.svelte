<script lang="ts">
	/**
	 * The markup you paid for enhancers, by kind and tier. Enhancers are
	 * bought on their own and fitted to any weapon, so what you paid is set
	 * once per tier here, not per weapon. A break costs its TT and the
	 * markup set for its tier when it broke, less the Shrapnel it returns;
	 * a new figure applies from then on. One row per kind: Weapon Damage
	 * today, others as they are supported.
	 */
	import { onMount } from 'svelte';
	import { ErrorNotice } from '$lib/components';
	import type { EnhancerMarkupKind, EnhancerMarkups, EnhancerTierMarkup } from '$lib/api/commands.gen';
	import { getEnhancerMarkups, setEnhancerMarkup } from '$lib/api/equipment';
	import { formatDate } from '$lib/utils/format';
	import { describeError } from '$lib/view/errorState';
	import { parseMarkup } from '$lib/features/armour/armour';

	const KIND_LABEL: Record<EnhancerMarkupKind, string> = {
		weaponDamage: 'Weapon Damage',
	};

	let markups = $state<EnhancerMarkups | null>(null);
	let readError = $state<string | null>(null);
	let saving = $state<string | null>(null);
	let failure = $state<{ key: string; message: string } | null>(null);
	let drafts = $state<Record<string, string>>({});

	onMount(() => {
		void (async () => {
			try {
				markups = await getEnhancerMarkups();
			} catch (cause) {
				readError = describeError(cause, 'Could not read your enhancer markups.');
			}
		})();
	});

	const kinds = $derived.by(() => {
		const out: { kind: EnhancerMarkupKind; tiers: EnhancerTierMarkup[] }[] = [];
		for (const tier of markups?.tiers ?? []) {
			let row = out.find((entry) => entry.kind === tier.kind);
			if (!row) {
				row = { kind: tier.kind, tiers: [] };
				out.push(row);
			}
			row.tiers.push(tier);
		}
		return out;
	});
	const tierNumbers = $derived(kinds[0]?.tiers.map((tier) => tier.tier) ?? []);

	const keyOf = (tier: EnhancerTierMarkup) => `${tier.kind}-${tier.tier}`;
	const shown = (tier: EnhancerTierMarkup) =>
		drafts[keyOf(tier)] ?? (tier.markupPercent === null ? '' : String(tier.markupPercent));

	async function commit(tier: EnhancerTierMarkup) {
		const key = keyOf(tier);
		const text = drafts[key];
		if (text === undefined) return;
		const value = parseMarkup(text);
		if (value === undefined) {
			failure = { key, message: 'A markup is a number from 100%; leave it empty for TT.' };
			return;
		}
		failure = null;
		if (value === tier.markupPercent) {
			delete drafts[key];
			return;
		}
		saving = key;
		try {
			markups = await setEnhancerMarkup({ kind: tier.kind, tier: tier.tier, markupPercent: value });
			delete drafts[key];
		} catch (cause) {
			failure = { key, message: describeError(cause, 'Could not save the markup.') };
		} finally {
			saving = null;
		}
	}

	function title(tier: EnhancerTierMarkup): string {
		const tt = tier.ttPed === null ? 'TT unknown' : `${tier.ttPed.toFixed(2)} PED TT`;
		const set = tier.setAt ? `; set ${formatDate(tier.setAt)}, applies from then on` : '; no markup set: breaks cost TT';
		return `T${tier.tier}: ${tt}${set}`;
	}
</script>

<div class="space-y-6">
	<div>
		<h2 class="text-lg font-semibold text-text">Enhancers</h2>
		<p class="mt-1 max-w-2xl text-sm text-text-secondary">
			What you paid for enhancers, by tier, as a percent of their TT. A break costs its TT and this
			markup, less the Shrapnel it returns; one with no markup set costs its TT. A new figure applies to
			breaks from then on.
		</p>
	</div>

	{#if readError}
		<ErrorNotice message={readError} />
	{:else if markups === null}
		<div class="py-10 text-center text-sm text-text-tertiary animate-pulse">Loading enhancers...</div>
	{:else}
		<div class="overflow-x-auto">
			<table class="w-full min-w-[44rem] text-sm">
				<thead>
					<tr class="border-b border-border/60">
						<th class="eyebrow py-2 pr-4 text-left font-normal">Kind</th>
						{#each tierNumbers as number (number)}
							<th class="eyebrow px-1 py-2 text-right font-normal">T{number}</th>
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each kinds as row (row.kind)}
						<tr class="border-b border-border/40">
							<th scope="row" class="py-2.5 pr-4 text-left font-normal text-text">{KIND_LABEL[row.kind]}</th>
							{#each row.tiers as tier (keyOf(tier))}
								<td class="px-1 py-2.5 text-right">
									<label class="sr-only" for={`enhancer-${keyOf(tier)}`}>
										Markup paid for {KIND_LABEL[row.kind]} tier {tier.tier}
									</label>
									<input
										id={`enhancer-${keyOf(tier)}`}
										type="text"
										inputmode="decimal"
										class="w-16 rounded-md border bg-surface px-1.5 py-1 text-right tabular-nums text-text
											placeholder:text-text-tertiary focus:border-accent focus:outline-none disabled:opacity-60
											{failure?.key === keyOf(tier) ? 'border-error' : 'border-border/70'}"
										placeholder="TT"
										title={title(tier)}
										value={shown(tier)}
										disabled={saving !== null && saving !== keyOf(tier)}
										oninput={(event) => (drafts[keyOf(tier)] = event.currentTarget.value)}
										onblur={() => void commit(tier)}
										onkeydown={(event) => {
											if (event.key === 'Enter') event.currentTarget.blur();
											if (event.key === 'Escape') {
												delete drafts[keyOf(tier)];
												event.currentTarget.blur();
											}
										}}
									/>
								</td>
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<p class="text-xs text-text-tertiary">Percent of TT. Leave a tier empty to cost its breaks at TT.</p>
		{#if failure}
			<p class="text-xs text-error" role="alert">{failure.message}</p>
		{/if}
	{/if}
</div>
