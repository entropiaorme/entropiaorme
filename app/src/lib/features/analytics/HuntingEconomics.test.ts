// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ExpectedHuntingEconomics } from '$lib/api';
import { createTableModel, type TableModel } from '$lib/view/tableModel.svelte';
import HuntingPrimaryView from './HuntingPrimaryView.svelte';
import HuntingSessions from './HuntingSessions.svelte';
import type {
	HuntingActivitySection,
	HuntingOverallLine,
	HuntingSessionSection,
} from './huntingModel.svelte';
import TreeCuttingStock from './TreeCuttingStock.svelte';
import { marketOpportunity } from './treeCuttingModel.svelte';

const item = {
	name: 'Animal Muscle Oil',
	quantity: 40,
	ttValue: 12,
	sharePct: 100,
	opportunity: marketOpportunity(undefined, 100.6),
	ownMarkupPct: 130,
	markupHorizon: 'week',
	effectiveMarkupPct: 130,
	markupBasis: 'market' as const,
	floored: false,
	tier: 'liquid' as const,
	salesPed: 5000,
	weeklySalesPed: 5000,
	recommendedPacketTt: 32.67,
	marketValuePed: 15.6,
	manual: null,
	nanocubeMarked: false,
};

const expectedEconomics: ExpectedHuntingEconomics = {
	modelVersion: 'community_v1',
	looterSource: 'three_looter_mean',
	looterLevel: 55,
	expectedLootTt: 94,
	modelledRawTt: 100,
	eligibleOffensiveCost: 100,
	offensiveTtRecovery: 0.94,
	expectedTtRate: 0.94,
	effectiveEfficiency: { status: 'within_model_range', efficiencyPct: 59.29 },
	breakEvenLootMarkup: 1 / 0.94,
	coverage: 1,
	incomplete: false,
	missingBasisPhases: 0,
};

function session(overrides: Partial<HuntingSessionSection> = {}): HuntingSessionSection {
	return {
		definitionId: 7,
		name: 'ARIS Dailies',
		isArchived: false,
		cycled: 100,
		returns: 90,
		lootRate: 0.9,
		expected: null,
		lootItems: [],
		activities: [],
		key: 'definition:7',
		isUnassigned: false,
		confirmedRewardPed: 0,
		reward: { rewardTtPed: 0, rewardMuPed: null, treatments: [] },
		rewardMuRate: null,
		expectedTotalRate: null,
		realisedMarkup: 15,
		muProjectedReturns: 106,
		muRate: 1.06,
		lootMarkupFactor: 106 / 90,
		expectedTtRate: null,
		expectedMarketRate: null,
		realisedReturns: 105,
		realisedRate: 1.05,
		items: [item],
		...overrides,
	};
}

function overall(overrides: Partial<HuntingOverallLine> = {}): HuntingOverallLine {
	return {
		cycled: 180,
		returns: 162,
		lootRate: 0.9,
		reward: { rewardTtPed: 0, rewardMuPed: null, treatments: [] },
		rewardMuRate: null,
		expectedTotalRate: null,
		muProjectedReturns: 190.8,
		muRate: 1.06,
		lootMarkupFactor: 190.8 / 162,
		expectedTtRate: null,
		expectedMarketRate: null,
		expected: null,
		realisedMarkup: 27,
		realisedReturns: 189,
		realisedRate: 1.05,
		realisedOutsidePeriod: 0,
		...overrides,
	};
}

function primaryProps(
	table: TableModel<HuntingSessionSection>,
	selected: HuntingSessionSection | null,
	onselect = vi.fn(),
) {
	return {
		overall: overall(),
		table,
		selected,
		totalCount: table.filtered.length,
		onselect,
	};
}

function activity(overrides: Partial<HuntingActivitySection> = {}): HuntingActivitySection {
	return {
		kind: 'quest',
		label: 'Daily Hunting 1',
		cycled: 100,
		returns: 90,
		lootRate: 0.9,
		expected: null,
		confirmedRewardPed: 0,
		realisedRewardMarkup: 0,
		rewardItems: [],
		rewardMuPed: null,
		rewardMuRate: null,
		expectedTotalRate: null,
		rewardedReturns: 90,
		rewardedRate: 0.9,
		rewardStatus: 'none',
		lootItems: [],
		key: 'quest:daily-hunting-1',
		isUnscoped: false,
		muProjectedReturns: 106,
		lootMarkupFactor: 106 / 90,
		expectedTtRate: null,
		expectedMarketRate: null,
		items: [item],
		variants: [],
		...overrides,
	};
}

describe('Hunting economic comparisons', () => {
	it('presents quiet long-run rates with the offensive-only disclosure at point of use', async () => {
		const row = session({
			expected: expectedEconomics,
			expectedTtRate: 0.94,
			expectedMarketRate: 0.94 * (106 / 90),
		});
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [row],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, { props: primaryProps(table, row) });

		const strip = screen.getByTestId('hunting-expected-economics');
		expect(within(strip).queryByText('Long-run planning')).toBeNull();
		expect(within(strip).getByText('Effective Efficiency')).not.toBeNull();
		expect(within(strip).getByText('59.3%')).not.toBeNull();
		expect(within(strip).getByText('94.0%')).not.toBeNull();
		expect(within(strip).getByText('117.8%')).not.toBeNull();
		expect(within(strip).getByText('110.7%')).not.toBeNull();
		const disclosures = within(strip).getAllByLabelText('What Expected Return includes');
		expect(disclosures).toHaveLength(2);
		await fireEvent.click(disclosures[0]);
		expect(screen.getAllByText('Offensive spend only')).toHaveLength(2);
		expect(screen.getAllByText(/Healing, armour, harvesting/)).toHaveLength(2);
		const effectiveDisclosure = within(strip).getByLabelText('What Effective Efficiency means');
		await fireEvent.click(effectiveDisclosure);
		expect(screen.getByText('Unlimited economic equivalent')).not.toBeNull();
		expect(screen.getByText(/weighted by raw TT/)).not.toBeNull();
		expect(screen.getByText('59.3% effective Efficiency')).not.toBeNull();
		expect(screen.queryByText(/partial historical basis/)).toBeNull();
	});

	it('states a session-level completion reward and carries it into the outlook', async () => {
		const row = session({
			expected: expectedEconomics,
			expectedTtRate: 0.94,
			expectedMarketRate: 0.94 * (106 / 90),
			reward: { rewardTtPed: 12, rewardMuPed: 15, treatments: ['item', 'unverified'] },
			rewardMuRate: 0.15,
			expectedTotalRate: 0.94 * (106 / 90) + 0.15,
		});
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [row],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, { props: primaryProps(table, row) });

		const reward = screen.getByTestId('session-reward-context');
		expect(within(reward).getByText('Reward TT')).not.toBeNull();
		expect(within(reward).getByText('12.00')).not.toBeNull();
		expect(within(reward).getByText('15.00')).not.toBeNull();
		// Knowingly excluded completions are disclosed, never estimated.
		await fireEvent.click(within(reward).getByLabelText('How this reward is counted'));
		expect(screen.getByText(/no usable reward evidence/)).not.toBeNull();

		const strip = screen.getByTestId('hunting-expected-economics');
		expect(within(strip).getByText('Reward MU')).not.toBeNull();
		expect(within(strip).getByText('15.0%')).not.toBeNull();
		expect(within(strip).getByText('125.7%')).not.toBeNull();
	});

	it('states an all-sessions reward without describing it as one session', async () => {
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [session()],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, {
			props: {
				...primaryProps(table, null),
				overall: overall({
					reward: { rewardTtPed: 30, rewardMuPed: 36, treatments: ['item'] },
					rewardMuRate: 0.2,
					expectedTotalRate: 1.1,
				}),
			},
		});

		const reward = screen.getByTestId('overall-reward-context');
		expect(within(reward).getByText('30.00')).not.toBeNull();
		await fireEvent.click(within(reward).getByLabelText('How this reward is counted'));
		// The figure spans every session, so the disclosure must not claim one.
		expect(screen.getByText(/completions across every session in this period/)).not.toBeNull();
		expect(screen.queryByText(/this session's completions/)).toBeNull();
	});

	it('leaves a reward-free session with the loot-only outlook', () => {
		const row = session({
			expected: expectedEconomics,
			expectedTtRate: 0.94,
			expectedMarketRate: 0.94 * (106 / 90),
		});
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [row],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, { props: primaryProps(table, row) });

		expect(screen.queryByTestId('session-reward-context')).toBeNull();
		const strip = screen.getByTestId('hunting-expected-economics');
		expect(within(strip).queryByText('Reward MU')).toBeNull();
		expect(within(strip).getByText('110.7%')).not.toBeNull();
	});

	it('keeps the long-stock search compact and visually discloses overflow', async () => {
		const stock = Array.from({ length: 9 }, (_, index) => ({
			itemName: `Hunting loot ${index + 1}`,
			heldQty: 10,
			heldTt: 5,
			listedQty: 0,
			readings: [],
			opportunity: marketOpportunity(undefined, 100.6),
			markupPct: null,
			markupHorizon: null,
			tier: 'illiquid' as const,
			effectiveMarkupPct: 100.6,
			markupBasis: 'nanocube' as const,
			floored: true,
			salesPed: null,
			weeklySalesPed: null,
			recommendedPacketTt: null,
			marketItem: null,
		}));
		render(TreeCuttingStock, {
			props: {
				stock,
				onsell: vi.fn(),
				onconvert: vi.fn(),
				onremove: vi.fn(),
				onshrapnelconvert: vi.fn(),
			},
		});

		const strip = screen.getByTestId('stock-utility-strip');
		expect(within(strip).getByText('Your Current Stock')).not.toBeNull();
		expect(strip.className).toContain('gap-x-5');
		expect(strip.className).not.toContain('justify-between');
		const search = within(strip).getByLabelText('Find an item');
		expect(search.parentElement?.className).toContain('sm:w-64');
		expect(search.parentElement?.className).not.toContain('sm:ml-auto');
		expect(search.parentElement?.className).not.toContain('sm:w-full');

		const list = screen.getByTestId('stock-scroll-list');
		Object.defineProperty(list, 'scrollHeight', { configurable: true, value: 800 });
		Object.defineProperty(list, 'clientHeight', { configurable: true, value: 300 });
		Object.defineProperty(list, 'scrollTop', { configurable: true, value: 0, writable: true });
		await fireEvent.scroll(list);
		const continuation = screen.getByTestId('stock-scroll-continuation');
		expect(continuation.className).toContain('opacity-100');
		expect(continuation.textContent?.trim()).toBe('');

		list.scrollTop = 500;
		await fireEvent.scroll(list);
		expect(continuation.className).toContain('opacity-0');
	});

	it('offers removal on every holding and deliberate conversion only on Shrapnel', async () => {
		const base = {
			heldQty: 100,
			heldTt: 1,
			listedQty: 0,
			readings: [],
			opportunity: marketOpportunity(undefined, 100.6),
			markupPct: null,
			markupHorizon: null,
			tier: 'illiquid' as const,
			effectiveMarkupPct: 100.6,
			markupBasis: 'nanocube' as const,
			floored: true,
			salesPed: null,
			weeklySalesPed: null,
			recommendedPacketTt: null,
			marketItem: null,
		};
		const onremove = vi.fn();
		const onshrapnelconvert = vi.fn();
		render(TreeCuttingStock, {
			props: {
				stock: [
					{ ...base, itemName: 'Shrapnel' },
					{ ...base, itemName: 'Animal Muscle Oil' },
				],
				onsell: vi.fn(),
				onconvert: vi.fn(),
				onremove,
				onshrapnelconvert,
			},
		});

		expect(screen.getAllByLabelText('Remove')).toHaveLength(2);
		expect(screen.getAllByLabelText('Convert')).toHaveLength(1);
		const rows = screen.getByTestId('stock-scroll-list').querySelectorAll('li');
		expect(
			rows[0]
				?.querySelector('[aria-label="Convert"]')
				?.nextElementSibling?.getAttribute('aria-label'),
		).toBe('Nanocube');
		expect(
			rows[1]
				?.querySelector('[aria-hidden="true"]')
				?.nextElementSibling?.getAttribute('aria-label'),
		).toBe('Nanocube');
		await fireEvent.click(screen.getAllByLabelText('Remove')[1]);
		await fireEvent.click(screen.getByLabelText('Convert'));
		expect(onremove).toHaveBeenCalledWith(
			expect.objectContaining({ itemName: 'Animal Muscle Oil' }),
		);
		expect(onshrapnelconvert).toHaveBeenCalledWith(
			expect.objectContaining({ itemName: 'Shrapnel' }),
		);
	});

	it('uses the Tree Cutting frame for sessions and omits legacy activity statistics', async () => {
		const row = session({
			items: Array.from({ length: 9 }, (_, index) => ({
				...item,
				name: index === 0 ? item.name : `Hunting loot ${index + 1}`,
			})),
		});
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [row],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, { props: primaryProps(table, row) });
		const surface = screen.getByTestId('hunting-primary-surface');
		expect(surface.tagName).toBe('SECTION');
		for (const boxClass of ['border', 'rounded', 'shadow', 'bg-gradient', 'backdrop-blur']) {
			expect(surface.className).not.toContain(boxClass);
		}

		for (const label of ['TT Net', 'MU Net', 'Realised Net']) {
			expect(screen.getByText(label)).not.toBeNull();
		}
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
		expect(screen.queryByLabelText('Find an item')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Show session loot' }));
		expect(screen.getByText('Animal Muscle Oil')).not.toBeNull();
		expect(screen.getByRole('button', { name: 'Hide session loot' })).not.toBeNull();
		expect(screen.queryByLabelText('Find an item')).toBeNull();
		const sessionLoot = screen.getByTestId('session-loot-list');
		const sessionLootHeader = screen.getByTestId('session-loot-header');
		expect(sessionLoot.className).toContain('max-h-[24rem]');
		expect(sessionLoot.className).toContain('overflow-y-auto');
		expect(sessionLootHeader.className).not.toContain('border');
		expect(sessionLootHeader.className).not.toContain('bg-');
		await fireEvent.click(screen.getByRole('button', { name: 'Hide session loot' }));
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
		const trigger = screen.getByLabelText('Switch hunting view (currently ARIS Dailies)');
		expect(trigger.className).not.toContain('border');
		expect(screen.getByTitle('ARIS Dailies').className).toContain('text-text');
		const headline = screen.getByTestId('activity-economic-headline');
		expect(headline.className).toContain('economic-horizon');
		expect(screen.getByText('Session view')).not.toBeNull();
		expect(
			within(headline).queryByLabelText('Switch hunting view (currently ARIS Dailies)'),
		).toBeNull();
		expect(screen.getByTestId('economic-subordinate-cycled').textContent).toContain('100.00');
		const ttRate = within(screen.getByTestId('economic-subordinate-tt-rate')).getByText('90.0%');
		const muRate = within(screen.getByTestId('economic-subordinate-mu-rate')).getByText('106.0%');
		const realisedRate = within(screen.getByTestId('economic-subordinate-realised-rate')).getByText(
			'105.0%',
		);
		expect(ttRate.classList.contains('text-text')).toBe(true);
		expect(ttRate.classList.contains('text-negative')).toBe(false);
		expect(muRate.classList.contains('text-text')).toBe(true);
		expect(muRate.classList.contains('text-positive')).toBe(false);
		expect(realisedRate.classList.contains('text-positive')).toBe(true);
		expect(screen.queryByRole('menu')).toBeNull();
		await fireEvent.click(trigger);
		const menu = screen.getByRole('menu');
		expect(within(menu).getByRole('menuitem', { name: /Overall/ })).not.toBeNull();
		for (const label of ['Cycled', 'MU Rate', 'Realised Rate']) {
			expect(within(menu).getByText(label)).not.toBeNull();
		}
		expect(screen.queryByRole('button', { name: 'Activities' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Loot' })).toBeNull();
		for (const legacy of ['PES/100', 'Kills', 'Runs', 'Instances', 'Duration', 'Net / Kill']) {
			expect(screen.queryByText(legacy)).toBeNull();
		}
	});

	it('pins unassigned sessions last in the picker and suppresses their economic metrics', async () => {
		const unassigned = session({
			definitionId: null,
			name: 'Unassigned',
			key: 'unassigned',
			isUnassigned: true,
			cycled: 999,
		});
		const defined = session();
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [unassigned, defined],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		render(HuntingPrimaryView, { props: primaryProps(table, unassigned) });

		await fireEvent.click(screen.getByLabelText('Switch hunting view (currently Unassigned)'));
		const rows = screen.getAllByRole('menuitem');
		expect(rows).toHaveLength(3);
		expect(rows[0].textContent).toContain('Overall');
		expect(rows[1].textContent).toContain('ARIS Dailies');
		expect(rows[2].textContent).toContain('Unassigned');
		expect(rows[2].textContent).not.toContain('999.00');
		expect(screen.queryByText('TT Net')).toBeNull();
	});

	it('starts on Overall and returns there from the session picker', async () => {
		const row = session();
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [row],
			pageSize: Number.MAX_SAFE_INTEGER,
		});
		const onselect = vi.fn();
		const view = render(HuntingPrimaryView, { props: primaryProps(table, null, onselect) });

		expect(screen.getByLabelText('Switch hunting view (currently Overall)')).not.toBeNull();
		expect(screen.getByText('180.00')).not.toBeNull();
		for (const panel of ['Stock', 'Market', 'History']) {
			expect(screen.queryByRole('button', { name: panel })).toBeNull();
		}
		await fireEvent.click(screen.getByLabelText('Switch hunting view (currently Overall)'));
		await fireEvent.click(screen.getByRole('menuitem', { name: /ARIS Dailies/ }));
		expect(onselect).toHaveBeenCalledWith('definition:7');

		await view.rerender(primaryProps(table, row, onselect));
		await fireEvent.click(screen.getByLabelText('Switch hunting view (currently ARIS Dailies)'));
		await fireEvent.click(screen.getByRole('menuitem', { name: /Overall/ }));
		expect(onselect).toHaveBeenCalledWith(null);
	});

	it('shows activity detail and session loot disclosure together', () => {
		const row = session({ activities: [activity()] });
		render(HuntingSessions, {
			props: { selected: row },
		});

		expect(screen.queryByRole('button', { name: 'Activities' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Loot' })).toBeNull();
		const activityTrigger = screen.getByLabelText(
			'Switch session activity (currently Daily Hunting 1)',
		);
		const sessionLootTrigger = screen.getByRole('button', { name: 'Show session loot' });
		expect(
			sessionLootTrigger.compareDocumentPosition(activityTrigger) &
				Node.DOCUMENT_POSITION_FOLLOWING,
		).not.toBe(0);
		expect(screen.getByRole('button', { name: 'Show activity loot' })).not.toBeNull();
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
	});

	it('offers only session loot when the activity evidence is Unscoped', async () => {
		const row = session({
			activities: [activity({ kind: 'ambient', label: 'Unscoped', isUnscoped: true })],
		});
		render(HuntingSessions, {
			props: { selected: row },
		});

		expect(screen.queryByRole('button', { name: 'Activities' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Loot' })).toBeNull();
		expect(screen.queryByText('Unscoped')).toBeNull();
		expect(screen.queryByText('Animal Muscle Oil')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Show session loot' }));
		expect(screen.getByText('Animal Muscle Oil')).not.toBeNull();
	});

	it('filters rich economic rows and selects from the keyboard-ready overlay', async () => {
		const current = session();
		const alternative = session({
			definitionId: 8,
			name: 'General Hunting',
			key: 'definition:8',
			cycled: 80,
			realisedRate: 0.98,
		});
		const table = createTableModel<HuntingSessionSection>({
			rows: () => [current, alternative],
			pageSize: Number.MAX_SAFE_INTEGER,
			searchText: (row) => [row.name],
			initialSort: { key: 'cycled', dir: 'desc' },
		});
		const onselect = vi.fn();
		render(HuntingPrimaryView, { props: primaryProps(table, current, onselect) });

		const trigger = screen.getByLabelText('Switch hunting view (currently ARIS Dailies)');
		await fireEvent.keyDown(trigger, { key: 'ArrowDown' });
		const filter = screen.getByLabelText('Filter analytics sessions');
		expect(document.activeElement).toBe(filter);
		const menu = screen.getByRole('menu');
		expect(within(menu).getByText('100.00')).not.toBeNull();
		expect(within(menu).getAllByText('105.0%')).toHaveLength(2);

		await fireEvent.input(filter, { target: { value: 'general' } });
		const match = screen.getByRole('menuitem', { name: /General Hunting/ });
		expect(screen.queryByRole('menuitem', { name: /ARIS Dailies/ })).toBeNull();
		await fireEvent.keyDown(filter, { key: 'ArrowDown' });
		expect(document.activeElement).toBe(match);
		await fireEvent.click(match);

		expect(onselect).toHaveBeenCalledWith('definition:8');
		expect(screen.queryByRole('menu')).toBeNull();
	});
});
