// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { createTableModel } from '$lib/view/tableModel.svelte';
import TreeCuttingPrimaryView from './TreeCuttingPrimaryView.svelte';
import {
	marketOpportunity,
	type TreeCuttingOverall,
	type TreeCuttingSection,
} from './treeCuttingModel.svelte';

const item = {
	name: 'Long Moonleaf Board',
	quantity: 40,
	ttValue: 12,
	sharePct: 100,
	opportunity: marketOpportunity(undefined, 100.6),
	ownMarkupPct: 353.7,
	markupHorizon: 'week',
	effectiveMarkupPct: 353.7,
	markupBasis: 'market' as const,
	floored: false,
	tier: 'liquid' as const,
	salesPed: 320,
	weeklySalesPed: 320,
	marketValuePed: 0,
	manual: null,
	nanocubeMarked: false,
};

function section(
	yieldTier: TreeCuttingSection['yieldTier'],
	overrides: Partial<TreeCuttingSection> = {},
): TreeCuttingSection {
	return {
		yieldTier,
		swings: 20,
		cycled: 10,
		returns: 8,
		lootRate: 0.8,
		muProjectedReturns: 12,
		muRate: 1.2,
		realisedReturns: 11,
		realisedRate: 1.1,
		realisedMarkup: 3,
		items: [item],
		...overrides,
	};
}

function overall(): TreeCuttingOverall {
	return {
		cycled: 20,
		returns: 17,
		lootRate: 0.85,
		muProjectedReturns: 24,
		muRate: 1.2,
		realisedReturns: 22,
		realisedRate: 1.1,
		realisedMarkup: 5,
	};
}

function table(rows: TreeCuttingSection[]) {
	return createTableModel<TreeCuttingSection>({
		rows: () => rows,
		pageSize: Number.MAX_SAFE_INTEGER,
		searchText: (row) => [row.yieldTier],
		initialSort: { key: 'cycled', dir: 'desc' },
	});
}

describe('Tree Cutting economic comparisons', () => {
	it('uses Hunting’s flat row-wise Overall shape without operational panels', () => {
		const sections = [section('huge')];
		render(TreeCuttingPrimaryView, {
			props: {
				overall: overall(),
				table: table(sections),
				selected: null,
				totalCount: sections.length,
				onselect: vi.fn(),
			},
		});

		const surface = screen.getByTestId('tree-cutting-primary-surface');
		for (const boxClass of ['border', 'rounded', 'shadow', 'bg-gradient', 'backdrop-blur']) {
			expect(surface.className).not.toContain(boxClass);
		}
		expect(screen.getByLabelText('Switch tree cutting view (currently Overall)')).not.toBeNull();
		expect(screen.getByText('Board activity')).not.toBeNull();
		expect(
			within(screen.getByTestId('activity-economic-headline')).queryByLabelText(
				'Switch tree cutting view (currently Overall)',
			),
		).toBeNull();
		for (const label of ['TT Net', 'MU Net', 'Realised Net', 'PED cycled', 'TT Rate', 'MU Rate']) {
			expect(screen.getByText(label)).not.toBeNull();
		}
		for (const operation of ['Stock', 'Market', 'History', 'Sell', 'Convert', 'Remove']) {
			expect(screen.queryByRole('button', { name: operation })).toBeNull();
		}
	});

	it('selects rich activity rows from the keyboard-ready overlay', async () => {
		const long = section('huge');
		const unclassified = section('unknown', { cycled: 999, swings: 4 });
		const onselect = vi.fn();
		render(TreeCuttingPrimaryView, {
			props: {
				overall: overall(),
				table: table([unclassified, long]),
				selected: null,
				totalCount: 2,
				onselect,
			},
		});

		const trigger = screen.getByLabelText('Switch tree cutting view (currently Overall)');
		await fireEvent.keyDown(trigger, { key: 'ArrowDown' });
		expect(document.activeElement).toBe(screen.getByLabelText('Filter tree cutting activities'));
		const rows = screen.getAllByRole('menuitem');
		expect(rows.map((row) => row.textContent)).toEqual(
			expect.arrayContaining([
				expect.stringContaining('Overall'),
				expect.stringContaining('Long Boards'),
			]),
		);
		expect(rows.at(-1)?.textContent).toContain('Unclassified');
		expect(rows.at(-1)?.textContent).not.toContain('999.00');
		await fireEvent.click(screen.getByRole('menuitem', { name: /Long Boards/ }));
		expect(onselect).toHaveBeenCalledWith('huge');
	});

	it('replaces Overall in place and progressively discloses activity loot', async () => {
		const selected = section('huge');
		render(TreeCuttingPrimaryView, {
			props: {
				overall: overall(),
				table: table([selected]),
				selected,
				totalCount: 1,
				onselect: vi.fn(),
			},
		});

		expect(
			screen.getByLabelText('Switch tree cutting view (currently Long Boards)'),
		).not.toBeNull();
		expect(
			within(screen.getByTestId('economic-subordinate-cycled')).getByText('10.00'),
		).not.toBeNull();
		expect(screen.queryByText('Long Moonleaf Board')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Show activity loot' }));
		expect(screen.getByText('Long Moonleaf Board')).not.toBeNull();
	});

	it('keeps Unclassified diagnostic and suppresses unsupported economics', () => {
		const selected = section('unknown', { swings: 4, cycled: 999 });
		render(TreeCuttingPrimaryView, {
			props: {
				overall: overall(),
				table: table([selected]),
				selected,
				totalCount: 1,
				onselect: vi.fn(),
			},
		});

		expect(screen.getByText(/4 swings are unclassified/)).not.toBeNull();
		expect(screen.queryByText('TT Net')).toBeNull();
		expect(screen.getByRole('button', { name: 'Why swings can be unclassified' })).not.toBeNull();
	});
});
