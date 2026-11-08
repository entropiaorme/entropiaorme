// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import { NO_DATA } from '$lib/utils/format';
import HuntingSessionActivities from './HuntingSessionActivities.svelte';
import type { HuntingActivitySection } from './huntingModel.svelte';
import { marketOpportunity } from './treeCuttingModel.svelte';

const expectedEconomics = {
	modelVersion: 'community_v1',
	looterSource: 'three_looter_mean',
	looterLevel: 50,
	expectedLootTt: 94,
	modelledRawTt: 100,
	eligibleOffensiveCost: 100,
	offensiveTtRecovery: 0.94,
	expectedTtRate: 0.94,
	effectiveEfficiency: { status: 'within_model_range' as const, efficiencyPct: 57.14 },
	breakEvenLootMarkup: 1 / 0.94,
	coverage: 1,
	incomplete: false,
	missingBasisPhases: 0,
};

function activity(overrides: Partial<HuntingActivitySection> = {}): HuntingActivitySection {
	return {
		kind: 'quest',
		label: 'Daily Hunting 1',
		cycled: 100,
		returns: 90,
		lootRate: 0.9,
		expected: null,
		confirmedRewardPed: 15,
		realisedRewardMarkup: 0,
		rewardItems: [{ itemName: 'Animal Muscle Oil', quantity: 50, valuePed: 15 }],
		rewardMuPed: 18,
		rewardMuRate: null,
		expectedTotalRate: null,
		rewardedReturns: 105,
		rewardedRate: 1.05,
		rewardStatus: 'fixed_liquid',
		lootItems: [],
		key: 'quest:daily-1',
		isUnscoped: false,
		muProjectedReturns: 104,
		lootMarkupFactor: null,
		expectedTtRate: null,
		expectedMarketRate: null,
		items: [
			{
				name: 'Animal Muscle Oil',
				quantity: 40,
				ttValue: 90,
				sharePct: 100,
				opportunity: marketOpportunity(undefined, 100.6),
				ownMarkupPct: 120,
				markupHorizon: 'week',
				effectiveMarkupPct: 120,
				markupBasis: 'market',
				floored: false,
				tier: 'liquid',
				salesPed: 5000,
				weeklySalesPed: 5000,
				marketValuePed: 0,
				manual: null,
				nanocubeMarked: false,
			},
		],
		variants: [],
		...overrides,
	};
}

describe('HuntingSessionActivities', () => {
	it('opens the first activity detail and switches through the searchable economic picker', async () => {
		const alternative = activity({
			label: 'Bonus Challenge',
			key: 'quest:bonus',
			cycled: 80,
			returns: 76,
			lootRate: 0.95,
			confirmedRewardPed: 0,
			rewardedReturns: 76,
			rewardedRate: 0.95,
			rewardStatus: 'none',
		});
		render(HuntingSessionActivities, {
			props: { activities: [activity(), alternative], marketAvailable: true },
		});

		const trigger = screen.getByLabelText('Switch session activity (currently Daily Hunting 1)');
		const activityName = screen.getByTitle('Daily Hunting 1');
		expect(activityName.className).toContain('break-words');
		expect(activityName.className).not.toContain('truncate');
		const grid = screen.getByTestId('activity-economic-grid');
		expect(grid.className).toContain('economic-horizon');
		for (const label of [
			'TT Net',
			'MU Net',
			'Realised Net',
			'TT Rate',
			'MU Rate',
			'Realised Rate',
		]) {
			expect(within(grid).getByText(label)).not.toBeNull();
		}
		const reward = screen.getByTestId('activity-reward-context');
		expect(within(reward).getByText('Completion reward')).not.toBeNull();
		expect(within(reward).getByText('Reward TT')).not.toBeNull();
		expect(within(reward).getByText('Reward MU')).not.toBeNull();
		expect(within(reward).getByText('15.00')).not.toBeNull();
		expect(within(reward).getByText('18.00')).not.toBeNull();
		expect(within(grid).queryByText('Cycled')).toBeNull();
		expect(within(grid).getByText('+4.00').className).not.toContain('text-positive');
		expect(within(grid).getByText('+5.00').className).toContain('text-positive');
		expect(within(grid).getByText('104.0%').className).not.toContain('text-positive');
		expect(within(grid).getByText('105.0%').className).toContain('text-positive');
		expect(screen.queryByText('Rewarded Net')).toBeNull();
		expect(screen.queryByText('At current market')).toBeNull();
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
		expect(screen.queryByLabelText('Find an item')).toBeNull();
		const lootTrigger = screen.getByRole('button', { name: 'Show activity loot' });
		expect(lootTrigger.parentElement?.className).not.toContain('border-t');
		await fireEvent.click(lootTrigger);
		expect(screen.getByText('Animal Muscle Oil')).not.toBeNull();
		expect(screen.getByRole('button', { name: 'Hide activity loot' })).not.toBeNull();
		expect(screen.queryByLabelText('Find an item')).toBeNull();
		const activityLoot = screen.getByTestId('activity-loot-list');
		const activityLootHeader = screen.getByTestId('activity-loot-header');
		expect(activityLoot.className).toContain('max-h-[24rem]');
		expect(activityLoot.className).toContain('overflow-y-auto');
		expect(activityLootHeader.className).not.toContain('border');
		expect(activityLootHeader.className).not.toContain('bg-');
		expect(screen.queryByRole('menu')).toBeNull();

		await fireEvent.keyDown(trigger, { key: 'ArrowDown' });
		const filter = screen.getByLabelText('Filter session activities');
		expect(document.activeElement).toBe(filter);
		const menu = screen.getByRole('menu');
		for (const label of ['Cycled', 'TT Rate', 'Rewarded Rate']) {
			expect(within(menu).getByText(label)).not.toBeNull();
		}
		expect(within(menu).getByText('90.0%')).not.toBeNull();
		expect(within(menu).getByText('105.0%')).not.toBeNull();

		await fireEvent.input(filter, { target: { value: 'bonus' } });
		const match = screen.getByRole('menuitem', { name: /Bonus Challenge/ });
		expect(screen.queryByRole('menuitem', { name: /Daily Hunting 1/ })).toBeNull();
		await fireEvent.keyDown(filter, { key: 'ArrowDown' });
		expect(document.activeElement).toBe(match);
		await fireEvent.click(match);

		expect(
			screen.getByLabelText('Switch session activity (currently Bonus Challenge)'),
		).not.toBeNull();
		expect(screen.getByRole('button', { name: 'Show activity loot' })).not.toBeNull();
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
		expect(screen.queryByRole('menu')).toBeNull();
	});

	it('states tracked completion loot as zero confirmed TT without adding it again', async () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [
					activity({
						returns: 105,
						lootRate: 1.05,
						confirmedRewardPed: 0,
						rewardMuPed: null,
						rewardedReturns: 105,
						rewardedRate: 1.05,
						rewardStatus: 'included_in_loot',
					}),
				],
				marketAvailable: true,
			},
		});

		// The figure is the answer; the provenance that makes it zero is
		// available on demand rather than occupying the value slot.
		const reward = screen.getByTestId('activity-reward-context');
		expect(within(reward).queryByText('In loot')).toBeNull();
		expect(within(reward).getByText('0.00')).not.toBeNull();
		await fireEvent.click(within(reward).getByLabelText('How this reward is counted'));
		expect(screen.getByText(/already present in tracked loot/)).not.toBeNull();
		const nets = screen.getAllByText('+5.00');
		expect(nets).toHaveLength(2);
		expect(nets.filter((value) => value.classList.contains('text-positive'))).toHaveLength(1);
	});

	it('carries the completion reward into the long-run outlook as its own term', () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [
					activity({
						expected: expectedEconomics,
						lootMarkupFactor: 1.018,
						expectedTtRate: 0.936,
						expectedMarketRate: 0.936 * 1.018,
						rewardMuRate: 0.036,
						expectedTotalRate: 0.936 * 1.018 + 0.036,
					}),
				],
				marketAvailable: true,
			},
		});

		const strip = screen.getByTestId('activity-expected-economics');
		for (const label of ['Expected Return', 'Loot MU', 'Reward MU', 'Expected + MU']) {
			expect(within(strip).getByText(label)).not.toBeNull();
		}
		expect(within(strip).getByText('93.6%')).not.toBeNull();
		expect(within(strip).getByText('101.8%')).not.toBeNull();
		expect(within(strip).getByText('3.6%')).not.toBeNull();
		// The result is the loot product plus the reward, not the product alone.
		expect(within(strip).getByText('98.9%')).not.toBeNull();
		expect(within(strip).queryByText('95.3%')).toBeNull();
	});

	it('omits the reward term entirely where no completion reward exists', () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [
					activity({
						expected: expectedEconomics,
						lootMarkupFactor: 1.018,
						expectedTtRate: 0.936,
						expectedMarketRate: 0.936 * 1.018,
						rewardMuRate: null,
						expectedTotalRate: null,
						rewardStatus: 'none',
					}),
				],
				marketAvailable: true,
			},
		});

		const strip = screen.getByTestId('activity-expected-economics');
		expect(within(strip).queryByText('Reward MU')).toBeNull();
		// No zero figure stands in for an absent reward.
		expect(within(strip).queryByText('0.0%')).toBeNull();
		expect(within(strip).getByText('95.3%')).not.toBeNull();
	});

	it('shows neutral activity-level expected economics from ordinary loot only', () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [
					activity({
						expected: expectedEconomics,
						lootMarkupFactor: 1.3,
						expectedTtRate: 0.94,
						expectedMarketRate: 1.222,
					}),
				],
				marketAvailable: true,
			},
		});

		const strip = screen.getByTestId('activity-expected-economics');
		for (const label of ['Effective Efficiency', 'Loot MU', 'Expected Return', 'Expected + MU']) {
			expect(within(strip).getByText(label)).not.toBeNull();
		}
		for (const value of ['57.1%', '130.0%', '94.0%', '122.2%']) {
			const displayed = within(strip).getByText(value);
			expect(displayed.className).not.toContain('text-positive');
			expect(displayed.className).not.toContain('text-negative');
		}
	});

	it('shows a separated zero-TT item reward without calling it ordinary loot', () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [
					activity({
						confirmedRewardPed: 0,
						rewardItems: [{ itemName: 'Hyperion Daily Voucher', quantity: 20, valuePed: 0 }],
						rewardMuPed: 40,
						rewardStatus: 'item',
					}),
				],
				marketAvailable: true,
			},
		});

		expect(screen.getByText('Reward TT')).not.toBeNull();
		expect(screen.getByText('0.00')).not.toBeNull();
		expect(screen.getByText('40.00')).not.toBeNull();
		expect(screen.queryByText('In loot')).toBeNull();
	});

	it('suppresses an unverified historical reward instead of reading current quest settings', () => {
		render(HuntingSessionActivities, {
			props: {
				activities: [activity({ rewardStatus: 'unverified', key: 'quest:legacy' })],
				marketAvailable: false,
			},
		});

		expect(screen.getByText('Realised Net')).not.toBeNull();
		expect(screen.getByText('Realised Rate')).not.toBeNull();
		expect(screen.getAllByText(NO_DATA).length).toBeGreaterThan(0);
	});

	it('keeps variants in the picker hierarchy and opens their detail directly', async () => {
		const variant = activity({
			label: 'Daily Hunting 1 + Bonus',
			key: 'quest:daily-1/variant:bonus',
			variants: [],
		});
		render(HuntingSessionActivities, {
			props: {
				activities: [activity({ variants: [variant] })],
				marketAvailable: true,
			},
		});

		await fireEvent.click(
			screen.getByLabelText('Switch session activity (currently Daily Hunting 1)'),
		);
		const variantRow = screen.getByRole('menuitem', { name: /Daily Hunting 1 \+ Bonus/ });
		expect(variantRow.textContent).toContain('↳');
		await fireEvent.click(variantRow);
		expect(
			screen.getByLabelText('Switch session activity (currently Daily Hunting 1 + Bonus)'),
		).not.toBeNull();
	});

	it('falls back to the first declared activity when the selected session changes', async () => {
		const firstSession = [activity(), activity({ label: 'Bonus Challenge', key: 'quest:bonus' })];
		const view = render(HuntingSessionActivities, {
			props: { activities: firstSession, marketAvailable: true },
		});

		await fireEvent.click(
			screen.getByLabelText('Switch session activity (currently Daily Hunting 1)'),
		);
		await fireEvent.click(screen.getByRole('menuitem', { name: /Bonus Challenge/ }));
		expect(
			screen.getByLabelText('Switch session activity (currently Bonus Challenge)'),
		).not.toBeNull();

		await view.rerender({
			activities: [activity({ label: 'New Session Activity', key: 'quest:new-session' })],
			marketAvailable: true,
		});
		expect(
			screen.getByLabelText('Switch session activity (currently New Session Activity)'),
		).not.toBeNull();
	});
});
