/**
 * Proposals as the account readings make them, for the strongbox and
 * listing tests: the recorded hunt's opening of seven, one that did not
 * settle, and a listing's outcome.
 */
import type { AccountProposal, MeasuredAmount } from '$lib/api/commands.gen';
import type { OpeningProposal } from '../openingDraft';

export function ped(value: number, precision: MeasuredAmount['precision'] = 'wholePec') {
	return { ped: value, precision };
}

function carried(name: string, units: number, value: number) {
	return { item: { name, place: { kind: 'carried' as const } }, units, amount: ped(value) };
}

/** Seven Halloween Strongboxes during the hunt: six rows of ten PED of
 * ammo, and the trousers. */
export function seven(overrides: Partial<OpeningProposal['outcome']> = {}): OpeningProposal {
	return {
		id: 7,
		readingId: 11,
		previousReadingId: 10,
		between: { from: '2026-11-14T18:02:00Z', to: '2026-11-14T18:03:00Z' },
		sessionId: 'hunt',
		possiblyRecorded: false,
		outcome: {
			kind: 'opening',
			boxes: [{ name: 'Halloween Strongbox', count: 7 }],
			keysUsed: 7,
			keys: ped(70),
			ammo: { units: 600_000, amount: ped(60, 'exact') },
			contents: [carried('Skeleton Pants Red (F)', 1, 10)],
			possibleContents: [],
			settlement: 'settled',
			doubt: null,
			paidAmmoAlone: 6,
			...overrides,
		},
	};
}

/** Six keys gone with seven boxes: the readings could not settle it. */
export function unsettled(): OpeningProposal {
	return seven({
		keysUsed: 6,
		keys: ped(60),
		ammo: null,
		contents: [],
		possibleContents: [carried('Skeleton Pants Red (F)', 1, 10), carried('Mayhem Token', 1, 0)],
		settlement: 'unsettled',
		doubt: 'countsDiffer',
		paidAmmoAlone: null,
	});
}

/** The five before the hunt: keys of 50.00 PED, ammo read as 49.95 PED in
 * five rows (four of ten PED), and a fireworks with no TT that may be its
 * own. */
export function five(): OpeningProposal {
	return {
		...seven({
			boxes: [{ name: 'Halloween Strongbox', count: 5 }],
			keysUsed: 5,
			keys: ped(50),
			ammo: { units: 499_500, amount: ped(49.95, 'exact') },
			contents: [],
			possibleContents: [carried('Halloween Fireworks', 1, 0)],
			paidAmmoAlone: 4,
		}),
		id: 5,
		readingId: 3,
		previousReadingId: 2,
		sessionId: null,
	};
}

export function sold(listingId: string, proceeds: number | null, id = 21): AccountProposal {
	return {
		id,
		readingId: 31,
		previousReadingId: 30,
		between: { from: '2026-11-14T18:02:00Z', to: '2026-11-15T09:11:00Z' },
		sessionId: null,
		possiblyRecorded: false,
		outcome: {
			kind: 'listingSold',
			listingId,
			itemName: 'Shrapnel',
			units: 100_000,
			tt: ped(10),
			proceeds: proceeds === null ? null : ped(proceeds),
		},
	};
}

export function returned(listingId: string, id = 22): AccountProposal {
	return {
		id,
		readingId: 32,
		previousReadingId: 31,
		between: { from: '2026-11-14T18:02:00Z', to: '2026-11-15T09:11:00Z' },
		sessionId: null,
		possiblyRecorded: false,
		outcome: {
			kind: 'listingReturned',
			listingId,
			itemName: 'Shrapnel',
			units: 100_000,
			tt: ped(10),
			returnedTo: { kind: 'storage', planet: 'Calypso' },
		},
	};
}
