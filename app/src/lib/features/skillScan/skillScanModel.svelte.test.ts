import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillScanReply, SkillScanStatus } from '$lib/api';
import { createSkillScanModel } from './skillScanModel.svelte';

const startSkillScan = vi.fn();
const cancelSkillScan = vi.fn();

vi.mock('$lib/api', () => ({
	startSkillScan: (...args: unknown[]) => startSkillScan(...args),
	cancelSkillScan: (...args: unknown[]) => cancelSkillScan(...args),
}));

const idle: SkillScanStatus = { phase: 'idle', available: true, stopped: null };
const collecting: SkillScanStatus = {
	phase: 'collecting',
	pagesCollected: 2,
	pageCount: 13,
	missingPages: [1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
	hint: 'notSkillsTab',
};

/** A model over a status the test sets, whose re-read is counted. */
function rig(initial: SkillScanStatus | null) {
	let status: SkillScanStatus | null = initial;
	const refresh = vi.fn(async () => {});
	const model = createSkillScanModel({ status: () => status, refresh });
	return {
		model,
		refresh,
		set: (next: SkillScanStatus | null) => {
			status = next;
		},
	};
}

beforeEach(() => {
	startSkillScan.mockReset();
	cancelSkillScan.mockReset();
});

describe('skill scan model', () => {
	it('renders the status it is given, and follows it', () => {
		const { model, set } = rig(idle);
		expect(model.view).toEqual({ kind: 'idle', canStart: true, note: null });
		set(collecting);
		expect(model.view).toMatchObject({
			kind: 'collecting',
			progress: '2 of 13 pages',
			hint: 'Switch the Skills window to its SKILLS tab.',
		});
		// A hint never outlives the status that carried it.
		set({ ...collecting, hint: null });
		expect(model.view).toMatchObject({ hint: null });
		set({ phase: 'awaitingReview' });
		expect(model.view).toEqual({ kind: 'review' });
	});

	it('starts the scan, then re-reads the status rather than folding the answer in', async () => {
		startSkillScan.mockResolvedValue({ status: collecting, refused: null });
		const { model, refresh } = rig(idle);
		await model.start();
		expect(startSkillScan).toHaveBeenCalledTimes(1);
		expect(refresh).toHaveBeenCalledTimes(1);
		// The view moves only with the status, never with the answer.
		expect(model.view).toMatchObject({ kind: 'idle' });
		expect(model.busy).toBe(false);
		expect(model.error).toBeNull();
	});

	it('cancels the scan and re-reads', async () => {
		cancelSkillScan.mockResolvedValue({ status: idle, refused: null });
		const { model, refresh } = rig(collecting);
		await model.cancel();
		expect(cancelSkillScan).toHaveBeenCalledTimes(1);
		expect(refresh).toHaveBeenCalledTimes(1);
	});

	it('says when an action fails, still re-reads, and clears the failure on the next', async () => {
		startSkillScan.mockRejectedValueOnce(new Error('backend away'));
		const { model, refresh } = rig(idle);
		await model.start();
		expect(model.error).toBe('backend away');
		expect(model.busy).toBe(false);
		expect(refresh).toHaveBeenCalledTimes(1);
		startSkillScan.mockResolvedValueOnce({ status: collecting, refused: null });
		await model.start();
		expect(model.error).toBeNull();
	});

	it('says why a start was refused', async () => {
		startSkillScan.mockResolvedValue({ status: idle, refused: 'surfaceClosed' });
		const { model, refresh } = rig(idle);
		await model.start();
		expect(model.error).toBe('Open the scan window to start a scan.');
		expect(refresh).toHaveBeenCalledTimes(1);
	});

	it('shows a review being saved as such', () => {
		const { model } = rig({ phase: 'saving' });
		expect(model.view).toEqual({ kind: 'saving' });
	});

	it('is busy while an action is in flight', async () => {
		let resolve!: (value: SkillScanReply) => void;
		startSkillScan.mockReturnValue(
			new Promise<SkillScanReply>((r) => {
				resolve = r;
			}),
		);
		const { model } = rig(idle);
		const started = model.start();
		expect(model.busy).toBe(true);
		resolve({ status: collecting, refused: null });
		await started;
		expect(model.busy).toBe(false);
	});
});
