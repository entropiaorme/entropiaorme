// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionCostWindow, ProtectionOverview, ProtectionSet } from '$lib/api';

vi.mock('$lib/api', () => ({
	PROTECTION_TOPIC: 'protection:updated',
	archiveProtectionSet: vi.fn(),
	createProtectionSet: vi.fn(),
	getProtectionOverview: vi.fn(),
	restoreProtectionSet: vi.fn(),
	undoProtectionRecording: vi.fn(),
	updateProtectionSet: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
}));

import * as api from '$lib/api';
import ProtectionTab from './ProtectionTab.svelte';
import { createProtectionModel } from './protectionModel.svelte';

const mocked = vi.mocked(api);

function overview(overrides: Partial<ProtectionOverview> = {}): ProtectionOverview {
	return {
		sets: [],
		removedSets: [],
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

beforeAll(() => {
	// happy-dom has no Web Animations; the modal's transition needs one.
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

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
							measured: true,
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

	it('lets only the latest recording of a stream be undone, and shows an undone one as such', async () => {
		await renderTab(
			overview({
				recentCostWindows: [
					recording({ id: '3', undoable: true }),
					recording({ id: '2', supersededAt: 1_756_450_000, costPed: 4 }),
					recording({ id: '1', costPed: 1 }),
				],
			}),
		);
		expect(screen.getAllByRole('button', { name: 'Undo' })).toHaveLength(1);
		expect(screen.getByText(/^Undone /)).toBeTruthy();
		expect(screen.getByText('4.00 PED').className).toContain('line-through');
	});

	it('confirms an undo before taking the cost back', async () => {
		mocked.undoProtectionRecording.mockResolvedValue(overview());
		await renderTab(overview({ recentCostWindows: [recording({ id: '3', undoable: true })] }));
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		expect(screen.getByText('Undo this unlimited repair?')).toBeTruthy();
		const dialog = screen.getByRole('dialog');
		await fireEvent.click(within(dialog).getByRole('button', { name: 'Undo' }));
		expect(mocked.undoProtectionRecording).toHaveBeenCalledWith({ kind: 'recording', windowId: 3 });
	});

	it('opens a recording to show each session and segment it reached', async () => {
		await renderTab(overview({ recentCostWindows: [recording({ id: '3' })] }));
		expect(screen.queryByTestId('protection-recording-detail')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: /Unlimited repair/ }));
		const detail = screen.getByTestId('protection-recording-detail');
		expect(within(detail).getByText('ARIS Dailies')).toBeTruthy();
		expect(within(detail).getByText('Boss room')).toBeTruthy();
		expect(within(detail).getByText('Outside any segment')).toBeTruthy();
		expect(within(detail).getByText('75%')).toBeTruthy();
		expect(within(detail).getByText('6.00 PED')).toBeTruthy();
	});

	it('offers to undo a reading only when it booked nothing', async () => {
		const reading = {
			id: '5',
			setId: '1',
			ttValuePed: 41.2,
			source: 'manual' as const,
			rawText: null,
			observedAt: 1_756_400_000,
			resetReason: null,
			measured: false,
		};
		await renderTab(overview({ sets: [limitedSet({ latestObservation: reading })] }));
		expect(screen.getByRole('button', { name: 'Undo reading' })).toBeTruthy();
	});

	it('lists removed sets on request and restores one', async () => {
		mocked.restoreProtectionSet.mockResolvedValue(overview());
		await renderTab(overview({ removedSets: [limitedSet({ id: '9', name: 'Old Pegasus' })] }));
		expect(screen.queryByText('Old Pegasus')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Show removed sets (1)' }));
		expect(screen.getByText('Old Pegasus')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Restore' }));
		expect(mocked.restoreProtectionSet).toHaveBeenCalledWith('9');
	});
});

function limitedSet(overrides: Partial<ProtectionSet> = {}): ProtectionSet {
	return {
		id: '1',
		kind: 'armour',
		name: 'Pegasus',
		markupPercent: 120,
		latestObservation: null,
		basisLocked: false,
		backlog: { lastRecordedAt: null, sessions: 0, hits: 0 },
		...overrides,
	};
}

function recording(overrides: Partial<ProtectionCostWindow> = {}): ProtectionCostWindow {
	return {
		id: '1',
		kind: 'repair',
		setId: null,
		setName: null,
		consumedTtPed: null,
		markupPercent: null,
		costPed: 8,
		costKnown: true,
		status: 'booked',
		reason: null,
		createdAt: 1_756_400_000,
		supersededAt: null,
		undoable: false,
		allocations: [
			{
				sessionId: 'a',
				sessionName: null,
				definitionName: 'ARIS Dailies',
				startedAt: 1_756_300_000,
				hitCount: 30,
				allocationShare: 0.75,
				costPed: 6,
				contexts: [
					{ label: null, hitCount: 10, costPed: 2 },
					{ label: 'Boss room', hitCount: 20, costPed: 4 },
				],
			},
			{
				sessionId: 'b',
				sessionName: 'Evening run',
				definitionName: null,
				startedAt: 1_756_310_000,
				hitCount: 10,
				allocationShare: 0.25,
				costPed: 2,
				contexts: [{ label: null, hitCount: 10, costPed: 2 }],
			},
		],
		...overrides,
	};
}
