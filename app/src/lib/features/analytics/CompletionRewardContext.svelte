<script lang="ts">
	import InfoTip from '$lib/components/InfoTip.svelte';
	import RewardMuInfoTip from '$lib/components/RewardMuInfoTip.svelte';
	import type { HuntingRewardStatus } from '$lib/types/analytics';
	import { NO_DATA, formatPed } from '$lib/utils/format';

	let {
		ttPed,
		muPed,
		treatments,
		scope,
	}: {
		/** Confirmed reward TT, exactly as captured. The figure is the answer;
		 * the treatments only explain what it is made of. */
		ttPed: number;
		muPed: number | null;
		treatments: HuntingRewardStatus[];
		/** Which economics scope states this reward. It selects the wording
		 * that says what the figure covers, so it must follow the surface
		 * actually being rendered rather than the component's usual caller. */
		scope: 'activity' | 'session' | 'overall';
	} = $props();

	const SCOPE_SUBJECT: Record<'activity' | 'session' | 'overall', string> = {
		activity: 'this activity',
		session: "this session's completions",
		overall: 'completions across every session in this period',
	};

	/** A completion whose reward evidence is unresolved or predates immutable
	 * capture. It is knowingly excluded from the figure and never estimated,
	 * so it is the one thing the number itself cannot say. */
	const incomplete = $derived(treatments.includes('unverified'));
	const counted = $derived(treatments.filter((treatment) => treatment !== 'unverified'));

	const TREATMENT_DETAIL: Record<Exclude<HuntingRewardStatus, 'none' | 'unverified'>, string> = {
		item: 'Reward items separated from ordinary loot. Their observed TT is counted here and their current market value appears as Reward MU.',
		fixed_liquid: 'A separately recorded liquid reward, counted exactly once.',
		included_in_loot:
			'A payout already present in tracked loot. It is counted there and never added again here.',
		skill: 'A progression payout. It carries no TT and stays outside PED profit.',
		mixed: 'An activity whose own completions were treated in more than one way.',
	};
</script>

<div
	class="flex flex-wrap items-center gap-x-5 gap-y-2 pl-1 text-xs text-text-tertiary"
	data-testid="{scope}-reward-context"
>
	<span class="eyebrow">Completion reward</span>
	<span class="flex items-baseline gap-1.5">
		<span>Reward TT</span>
		<strong class="font-medium tabular-nums text-text-secondary">{formatPed(ttPed)}</strong>
		<span>PED</span>
		<InfoTip align="right" width="w-80" label="How this reward is counted">
			<p class="text-xs font-semibold leading-relaxed text-text">Confirmed reward TT</p>
			<p class="mt-1 text-xs leading-relaxed text-text-secondary">
				What {SCOPE_SUBJECT[scope]} paid, as recorded at the moment of completion.
			</p>
			{#if counted.length}
				<ul class="mt-2 space-y-1.5">
					{#each counted as treatment (treatment)}
						<li class="text-xs leading-relaxed text-text-secondary">
							{TREATMENT_DETAIL[treatment as keyof typeof TREATMENT_DETAIL]}
						</li>
					{/each}
				</ul>
			{/if}
			{#if incomplete}
				<p class="mt-2 text-xs leading-relaxed text-text-secondary">
					Some completions have no usable reward evidence, either unresolved or predating reward
					capture. They contribute nothing to this figure and are never estimated. Any that can
					still be settled are marked on the dashboard's Quests board.
				</p>
			{/if}
		</InfoTip>
	</span>
	<span class="h-3 w-px bg-border-bright" aria-hidden="true"></span>
	<span class="flex items-baseline gap-1.5" data-testid="{scope}-subordinate-reward-mu">
		<span>Reward MU</span>
		<strong
			class={muPed !== null
				? 'font-medium tabular-nums text-text-secondary'
				: 'font-medium tabular-nums text-text-tertiary'}
		>
			{muPed !== null ? formatPed(muPed) : NO_DATA}
		</strong>
		{#if muPed !== null}<span>PED</span>{/if}
		<RewardMuInfoTip />
	</span>
</div>
