// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SkillScanReview } from '$lib/api';
import { reviewNotes } from '$lib/features/skillScan/skillScanView';

const api = vi.hoisted(() => ({
	acceptSkillScan: vi.fn(),
	getCharacterSkills: vi.fn(),
	getSkillScanReview: vi.fn(),
	rejectSkillScan: vi.fn(),
	showScanOverlay: vi.fn(),
	startSkillScan: vi.fn(),
}));

vi.mock('$lib/api', () => api);

import ScanReviewKindPane from './ScanReviewKindPane.svelte';

/** A review with every kind of note the player can be told. */
const everything: SkillScanReview = {
	scan: 3,
	skills: { Agility: 73.71, Alertness: 3068.94 },
	total: { check: 'mismatch', sum: 4427, total: 4500 },
	nearMisses: [{ skill: 'Alertness', readAs: 'Alertnes' }],
	guesses: [{ readAs: 'Alm', probably: 'Aim', pointsText: '1,268.53' }],
	unknown: [{ name: 'Food Technology', pointsText: '5.25' }],
	unplaced: [{ name: 'Arn', pointsText: '9.00' }],
	repeated: ['Analysis'],
};

beforeEach(() => {
	for (const fn of Object.values(api)) fn.mockReset();
	api.getCharacterSkills.mockResolvedValue([]);
});

describe('the scan review pane', () => {
	it('shows every note the review has, each with the skills it concerns', async () => {
		api.getSkillScanReview.mockResolvedValue(everything);
		render(ScanReviewKindPane, { onComplete: () => {} });
		const notes = reviewNotes(everything);
		expect(notes).toHaveLength(6);
		for (const note of notes) {
			expect(await screen.findByText(note.text)).toBeTruthy();
			if (note.items.length > 0) {
				expect(screen.getByText(note.items.join(', '))).toBeTruthy();
			}
		}
	});

	it('leads with scanning again when the figures do not add up, saving only on request', async () => {
		api.getSkillScanReview.mockResolvedValue(everything);
		render(ScanReviewKindPane, { onComplete: () => {} });
		expect(await screen.findByRole('button', { name: 'Scan again' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Save these figures anyway' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Accept' })).toBeNull();
	});

	it('offers Accept when the figures add up, and locks every action while saving', async () => {
		api.getSkillScanReview.mockResolvedValue({
			...everything,
			total: { check: 'matches' },
		});
		render(ScanReviewKindPane, { onComplete: () => {}, saving: true });
		const accept = await screen.findByRole('button', { name: 'Accept' });
		expect((accept as HTMLButtonElement).disabled).toBe(true);
		expect((screen.getByRole('button', { name: 'Reject' }) as HTMLButtonElement).disabled).toBe(
			true,
		);
		expect(screen.queryByRole('button', { name: 'Scan again' })).toBeNull();
	});
});
