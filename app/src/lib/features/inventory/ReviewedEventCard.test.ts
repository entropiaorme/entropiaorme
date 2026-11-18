// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { entry, opening } from './__fixtures__/activity';
import { type ReviewedEvent, splitHistory } from './accountActivity';
import ReviewedEventCard from './ReviewedEventCard.svelte';

function show(event: ReviewedEvent, onundo = vi.fn().mockResolvedValue(undefined)) {
	render(ReviewedEventCard, { props: { event, onundo } });
	return onundo;
}

const check = () =>
	splitHistory(
		[
			entry({ id: 'a', reconciliationId: 'r1', kind: 'removal', itemName: 'Lysterium Ingot' }),
			entry({ id: 'b', reconciliationId: 'r1', kind: 'removal', itemName: 'Animal Oil Residue' }),
			entry({ id: 'c', reconciliationId: 'r1', kind: 'unknownOrigin', itemName: 'Iron Ingot' }),
		],
		[],
		[],
	).reviewed[0];

describe('a stock check, as one event', () => {
	it('groups what it recorded by what each record did', () => {
		show(check());
		expect(screen.getByRole('heading', { name: 'Stock check' })).toBeTruthy();
		expect(screen.getByText('Removed')).toBeTruthy();
		expect(screen.getByText('Held, of unknown origin')).toBeTruthy();
		expect(screen.getByTitle('Iron Ingot')).toBeTruthy();
	});

	it('is taken back whole, after saying what that does', async () => {
		const event = check();
		const onundo = show(event);
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		expect(screen.getByText(/the differences it settled are open again/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Undo the check' }));
		expect(onundo).toHaveBeenCalledWith(event);
	});
});

describe('a strongbox opening, as one event', () => {
	it('shows the boxes and keys in, and the ammo, kept items and Nanocubes out', () => {
		show(splitHistory([], [opening()], []).reviewed[0]);
		expect(screen.getByRole('heading', { name: 'Strongbox opening' })).toBeTruthy();
		expect(screen.getByTitle('Halloween Strongbox').textContent).toBe('3 Halloween Strongbox');
		expect(screen.getByTitle('Keys')).toBeTruthy();
		expect(screen.getByTitle('Ammo')).toBeTruthy();
		expect(screen.getByTitle('Pumpkin Hat (C)')).toBeTruthy();
		expect(screen.getByTitle('Nanocube').textContent).toBe('40 Nanocube');
		expect(screen.getByText(/keys 0\.60 PED above TT/)).toBeTruthy();
	});

	it('says an opening already taken back is undone, and offers nothing', () => {
		show(splitHistory([], [opening({ undone: true })], []).reviewed[0]);
		expect(screen.getByText('Undone')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
	});
});
