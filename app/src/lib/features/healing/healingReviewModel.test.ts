import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { HealingOutput, HealingSessionSummary } from '$lib/api/healing';
import type { SessionDetail } from '$lib/types/tracking';
import { createHealingReviewModel, OUTPUT_PAGE } from './healingReviewModel.svelte';

vi.mock('$lib/api', () => ({
	correctHealing: vi.fn(),
	getHealingCorrectionTools: vi.fn(),
	getHealingOutputs: vi.fn(),
	undoHealingCorrection: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function healing(overrides: Partial<HealingSessionSummary> = {}): HealingSessionSummary {
	return {
		correctable: true,
		activations: [],
		activationCount: 1,
		outputCount: 4,
		directOutputs: 1,
		effectOutputs: 2,
		passiveOutputs: 0,
		unattributedOutputs: 1,
		...overrides,
	};
}

function detail(overrides: Partial<HealingSessionSummary> = {}): SessionDetail {
	return { sessionId: 's1', healing: healing(overrides) } as SessionDetail;
}

function output(id: string, overrides: Partial<HealingOutput> = {}): HealingOutput {
	return {
		id,
		observedAt: 1000,
		amount: 10,
		classification: 'unattributed',
		reason: 'no compatible paid-healer activation',
		toolName: null,
		correction: null,
		correctable: true,
		...overrides,
	};
}

function harness(initial = detail()) {
	let current = initial;
	const model = createHealingReviewModel({
		detail: () => current,
		apply: (fresh) => {
			current = fresh;
		},
	});
	return { model, current: () => current };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('the healing review model', () => {
	it('applies a correction’s refreshed detail and re-reads the open review in place', async () => {
		const { model, current } = harness();
		mocked.getHealingOutputs.mockResolvedValue({ outputs: [output('u1')], total: 1 });
		await model.toggleReview();
		expect(mocked.getHealingOutputs).toHaveBeenLastCalledWith('s1', 'unattributed', 0, OUTPUT_PAGE);

		const fresh = detail({ unattributedOutputs: 0, activationCount: 2 });
		mocked.correctHealing.mockResolvedValue(fresh);
		mocked.getHealingOutputs.mockResolvedValue({
			outputs: [output('t1', { classification: 'effect' })],
			total: 2,
		});
		await model.markPaid('u1', 7);

		expect(mocked.correctHealing).toHaveBeenCalledWith({
			kind: 'paidUse',
			outputId: 'u1',
			equipmentId: 7,
		});
		expect(current()).toBe(fresh);
		// Its group emptied, so review moves to the next group with heals.
		expect(model.filter).toBe('effect');
		expect(mocked.getHealingOutputs).toHaveBeenLastCalledWith('s1', 'effect', 0, OUTPUT_PAGE);
		expect(model.outputs.map((row) => row.id)).toEqual(['t1']);
		expect(model.busy).toBeNull();
	});

	it('takes a use back and undoes corrections through the typed targets', async () => {
		const { model } = harness();
		mocked.correctHealing.mockResolvedValue(detail());
		mocked.undoHealingCorrection.mockResolvedValue(detail());
		await model.markNotPaid('a1');
		expect(mocked.correctHealing).toHaveBeenCalledWith({ kind: 'notPaidUse', activationId: 'a1' });
		await model.undo('c1');
		expect(mocked.undoHealingCorrection).toHaveBeenCalledWith('c1');
		// A closed review is not re-read.
		expect(mocked.getHealingOutputs).not.toHaveBeenCalled();
	});

	it('keeps the detail and surfaces the refusal when a correction fails', async () => {
		const initial = detail();
		const { model, current } = harness(initial);
		mocked.correctHealing.mockRejectedValue(
			new Error('Stop the session before correcting its healing'),
		);
		await model.markNotPaid('a1');
		expect(current()).toBe(initial);
		expect(model.error).toBe('Stop the session before correcting its healing');
		model.dismissError();
		expect(model.error).toBeNull();
	});

	it('refuses a second correction while one is in flight', async () => {
		const { model } = harness();
		let finish: (value: SessionDetail) => void = () => {};
		mocked.correctHealing.mockReturnValue(
			new Promise((resolve) => {
				finish = resolve;
			}),
		);
		const first = model.markNotPaid('a1');
		await model.markNotPaid('a2');
		expect(mocked.correctHealing).toHaveBeenCalledTimes(1);
		expect(model.busy).toBe('a1');
		finish(detail());
		await first;
		expect(model.busy).toBeNull();
	});

	it('pages uncosted heals and ignores a page that lands after the group changed', async () => {
		const { model } = harness(detail({ unattributedOutputs: 60 }));
		mocked.getHealingOutputs.mockResolvedValueOnce({
			outputs: Array.from({ length: OUTPUT_PAGE }, (_, i) => output(`u${i}`)),
			total: 60,
		});
		await model.toggleReview();
		expect(model.hasMore).toBe(true);
		mocked.getHealingOutputs.mockResolvedValueOnce({
			outputs: Array.from({ length: 10 }, (_, i) => output(`v${i}`)),
			total: 60,
		});
		await model.loadMore();
		expect(mocked.getHealingOutputs).toHaveBeenLastCalledWith(
			's1',
			'unattributed',
			OUTPUT_PAGE,
			OUTPUT_PAGE,
		);
		expect(model.outputs).toHaveLength(60);
		expect(model.hasMore).toBe(false);

		let late: (value: { outputs: HealingOutput[]; total: number }) => void = () => {};
		mocked.getHealingOutputs.mockReturnValueOnce(
			new Promise((resolve) => {
				late = resolve;
			}),
		);
		const stale = model.setFilter('effect');
		mocked.getHealingOutputs.mockResolvedValueOnce({
			outputs: [output('p1', { classification: 'passive' })],
			total: 1,
		});
		await model.setFilter('passive');
		late({ outputs: [output('stale')], total: 1 });
		await stale;
		expect(model.outputs.map((row) => row.id)).toEqual(['p1']);
	});

	it('loads each heal’s items once and retries after a failure', async () => {
		const { model } = harness();
		mocked.getHealingCorrectionTools.mockRejectedValueOnce(new Error('offline'));
		await model.loadTools('u1');
		expect(model.toolsFor('u1')).toEqual({ status: 'error', message: 'offline' });
		mocked.getHealingCorrectionTools.mockResolvedValueOnce([
			{ equipmentId: 1, name: 'FAP', costPerUsePed: 0.03, fits: true },
		]);
		await model.loadTools('u1');
		await model.loadTools('u1');
		expect(mocked.getHealingCorrectionTools).toHaveBeenCalledTimes(2);
		expect(model.toolsFor('u1')).toMatchObject({ status: 'ready' });
	});
});
