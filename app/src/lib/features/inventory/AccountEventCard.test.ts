// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import {
	listedListing,
	listedRow,
	listing,
	listingRow,
	recycling,
	sale,
	shrapnel,
	step,
} from './__fixtures__/activity';
import AccountEventCard from './AccountEventCard.svelte';
import { type AccountEvent, splitHistory } from './accountActivity';

function eventOf(...rows: Parameters<typeof splitHistory>[0]): AccountEvent {
	return splitHistory(rows, [], [listing()]).account[0];
}

function show(event: AccountEvent, isNew = false, onundo = vi.fn().mockResolvedValue(undefined)) {
	render(AccountEventCard, { props: { event, isNew, onundo } });
	return onundo;
}

describe('an event the account recorded on its own', () => {
	it('lays a recycling out as what went in and the Nanocubes out, balanced', () => {
		show(eventOf(recycling()));
		expect(screen.getByRole('heading', { name: 'Recycled' })).toBeTruthy();
		expect(screen.getByText('In')).toBeTruthy();
		expect(screen.getByTitle('Lysterium Ingot').textContent).toBe('100 Lysterium Ingot');
		expect(screen.getByTitle('Nanocube').textContent).toBe('150 Nanocube');
		expect(screen.getByTestId('event-balance').textContent).toContain(
			'Balanced: the Nanocubes are worth what went in.',
		);
	});

	it("shows Shrapnel to ammo with the Trade Terminal's 1%", () => {
		show(eventOf(shrapnel()));
		expect(screen.getByRole('heading', { name: 'Shrapnel to ammo' })).toBeTruthy();
		expect(screen.getByTestId('event-balance').textContent).toMatch(
			/\+0\.10 PED, the Trade Terminal's 1% on 100000 Shrapnel\./,
		);
	});

	it('marks a conversion the readings inferred, and says from what', () => {
		show(eventOf(shrapnel()));
		expect(screen.queryByTestId('event-inferred')).toBeNull();
		const inferred = (basis: 'presumed' | 'session') =>
			eventOf(shrapnel({ recordedFromAccount: step(41, '2026-11-15T10:01:00Z', { basis }) }));
		show(inferred('session'));
		expect(screen.getByTestId('event-inferred').textContent).toBe('Inferred');
		expect(screen.getAllByTestId('event-balance').at(-1)?.textContent).toContain(
			'Inferred from the session',
		);
	});

	it("gathers a step's sales beside the Card's rise", () => {
		const rose = { recordedFromAccount: step(43, '2026-11-15T10:03:00Z', { cardChange: 0.9 }) };
		show(eventOf(sale('s1', 'Animal Hide', 0.5, 43, rose), sale('s2', 'Wool', 0.3, 43, rose)));
		expect(screen.getByRole('heading', { name: 'Sold to the Trade Terminal' })).toBeTruthy();
		expect(screen.getByTitle('Animal Hide')).toBeTruthy();
		expect(screen.getByTitle('Wool')).toBeTruthy();
		expect(screen.getByText('Rose by')).toBeTruthy();
		const balance = screen.getByTestId('event-balance').textContent ?? '';
		expect(balance).toContain('At TT: 0.80 PED');
		expect(balance).toContain('the Card rose by 0.90 PED');
	});

	it('shows a sold listing at its price and markup after fees', () => {
		show(eventOf(listingRow()));
		expect(screen.getByRole('heading', { name: 'Listing sold' })).toBeTruthy();
		expect(screen.getByText('Sold for')).toBeTruthy();
		const balance = screen.getByTestId('event-balance').textContent ?? '';
		expect(balance).toContain('Markup +2.25 PED after fees');
		expect(balance).toContain('a sale fee of 0.25 PED');
	});

	it('shows a listing that came back with the item back and the fee lost', () => {
		show(eventOf(listingRow({ status: 'returned', netMarkup: null })));
		expect(screen.getByRole('heading', { name: 'Listing came back' })).toBeTruthy();
		expect(screen.getByText('Back in your storage')).toBeTruthy();
		expect(screen.getByTestId('event-balance').textContent).toContain(
			'Listing fee lost: 0.50 PED.',
		);
	});

	it('says when the readings saw it', () => {
		show(eventOf(shrapnel()));
		expect(screen.getByText(/^Seen between /)).toBeTruthy();
	});
});

describe('the mark on a new event', () => {
	it('is shown while the event is new, and absent otherwise', () => {
		show(eventOf(shrapnel()), true);
		expect(screen.getByTestId('event-new').textContent).toBe('New');
	});

	it('is absent from an event already seen', () => {
		show(eventOf(shrapnel()));
		expect(screen.queryByTestId('event-new')).toBeNull();
	});
});

describe('taking an event back', () => {
	it('confirms in place, saying what it does, then takes it back', async () => {
		const event = eventOf(recycling());
		const onundo = show(event);
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		expect(screen.getByText(/The items come back and the Nanocubes go\./)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Undo recycling' }));
		expect(onundo).toHaveBeenCalledWith(event);
		await waitFor(() =>
			expect(screen.queryByRole('button', { name: 'Undo recycling' })).toBeNull(),
		);
	});

	it('keeps the confirmation open when the undo fails', async () => {
		show(eventOf(recycling()), false, vi.fn().mockRejectedValue(new Error('refused')));
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Undo recycling' }));
		await waitFor(() =>
			expect(screen.getByRole('button', { name: 'Undo recycling' })).toBeTruthy(),
		);
	});

	it('says why it is held when something since depends on it', () => {
		show(eventOf(shrapnel({ canDelete: false, undoBlockedReason: 'The ammo was spent.' })));
		expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
		expect(screen.getByText('Held')).toBeTruthy();
		expect(screen.getByText('The ammo was spent.')).toBeTruthy();
	});

	it('says an event already taken back is undone', () => {
		show(eventOf(shrapnel({ undone: true })));
		expect(screen.getByText('Undone')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
	});
});

describe('a listing the readings created', () => {
	it('shows the item, the fee, and the starting bid as an inferred range', () => {
		show(splitHistory([listedRow()], [], [listedListing()]).account[0]);
		expect(screen.getByRole('heading', { name: 'Listed on the auction' })).toBeTruthy();
		expect(screen.getByTestId('event-inferred').textContent).toBe('Inferred');
		expect(screen.getByTitle('Animal Hide').textContent).toBe('10 Animal Hide');
		expect(screen.getByTitle('Listing fee')).toBeTruthy();
		expect(document.body.textContent).toContain('0.51');
		expect(screen.getByTestId('event-balance').textContent).toMatch(
			/Starting bid inferred from the fee: between 5\.21 and 5\.41 PED\. Buyout and duration not known\./,
		);
	});

	it('shows no undo once the listing has an outcome', () => {
		const sold = listedRow({
			status: 'sold',
			recordedFromAccount: step(46, '2026-11-15T12:00:00Z'),
		});
		const [listed] = splitHistory([sold], [], [listedListing({ status: 'sold' })]).account.filter(
			(event) => event.kind === 'listed',
		);
		show(listed);
		expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
		expect(screen.queryByText('Held')).toBeNull();
	});

	it('is undone as a listing', async () => {
		const onundo = show(splitHistory([listedRow()], [], [listedListing()]).account[0]);
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Undo listing' }));
		await waitFor(() => expect(onundo).toHaveBeenCalledTimes(1));
	});
});
