// @vitest-environment happy-dom

import { render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionOverview } from '$lib/api';

vi.mock('$lib/api', () => ({
	archiveProtectionSet: vi.fn(),
	createProtectionSet: vi.fn(),
	getProtectionOverview: vi.fn(),
	updateProtectionSet: vi.fn(),
}));

import * as api from '$lib/api';
import ProtectionTab from './ProtectionTab.svelte';
import { createProtectionModel } from './protectionModel.svelte';

const mocked = vi.mocked(api);

function overview(overrides: Partial<ProtectionOverview> = {}): ProtectionOverview {
	return {
		sets: [],
		unlimited: { lastRecordedAt: null, sessions: 0, hits: 0 },
		recentCostWindows: [],
		unrecorded: { sessions: 0, hits: 0 },
		...overrides,
	};
}

async function renderTab(data: ProtectionOverview) {
	mocked.getProtectionOverview.mockResolvedValue(data);
	const model = createProtectionModel();
	await model.load();
	render(ProtectionTab, { props: { model } });
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('the Equipment armour tab', () => {
	it('shows the unlimited pool even with no limited set configured', async () => {
		await renderTab(
			overview({ unlimited: { lastRecordedAt: 1_756_400_000, sessions: 3, hits: 262 } }),
		);
		const unlimited = screen.getByTestId('unlimited-backlog');
		expect(within(unlimited).getByText(/Last repair/)).toBeTruthy();
		expect(within(unlimited).getByText('3 sessions, 262 hits since')).toBeTruthy();
		expect(screen.getAllByText('None yet.')).toHaveLength(2);
	});

	it('says when no repair has been recorded yet', async () => {
		await renderTab(overview({ unlimited: { lastRecordedAt: null, sessions: 2, hits: 40 } }));
		const unlimited = screen.getByTestId('unlimited-backlog');
		expect(within(unlimited).getByText('No repair recorded yet')).toBeTruthy();
		expect(within(unlimited).getByText('2 sessions with hits so far')).toBeTruthy();
	});

	it('shows each limited set with its reading and the play since it', async () => {
		await renderTab(
			overview({
				sets: [
					{
						id: '1',
						kind: 'armour',
						name: 'Pegasus',
						markupPercent: 120.65,
						latestObservation: {
							id: '5',
							setId: '1',
							ttValuePed: 41.2,
							source: 'ocr',
							rawText: '41.20',
							observedAt: 1_756_400_000,
							resetReason: null,
						},
						basisLocked: true,
						backlog: { lastRecordedAt: 1_756_400_000, sessions: 1, hits: 90 },
					},
				],
			}),
		);
		expect(screen.getByText('Pegasus')).toBeTruthy();
		expect(screen.getByText('120.65% average MU')).toBeTruthy();
		expect(screen.getByText('41.20 PED')).toBeTruthy();
		expect(screen.getByText(/1 session, 90 hits since/)).toBeTruthy();
	});

	it('states how much play has no armour cost recorded', async () => {
		await renderTab(overview({ unrecorded: { sessions: 6, hits: 444 } }));
		expect(
			screen.getByText(/6 sessions with 444 hits have no armour cost recorded yet/),
		).toBeTruthy();
	});

	it('offers no loadouts and no unlimited set to create', async () => {
		await renderTab(overview());
		expect(screen.queryByText(/loadout/i)).toBeNull();
		expect(screen.getByText('Add armour set')).toBeTruthy();
		expect(screen.getByText('Add plate set')).toBeTruthy();
	});
});
