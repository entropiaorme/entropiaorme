import { describe, expect, it } from 'vitest';
import { seven, sold } from './__fixtures__/proposals';
import { detectedWhat, detectedWhen, runProposals } from './detectedOpenings';

describe('openings read from the account', () => {
	it('takes the openings a run saw, between its first and last readings, oldest first', () => {
		const before = { ...seven(), id: 1, previousReadingId: 4, readingId: 5 };
		const first = { ...seven(), id: 2, previousReadingId: 5, readingId: 6 };
		const second = { ...seven(), id: 3, previousReadingId: 6, readingId: 7 };
		const after = { ...seven(), id: 4, previousReadingId: 7, readingId: 8 };
		expect(
			runProposals([after, second, sold('l', null), first, before], 5, 7).map((p) => p.id),
		).toEqual([2, 3]);
	});

	it('says what it was and when the readings saw it', () => {
		const proposal = seven();
		expect(detectedWhat(proposal)).toBe('7 Halloween Strongbox');
		const sameDay = new Date(proposal.between.to);
		expect(detectedWhen(proposal, sameDay)).toMatch(/^today, .+, during a tracked session$/);
		const later = new Date(Date.parse(proposal.between.to) + 3 * 86_400_000);
		expect(detectedWhen({ ...proposal, sessionId: null }, later)).not.toMatch(/today|session/);
	});
});
