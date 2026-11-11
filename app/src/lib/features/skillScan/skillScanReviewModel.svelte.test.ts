import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillScanReview } from '$lib/api';
import { createSkillScanReviewModel, reviewRows } from './skillScanReviewModel.svelte';

const api = vi.hoisted(() => ({
	acceptSkillScan: vi.fn(),
	getCharacterSkills: vi.fn(),
	getSkillScanReview: vi.fn(),
	rejectSkillScan: vi.fn(),
	showScanOverlay: vi.fn(),
	startSkillScan: vi.fn(),
}));

vi.mock('$lib/api', () => api);

function review(overrides: Partial<SkillScanReview> = {}): SkillScanReview {
	return {
		scan: 7,
		skills: { Agility: 73.71, Aim: 1268.53 },
		total: { check: 'matches' },
		nearMisses: [],
		guesses: [],
		unknown: [],
		unplaced: [],
		repeated: [],
		...overrides,
	};
}

const held = [
	{ name: 'Aim', level: 1260, anchorLevel: 1250, isAttribute: false },
	{ name: 'Agility', level: 73.7, anchorLevel: 73, isAttribute: true },
];

async function loaded(pending: SkillScanReview | null = review()) {
	api.getSkillScanReview.mockResolvedValue(pending);
	api.getCharacterSkills.mockResolvedValue(held);
	const onComplete = vi.fn();
	const model = createSkillScanReviewModel(onComplete);
	await model.load();
	return { model, onComplete };
}

beforeEach(() => {
	for (const fn of Object.values(api)) fn.mockReset();
});

describe('reviewRows', () => {
	it('puts the largest correction first', () => {
		const rows = reviewRows(review(), held as never);
		expect(rows.map((row) => row.name)).toEqual(['Aim', 'Agility']);
		expect(rows[0]).toMatchObject({ anchor: 1250, believed: 1260, scanned: 1268.53 });
	});
});

describe('skill scan review model', () => {
	it('loads the review with its rows and notes', async () => {
		const { model } = await loaded(
			review({ unknown: [{ name: 'Food Technology', pointsText: '1' }] }),
		);
		expect(model.review?.scan).toBe(7);
		expect(model.rows).toHaveLength(2);
		expect(model.notes).toHaveLength(1);
		expect(model.needsConfirmation).toBe(false);
		expect(model.error).toBeNull();
	});

	it('accepts by naming the review it shows', async () => {
		api.acceptSkillScan.mockResolvedValue({ ok: true, skills_persisted: 2 });
		const { model, onComplete } = await loaded();
		await model.accept();
		expect(api.acceptSkillScan).toHaveBeenCalledWith(7);
		expect(onComplete).toHaveBeenCalledTimes(1);
	});

	it('offers scanning again first when the figures do not add up, and saves only when told', async () => {
		const { model, onComplete } = await loaded(
			review({ total: { check: 'mismatch', sum: 4427, total: 4500 } }),
		);
		expect(model.needsConfirmation).toBe(true);
		api.acceptSkillScan.mockResolvedValue({ ok: true, skills_persisted: 2 });
		await model.saveAnyway();
		expect(api.acceptSkillScan).toHaveBeenCalledWith(7, true);
		expect(onComplete).toHaveBeenCalledTimes(1);
	});

	it('treats a total that could not be checked like one that does not add up', async () => {
		const { model } = await loaded(review({ total: { check: 'unchecked' } }));
		expect(model.needsConfirmation).toBe(true);
	});

	it('scans again by opening the scan window, rejecting this review, then starting', async () => {
		const calls: string[] = [];
		api.showScanOverlay.mockImplementation(async () => calls.push('show'));
		api.rejectSkillScan.mockImplementation(async (scan: number) => {
			calls.push(`reject ${scan}`);
			return { ok: true };
		});
		api.startSkillScan.mockImplementation(async () => {
			calls.push('start');
			return { status: { phase: 'collecting' }, refused: null };
		});
		const { model, onComplete } = await loaded(review({ total: { check: 'unchecked' } }));
		await model.scanAgain();
		expect(calls).toEqual(['show', 'reject 7', 'start']);
		expect(onComplete).toHaveBeenCalledTimes(1);
	});

	it('says why a request was refused and reloads an out-of-date review', async () => {
		api.acceptSkillScan.mockResolvedValue({
			refused: 'staleReview',
			error: 'The review shown is no longer the one held',
		});
		const { model, onComplete } = await loaded();
		api.getSkillScanReview.mockResolvedValue(review({ scan: 8 }));
		await model.accept();
		expect(onComplete).not.toHaveBeenCalled();
		expect(model.review?.scan).toBe(8);
		expect(model.error).toMatch(/out of date/);
	});

	it('shows the save failure the backend reports, keeping the review', async () => {
		api.acceptSkillScan.mockResolvedValue({
			refused: 'saveFailed',
			error: 'Persist failed: disk full',
		});
		const { model, onComplete } = await loaded();
		await model.accept();
		expect(model.error).toBe('Persist failed: disk full');
		expect(model.review?.scan).toBe(7);
		expect(onComplete).not.toHaveBeenCalled();
	});

	it('sends one request at a time', async () => {
		let resolve!: (value: unknown) => void;
		api.acceptSkillScan.mockReturnValue(new Promise((r) => (resolve = r)));
		const { model } = await loaded();
		const first = model.accept();
		expect(model.busy).toBe(true);
		await model.reject();
		expect(api.rejectSkillScan).not.toHaveBeenCalled();
		resolve({ ok: true, skills_persisted: 2 });
		await first;
		expect(model.busy).toBe(false);
	});
});
