// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ActivityHistoryEntry } from '$lib/api/commands.gen';
import ActivityHistory from './ActivityHistory.svelte';

/** A Shrapnel conversion the player entered by hand. */
const manual: ActivityHistoryEntry = {
	id: 'conversion-1',
	subjectKind: 'loot',
	channel: 'conversion',
	kind: 'conversion',
	status: 'converted',
	itemName: 'Shrapnel',
	targetItem: 'Universal Ammo',
	occurredAt: '2026-03-04',
	quantity: 12_000,
	ttValue: 1.2,
	netMarkup: null,
	activityNetMarkup: null,
	canRevertSale: false,
	canDelete: true,
	undoBlockedReason: null,
	undone: false,
	conversionLines: [],
	pedAmount: null,
	itemised: false,
	derivedFrom: [],
	recordedFromAccount: null,
	reconciliationId: null,
};

/** The same conversion, recorded on its own from the account readings. */
const fromAccount: ActivityHistoryEntry = {
	...manual,
	id: 'conversion-2',
	recordedFromAccount: {
		readingId: 7,
		from: '2026-03-04T14:02:00Z',
		to: '2026-03-04T14:03:00Z',
		recordedAt: '2026-03-04T14:03:05Z',
		cardChange: null,
	},
};

function show(entries: ActivityHistoryEntry[], onundo = vi.fn().mockResolvedValue(undefined)) {
	render(ActivityHistory, { props: { entries, loading: false, onundo, embedded: true } });
	return onundo;
}

describe('a history entry recorded from the account', () => {
	it('says so quietly, with when it happened on focus', async () => {
		show([fromAccount]);
		expect(screen.getByText('from your account')).toBeTruthy();
		const tip = screen.getByRole('tooltip');
		expect(tip.textContent).toContain(
			'Recorded from your account readings: it happened between Mar 4, 2:02 PM and 2:03 PM.',
		);
		expect(tip.classList.contains('invisible')).toBe(true);
		await fireEvent.focus(screen.getByRole('button', { name: 'Recorded from your account' }));
		expect(tip.classList.contains('invisible')).toBe(false);
	});

	it('is not said of an entry the player made', () => {
		show([manual]);
		expect(screen.queryByText('from your account')).toBeNull();
		expect(screen.queryByRole('tooltip')).toBeNull();
	});

	it('is undone as any other, saying the readings will not record it again', async () => {
		const onundo = show([fromAccount]);
		expect(screen.queryByText('Your account readings will not record this again.')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		expect(screen.getByText('Your account readings will not record this again.')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm undo' }));
		await waitFor(() => expect(onundo).toHaveBeenCalledWith(fromAccount, false));
	});

	it('keeps the copy off the undo of an entry the player made', async () => {
		const onundo = show([manual]);
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		expect(screen.queryByText('Your account readings will not record this again.')).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm undo' }));
		await waitFor(() => expect(onundo).toHaveBeenCalledWith(manual, false));
	});
});

describe('a history entry a stock check recorded', () => {
	// As the backend lists it: held by the check, so not undone on its own.
	const held = {
		reconciliationId: 'check-1',
		canDelete: false,
		undoBlockedReason:
			'this was recorded by a stock reconciliation: undo the reconciliation as a whole',
	};
	const removal: ActivityHistoryEntry = {
		...manual,
		...held,
		id: 'removal-1',
		channel: 'removal',
		kind: 'removal',
		status: 'removed',
		itemName: 'Lysterium Ingot',
		targetItem: null,
	};
	const conversion: ActivityHistoryEntry = { ...manual, ...held };

	function showCheck(entries: ActivityHistoryEntry[]) {
		const onundo = vi.fn().mockResolvedValue(undefined);
		const onundocheck = vi.fn().mockResolvedValue(undefined);
		render(ActivityHistory, {
			props: { entries, loading: false, onundo, onundocheck, embedded: true },
		});
		return { onundo, onundocheck };
	}

	it('is undone only with the whole check, naming what is listed of it', async () => {
		const { onundo, onundocheck } = showCheck([removal, conversion]);

		const undos = screen.getAllByRole('button', { name: 'Undo the stock check' });
		expect(undos).toHaveLength(2);
		expect(screen.queryByText('Held')).toBeNull();
		await fireEvent.click(undos[0]);
		expect(
			screen.getByText(
				'Part of a stock check: undo the check as a whole. It takes back Lysterium Ingot (removed), Shrapnel (converted) and anything else the check recorded.',
			),
		).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Confirm undo' })).toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Undo the check' }));
		await waitFor(() => expect(onundocheck).toHaveBeenCalledWith('check-1'));
		expect(onundo).not.toHaveBeenCalled();
	});

	it('is held, with no Undo of its own, where nothing can undo the check', () => {
		show([conversion]);
		expect(screen.getByText('Held')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Undo the stock check' })).toBeNull();
	});

	it('offers nothing once undone', () => {
		showCheck([{ ...removal, undone: true, canDelete: true, undoBlockedReason: null }]);
		expect(screen.queryByRole('button', { name: 'Undo the stock check' })).toBeNull();
		expect(screen.getByText('Undone')).toBeTruthy();
	});
});

describe('stock held as of unknown origin', () => {
	const arrival: ActivityHistoryEntry = {
		...manual,
		id: 'unknown-7',
		channel: 'unknownOrigin',
		kind: 'unknownOrigin',
		status: 'held',
		itemName: 'Iron Stone',
		targetItem: null,
		quantity: 12,
		ttValue: 1.2,
	};

	it('says plainly what was added, and can be undone', async () => {
		const onundo = show([arrival]);
		expect(screen.getByText('Added')).toBeTruthy();
		expect(screen.getByText('12 held as stock of unknown origin, 1.20 PED TT')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Undo' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm undo' }));
		await waitFor(() => expect(onundo).toHaveBeenCalledWith(arrival, false));
	});
});
