// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { HealingActivationRow, HealingSessionSummary } from '$lib/api/healing';
import type { SessionDetail } from '$lib/types/tracking';
import HealingEvidence from './HealingEvidence.svelte';
import { createHealingReviewModel } from './healingReviewModel.svelte';

vi.mock('$lib/api', () => ({
	correctHealing: vi.fn(),
	getHealingCorrectionTools: vi.fn(),
	getHealingOutputs: vi.fn(),
	undoHealingCorrection: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function activation(overrides: Partial<HealingActivationRow> = {}): HealingActivationRow {
	return {
		id: 'a1',
		toolName: 'Restoration Chip',
		observedAt: 1000,
		cost: 0.04,
		provenance: 'direct',
		effectUntil: 1020,
		outputCount: 5,
		amount: 30,
		superseded: false,
		correction: null,
		...overrides,
	};
}

function detail(overrides: Partial<HealingSessionSummary> = {}): SessionDetail {
	return {
		sessionId: 's1',
		healing: {
			correctable: true,
			activations: [activation()],
			activationCount: 1,
			outputCount: 7,
			directOutputs: 1,
			effectOutputs: 4,
			passiveOutputs: 0,
			unattributedOutputs: 2,
			...overrides,
		},
	} as SessionDetail;
}

function mount(initial: SessionDetail) {
	let current = initial;
	const model = createHealingReviewModel({
		detail: () => current,
		apply: (fresh) => {
			current = fresh;
		},
	});
	return render(HealingEvidence, { props: { detail: initial, model } });
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('healing evidence', () => {
	it('tallies uses apart from ticks and offers a use to be taken back', async () => {
		mocked.correctHealing.mockResolvedValue(detail());
		mount(detail());
		expect(screen.getByText('1 paid use · 4 effect ticks · 2 unresolved')).toBeTruthy();
		const row = screen.getByTestId('healing-activation');
		expect(within(row).getByText('30 HP')).toBeTruthy();
		expect(within(row).getByText('Effect window')).toBeTruthy();

		await fireEvent.click(within(row).getByRole('button', { name: /^Correct Restoration Chip/ }));
		await fireEvent.click(screen.getByRole('menuitem', { name: 'Not a paid use' }));
		expect(mocked.correctHealing).toHaveBeenCalledWith({ kind: 'notPaidUse', activationId: 'a1' });
	});

	it('shows a taken-back use struck through with Undo', async () => {
		mocked.undoHealingCorrection.mockResolvedValue(detail());
		mount(
			detail({
				activationCount: 0,
				activations: [
					activation({ superseded: true, correction: { id: 'c1', kind: 'notPaidUse' } }),
				],
			}),
		);
		const row = screen.getByTestId('healing-activation');
		expect(within(row).getByText('Not a paid use')).toBeTruthy();
		expect(within(row).getByText('0.04 PED').className).toContain('line-through');
		await fireEvent.click(within(row).getByRole('button', { name: /^Undo the correction/ }));
		expect(mocked.undoHealingCorrection).toHaveBeenCalledWith('c1');
	});

	it('reviews uncosted heals and bills one to a chosen item, fitting items first', async () => {
		mocked.getHealingOutputs.mockResolvedValue({
			outputs: [
				{
					id: 'u1',
					observedAt: 1100,
					amount: 80,
					classification: 'unattributed',
					reason: 'no compatible paid-healer activation',
					toolName: null,
					correction: null,
					correctable: true,
				},
			],
			total: 1,
		});
		mocked.getHealingCorrectionTools.mockResolvedValue([
			{ equipmentId: 9, name: 'FAP', costPerUsePed: 0.03, fits: true },
			{ equipmentId: 8, name: 'Restoration Chip', costPerUsePed: 0.04, fits: false },
		]);
		mocked.correctHealing.mockResolvedValue(detail());
		mount(detail());

		await fireEvent.click(screen.getByRole('button', { name: 'Review uncosted heals (6)' }));
		const row = await screen.findByTestId('healing-uncosted-output');
		expect(within(row).getByText('No paid use explains it')).toBeTruthy();
		await fireEvent.click(within(row).getByRole('button', { name: /as a paid use$/ }));
		const items = await screen.findAllByRole('menuitem');
		expect(items.map((item) => item.getAttribute('aria-label'))).toEqual([
			'FAP · 0.03 PED',
			'Restoration Chip · 0.04 PED',
		]);
		await fireEvent.click(items[0]);
		expect(mocked.correctHealing).toHaveBeenCalledWith({
			kind: 'paidUse',
			outputId: 'u1',
			equipmentId: 9,
		});
	});

	it('offers no corrections while the session is still running', () => {
		mount(detail({ correctable: false }));
		expect(screen.queryByRole('button', { name: /^Correct/ })).toBeNull();
		expect(screen.queryByRole('button', { name: /Review uncosted heals/ })).toBeNull();
	});
});
