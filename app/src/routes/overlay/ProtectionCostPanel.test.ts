// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionCostStep } from '$lib/features/protection/protectionCostFlow';

const api = vi.hoisted(() => ({
	assignSessionProtectionLoadout: vi.fn(),
	pendingProtectionAttribution: vi.fn(),
	confirmProtectionRepair: vi.fn(),
	scanRepairCost: vi.fn(),
	scanTradeTerminalValue: vi.fn(),
	confirmProtectionObservation: vi.fn(),
}));

vi.mock('$lib/api', () => api);

import ProtectionCostPanel from './ProtectionCostPanel.svelte';

const mixedSteps: ProtectionCostStep[] = [
	{
		layer: 'armour',
		method: 'repair',
		name: 'UL armour',
		setId: '1',
		armourSetId: '1',
		plateSetId: null,
		markupPercent: null,
		baselineTtPed: null,
	},
	{
		layer: 'plates',
		method: 'limited',
		name: 'L plates',
		setId: '2',
		armourSetId: null,
		plateSetId: '2',
		markupPercent: 125,
		baselineTtPed: null,
	},
];

beforeEach(() => {
	vi.clearAllMocks();
	api.confirmProtectionRepair.mockResolvedValue({
		costWindow: {
			id: 'window',
			kind: 'repair',
			setId: null,
			armourSetId: '1',
			plateSetId: null,
			consumedTtPed: null,
			markupPercent: null,
			costPed: 1.5,
			costKnown: true,
			status: 'booked',
			reason: null,
			createdAt: 1,
			allocations: [{ sessionId: 's1', hitCount: 1, allocationShare: 1, costPed: 1.5 }],
		},
	});
	api.scanTradeTerminalValue.mockResolvedValue({
		valuePed: 10,
		rawText: '10.00',
		confidence: 0.99,
		error: null,
		calibrated: true,
	});
	api.confirmProtectionObservation.mockResolvedValue({
		observation: {
			id: 'observation',
			setId: '2',
			ttValuePed: 10,
			source: 'ocr',
			rawText: '10.00',
			observedAt: 1,
			resetReason: null,
		},
		reconciliation: null,
		costWindow: null,
	});
});

describe('protection cost panel', () => {
	it('lets the user defer recording without consuming defensive evidence', async () => {
		const onClose = vi.fn();
		render(ProtectionCostPanel, {
			props: { sessionId: 's1', repairOcrEnabled: false, steps: mixedSteps, onClose },
		});

		await fireEvent.click(screen.getByText('Later'));
		expect(onClose).toHaveBeenCalledTimes(1);
		expect(api.confirmProtectionRepair).not.toHaveBeenCalled();
		expect(api.confirmProtectionObservation).not.toHaveBeenCalled();
	});

	it('records mixed protection armour-first and then establishes the limited plate baseline', async () => {
		const onClose = vi.fn();
		render(ProtectionCostPanel, {
			props: {
				sessionId: 's1',
				repairOcrEnabled: false,
				steps: mixedSteps,
				onClose,
			},
		});

		expect(screen.getByText('Armour')).toBeTruthy();
		expect(screen.getByText('Unlimited')).toBeTruthy();
		await fireEvent.click(screen.getByText('Enter manually'));
		await fireEvent.input(screen.getByPlaceholderText('0.00 PED'), {
			target: { value: '1.50' },
		});
		await fireEvent.click(screen.getByText('Confirm'));

		await waitFor(() =>
			expect(api.confirmProtectionRepair).toHaveBeenCalledWith(
				expect.objectContaining({ armourSetId: 1, plateSetId: null, costPed: 1.5 }),
			),
		);
		await fireEvent.click(screen.getByText('Continue to plates'));

		expect(screen.getByText('Plates')).toBeTruthy();
		expect(screen.getByText('Limited')).toBeTruthy();
		await fireEvent.click(screen.getByText('Scan Trade Terminal'));
		await waitFor(() => expect(screen.getByText('Set baseline')).toBeTruthy());
		await fireEvent.click(screen.getByText('Set baseline'));

		await waitFor(() =>
			expect(api.confirmProtectionObservation).toHaveBeenCalledWith(
				expect.objectContaining({ setId: 2, ttValuePed: 10, source: 'ocr' }),
			),
		);
		expect(screen.getByText('Baseline established')).toBeTruthy();
		await fireEvent.click(screen.getByText('Done'));
		expect(onClose).toHaveBeenCalledTimes(1);
	});

	it('names a whole-session setup for a session owed one', async () => {
		api.assignSessionProtectionLoadout.mockResolvedValue({});
		api.pendingProtectionAttribution
			.mockResolvedValueOnce([
				{
					sessionId: 's1',
					name: 'Caly AI Dailies',
					startedAt: 1,
					endedAt: null,
					defenceEventCount: 3,
				},
			])
			.mockResolvedValue([]);
		render(ProtectionCostPanel, {
			props: {
				sessionId: 's1',
				repairOcrEnabled: false,
				steps: [],
				requiresLoadoutSelection: true,
				protection: {
					sets: [],
					loadouts: [
						{
							id: '10',
							name: 'Jaguar and plates',
							armour: { id: '1', name: 'Jaguar', economyKind: 'unlimited', markupPercent: null },
							plates: null,
						},
					],
					activeLoadoutId: null,
					recentReconciliations: [],
					recentCostWindows: [],
				},
				onClose: vi.fn(),
			},
		});

		await waitFor(() => expect(screen.getByRole('combobox')).toBeTruthy());
		await fireEvent.change(screen.getByRole('combobox'), { target: { value: '10' } });
		await waitFor(() =>
			expect(api.assignSessionProtectionLoadout).toHaveBeenCalledWith('s1', '10'),
		);
		// Naming the setup takes the session off the owed list, which must not
		// read as the hits it just attributed having gone away.
		await waitFor(() => expect(screen.getByText(/Recording under/)).toBeTruthy());
		expect(screen.queryByText(/hits/)).toBeNull();
	});

	it('waits for a named setup before it will carry one forward', async () => {
		// A session that has taken no hits is owed no setup, so it is absent
		// from the pending list. Nothing may be carried forward on its behalf
		// until a setup is actually named.
		api.assignSessionProtectionLoadout.mockResolvedValue({});
		api.pendingProtectionAttribution.mockResolvedValue([]);
		render(ProtectionCostPanel, {
			props: {
				sessionId: 's1',
				repairOcrEnabled: false,
				steps: [],
				requiresLoadoutSelection: true,
				protection: {
					sets: [],
					loadouts: [
						{
							id: '10',
							name: 'Jaguar and plates',
							armour: { id: '1', name: 'Jaguar', economyKind: 'unlimited', markupPercent: null },
							plates: null,
						},
					],
					activeLoadoutId: null,
					recentReconciliations: [],
					recentCostWindows: [],
				},
				onClose: vi.fn(),
			},
		});

		await waitFor(() => expect(screen.getByRole('combobox')).toBeTruthy());
		expect(screen.queryByText('Continue')).toBeNull();
		expect(screen.queryByText('Armour setup saved')).toBeNull();

		await fireEvent.change(screen.getByRole('combobox'), { target: { value: '10' } });
		await waitFor(() =>
			expect(api.assignSessionProtectionLoadout).toHaveBeenCalledWith('s1', '10'),
		);
		await fireEvent.click(await screen.findByText('Continue'));
		expect(screen.queryByText('Choose armour setup')).toBeNull();
	});

	it('offers a session left without a setup alongside the running one', async () => {
		// A session put off for later carries unattributed evidence, which no
		// cost naming an armour set can settle. If the recording surface does
		// not say so, it stays unpriced silently.
		api.assignSessionProtectionLoadout.mockResolvedValue({});
		api.pendingProtectionAttribution.mockResolvedValue([
			{
				sessionId: 's1',
				name: 'Caly AI Dailies',
				startedAt: 2,
				endedAt: null,
				defenceEventCount: 3,
			},
			{
				sessionId: 'postponed',
				name: 'Caly AI Dailies',
				startedAt: 1,
				endedAt: 2,
				defenceEventCount: 123,
			},
		]);
		render(ProtectionCostPanel, {
			props: {
				sessionId: 's1',
				repairOcrEnabled: false,
				steps: [],
				requiresLoadoutSelection: true,
				protection: {
					sets: [],
					loadouts: [
						{
							id: '10',
							name: 'Hyperion + 5B',
							armour: { id: '1', name: 'Hyperion', economyKind: 'unlimited', markupPercent: null },
							plates: null,
						},
					],
					activeLoadoutId: null,
					recentReconciliations: [],
					recentCostWindows: [],
				},
				onClose: vi.fn(),
			},
		});

		await waitFor(() => expect(screen.getAllByRole('combobox')).toHaveLength(2));
		expect(screen.getByText('This session')).toBeTruthy();
		expect(screen.getByText(/123 hits/)).toBeTruthy();

		await fireEvent.change(screen.getByLabelText('Armour setup for Caly AI Dailies'), {
			target: { value: '10' },
		});
		await waitFor(() =>
			expect(api.assignSessionProtectionLoadout).toHaveBeenCalledWith('postponed', '10'),
		);
	});

	it('accepts a lower limited-armour reading without an implementation assertion', async () => {
		render(ProtectionCostPanel, {
			props: {
				sessionId: 's1',
				repairOcrEnabled: false,
				steps: [{ ...mixedSteps[1], baselineTtPed: 10 }],
				onClose: vi.fn(),
			},
		});

		await fireEvent.click(screen.getByText('Enter manually'));
		await fireEvent.input(screen.getByPlaceholderText('0.00 PED'), { target: { value: '9' } });
		expect(screen.queryByText(/This set was not replaced/)).toBeNull();
		await fireEvent.click(screen.getByText('Confirm'));
		await waitFor(() =>
			expect(api.confirmProtectionObservation).toHaveBeenCalledWith(
				expect.objectContaining({ setId: 2, ttValuePed: 9 }),
			),
		);
	});
});
