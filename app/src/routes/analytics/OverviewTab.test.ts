// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { OverviewStats } from '$lib/types/analytics';

vi.mock('$lib/api', () => ({
	getAnalyticsOverview: vi.fn(),
}));

import * as api from '$lib/api';
import OverviewTab from './OverviewTab.svelte';

const mocked = vi.mocked(api);

const CYCLED = {
	weapon: 700,
	healing: 150,
	enhancer: 100,
	armour: 50,
	dangling: 0,
	harvest: 0,
	consumables: 0,
};

function overview(
	overhead = { total: 0, travel: 0, fieldLogistics: 0, unclassified: 0, betweenSessions: 0 },
) {
	return {
		totalReturnRate: 0.85,
		trend: 'stable',
		returnsBreakdown: {
			lootTt: 850,
			questItemTt: 25,
			pes: 12.5,
			codexPes: 3,
			questPes: 1.5,
			ledger: { item_sale: 120, codex: 5 },
		},
		lossesBreakdown: {
			trackingCost: 1000,
			cycledBreakdown: { ...CYCLED },
			overhead,
			ledger: { repair: 20 },
		},
		totalGains: 1000,
		totalLosses: 1020,
		timeline: [],
		monthlyBreakdown: [],
	} as OverviewStats;
}

/** The figure in a row of the cumulative breakdown, by its label. */
function breakdownFigure(label: string): string {
	const row = screen.getByText(label, { selector: 'td' }).closest('tr');
	if (!row) throw new Error(`no ${label} row`);
	return row.lastElementChild?.textContent?.trim() ?? '';
}

/** The donut card's figure under a heading (Returns or Costs). */
function donutFigure(label: string): string {
	const figure = screen
		.getAllByText(label, { selector: 'span' })
		.map((heading) => heading.nextElementSibling)
		.find((next) => next?.textContent?.includes('PED'));
	if (!figure) throw new Error(`no ${label} figure`);
	return figure.firstChild?.textContent?.trim() ?? '';
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe("the Overview's losses", () => {
	it('sets an Overhead band beside Cycled, by category, leaving Cycled as it was', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({ total: 6, travel: 4, fieldLogistics: 0, unclassified: 2, betweenSessions: 0 }),
		);
		render(OverviewTab);
		const band = await screen.findByTestId('overhead-band');
		expect(band.textContent?.trim()).toBe('Overhead');
		expect(band.getAttribute('title')).toBe('Always included: travel 4.00, unclassified 2.00');
		const cycled = screen.getByText('Cycled', { selector: 'span' });
		expect(cycled.getAttribute('title')).toBe(
			'Always included: weapon, healing, enhancers, armour',
		);
		// Beside Cycled, never inside it.
		expect(cycled.nextElementSibling).toBe(band);
		expect(cycled.contains(band)).toBe(false);
	});

	it('has no Overhead band in a window without overhead', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(overview());
		render(OverviewTab);
		await screen.findByText('Global Returns');
		expect(screen.queryByTestId('overhead-band')).toBeNull();
	});

	it('totals the cumulative breakdown as the donut does, with Overhead beside Cycled', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({ total: 6, travel: 4, fieldLogistics: 2, unclassified: 0, betweenSessions: 0 }),
		);
		render(OverviewTab);
		await fireEvent.click(await screen.findByRole('button', { name: /Cumulative Breakdown/ }));

		// Costs are Cycled, the overhead and the ledger losses: the donut's.
		expect(breakdownFigure('Costs')).toBe('1026.00');
		expect(breakdownFigure('Costs')).toBe(donutFigure('Costs'));
		expect(breakdownFigure('Returns')).toBe(donutFigure('Returns'));
		expect(breakdownFigure('Cycled')).toBe('1000.00');
		expect(breakdownFigure('Overhead')).toBe('6.00');
		expect(breakdownFigure('Travel')).toBe('4.00');
		expect(breakdownFigure('Field logistics')).toBe('2.00');
		expect(screen.queryByText('Unclassified', { selector: 'td' })).toBeNull();
		// Net is what the totals leave: 995 returns less 1026 costs.
		const net = screen.getByText('Net', { selector: 'td' }).closest('tr');
		expect(within(net as HTMLElement).getByText(/-31\.00/)).toBeTruthy();
		expect(screen.queryByText('Of which between sessions', { selector: 'td' })).toBeNull();
	});

	it('says how much of the overhead was billed between sessions', async () => {
		mocked.getAnalyticsOverview.mockResolvedValue(
			overview({ total: 6, travel: 6, fieldLogistics: 0, unclassified: 0, betweenSessions: 4 }),
		);
		render(OverviewTab);
		await fireEvent.click(await screen.findByRole('button', { name: /Cumulative Breakdown/ }));
		expect(breakdownFigure('Overhead')).toBe('6.00');
		expect(breakdownFigure('Of which between sessions')).toBe('4.00');
	});
});
