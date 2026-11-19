import { describe, expect, it, vi } from 'vitest';

vi.mock('$lib/api/account', () => ({ ACCOUNT_TOPIC: 'account:updated' }));

import { returned, seven, sold } from '$lib/features/strongbox/__fixtures__/proposals';
import {
	listedFromAccountSentence,
	proposalSentence,
	proposalsByListing,
} from './listingProposals.svelte';

describe('what the account shows of listings', () => {
	it('keeps the latest proposal for each listing and nothing that is not a listing', () => {
		const found = proposalsByListing([
			sold('a', 3, 1),
			returned('a', 2),
			sold('b', null, 3),
			seven(),
		]);
		expect([...found.keys()]).toEqual(['a', 'b']);
		expect(found.get('a')?.id).toBe(1);
	});

	it('says what happened in plain words with the window it happened in', () => {
		const now = new Date('2026-11-20T12:00:00Z');
		const priced = sold('a', 12.5);
		if (priced.outcome.kind !== 'listingSold') throw new Error('a sale');
		const sentence = proposalSentence({ ...priced, outcome: priced.outcome }, now);
		expect(sentence).toMatch(
			/^Your account shows this left the auction between .+ and .+\. Your PED Card rose by 12\.50 PED between those two readings: that is the figure filled in, so correct it if the auction took a fee first\.$/,
		);
		const back = returned('a');
		if (back.outcome.kind !== 'listingReturned') throw new Error('a return');
		expect(proposalSentence({ ...back, outcome: back.outcome }, now)).toMatch(
			/^Your account shows this came back from the auction between .+ and .+\.$/,
		);
	});
});

describe('a listing the account created', () => {
	it('says when it was seen made, and its starting bid as the range the fee reads back as', () => {
		const now = new Date('2026-11-20T12:00:00Z');
		const seen = {
			from: '2026-11-20T10:00:00Z',
			to: '2026-11-20T10:01:00Z',
			inferredBid: { low: 5.21, high: 5.41 },
		};
		const sentence = listedFromAccountSentence(seen, now);
		expect(sentence).toMatch(/^Recorded from your account: listed between .+ and .+\./);
		expect(sentence).toContain('Starting bid inferred from the fee: between 5.21 and 5.41 PED.');
		expect(listedFromAccountSentence({ ...seen, inferredBid: null }, now)).not.toContain(
			'Starting bid',
		);
	});
});
