import { describe, expect, it } from 'vitest';
import type { StockConversionFromAccount } from '$lib/api/commands.gen';
import { listedListing, listedStep } from './__fixtures__/activity';
import {
	accountRecordSentence,
	fromAccountSentence,
	fromAccountWindow,
	isRecentFromAccount,
	recentListingFromAccount,
} from './fromAccount';

const now = new Date('2026-03-10T12:00:00Z');

describe('when a record from the account happened', () => {
	it('says the day once when both readings fall on it', () => {
		expect(
			fromAccountWindow({ from: '2026-03-04T14:02:00Z', to: '2026-03-04T14:03:00Z' }, now),
		).toBe('between Mar 4, 2:02 PM and 2:03 PM');
	});

	it('says both days when the step crosses midnight', () => {
		expect(
			fromAccountWindow({ from: '2026-03-04T23:58:00Z', to: '2026-03-05T00:01:00Z' }, now),
		).toBe('between Mar 4, 11:58 PM and Mar 5, 12:01 AM');
	});

	it('calls today today', () => {
		expect(
			fromAccountSentence({ from: '2026-03-10T09:02:00Z', to: '2026-03-10T09:03:00Z' }, now),
		).toBe('Recorded from your account readings: it happened between today, 9:02 AM and 9:03 AM.');
	});
});

describe('the reminder beside a manual conversion', () => {
	const conversion: StockConversionFromAccount = {
		id: 'conversion-2',
		quantity: 12_000,
		ttValue: 1.2,
		recordedAt: '2026-03-10T11:41:00Z',
	};

	it('says how much was recorded, its TT, and when', () => {
		expect(accountRecordSentence('conversion', 'Shrapnel', conversion)).toBe(
			'Your account recorded a conversion of 12000 Shrapnel (1.20 PED TT) at 11:41 AM; record this only if it is another one.',
		);
	});

	it('leaves out a TT of nothing', () => {
		const tokens = { ...conversion, quantity: 3, ttValue: 0 };
		expect(accountRecordSentence('conversion', 'Mayhem Token', tokens)).toBe(
			'Your account recorded a conversion of 3 Mayhem Token at 11:41 AM; record this only if it is another one.',
		);
	});

	it('names a Trade Terminal sale as one', () => {
		expect(accountRecordSentence('sale', 'Shrapnel', conversion)).toBe(
			'Your account recorded a Trade Terminal sale of 12000 Shrapnel (1.20 PED TT) at 11:41 AM; record this only if it is another one.',
		);
	});

	it('is recent for the hour after its step ended, and not after', () => {
		const at = '2026-03-10T11:00:00Z';
		expect(isRecentFromAccount(at, new Date('2026-03-10T11:59:59Z'))).toBe(true);
		expect(isRecentFromAccount(at, new Date('2026-03-10T12:00:00Z'))).toBe(true);
		expect(isRecentFromAccount(at, new Date('2026-03-10T12:00:01Z'))).toBe(false);
		expect(isRecentFromAccount('not a time', new Date('2026-03-10T11:00:00Z'))).toBe(false);
	});
});

describe('the reminder beside a manual listing', () => {
	// The listing's step ended at 10:05 on the 15th.
	const at = new Date('2026-11-15T10:30:00Z');

	it('is the latest listing of the item the readings created within the hour', () => {
		const older = listedListing({
			id: 'older',
			listedFromAccount: listedStep({ to: '2026-11-15T10:00:00Z' }),
		});
		const latest = listedListing({ quantity: 4, ttValue: 2 });
		expect(recentListingFromAccount([older, latest], ' animal hide ', at)).toEqual({
			id: 'listing-made',
			quantity: 4,
			ttValue: 2,
			recordedAt: '2026-11-15T10:05:00Z',
		});
		expect(recentListingFromAccount([latest, older], 'Animal Hide', at)?.id).toBe('listing-made');
	});

	it('is nothing for another item, one the player listed, or once the hour has passed', () => {
		const made = listedListing();
		expect(recentListingFromAccount([made], 'Iron Stone', at)).toBeNull();
		expect(
			recentListingFromAccount([listedListing({ listedFromAccount: null })], 'Animal Hide', at),
		).toBeNull();
		expect(
			recentListingFromAccount([made], 'Animal Hide', new Date('2026-11-15T11:05:01Z')),
		).toBeNull();
	});

	it('names an auction listing as one', () => {
		const record = { id: 'l', quantity: 10, ttValue: 5, recordedAt: '2026-03-10T11:41:00Z' };
		expect(accountRecordSentence('listing', 'Animal Hide', record)).toMatch(
			/^Your account recorded an auction listing of 10 Animal Hide \(5\.00 PED TT\) at .+; record this only if it is another one\.$/,
		);
	});
});
