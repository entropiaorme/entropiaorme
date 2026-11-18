// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import type { QueuedListingDraft, SaleQueueCapture } from '$lib/api/commands.gen';

vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn().mockResolvedValue(() => undefined),
}));

import CreateListingModal from './CreateListingModal.svelte';

beforeAll(() => {
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

const figure = (text: string, check = false) => ({ value: Number(text), text, check });

function draft(row: number, name: string): QueuedListingDraft {
	return {
		draftId: `4-${row}`,
		itemName: { text: name, found: 'nowhere' },
		ttValue: figure('20.80', true),
		quantity: figure('52', true),
		startingBid: figure('42'),
		buyout: { kind: 'off' },
		auctionDays: figure('7', true),
		listingFee: figure('1.54'),
	};
}

const QUEUE: SaleQueueCapture = {
	kind: 'read',
	captureId: 4,
	readAt: '2026-11-03T19:42:07',
	drafts: [draft(0, 'Output Amplifier Component'), draft(1, 'Animal Hide')],
	unread: [{ problems: [{ field: 'startingBid', why: 'notRead' }] }],
	queued: { kind: 'some', of: 4 },
};

/** A promise the test settles when it chooses. */
function pending<T>() {
	let settle!: (value: T) => void;
	const promise = new Promise<T>((resolve) => {
		settle = resolve;
	});
	return { promise, settle };
}

const NO_MATCH = { candidates: [], resolved: null };

function open(
	capture: SaleQueueCapture,
	onresolve = vi.fn().mockResolvedValue(NO_MATCH),
	onsubmit = vi.fn().mockResolvedValue(undefined),
) {
	render(CreateListingModal, {
		props: {
			open: true,
			holdings: [],
			onresolve,
			oncapture: vi.fn().mockResolvedValue(capture),
			oncollect: vi.fn().mockResolvedValue(null),
			onsubmit,
		},
	});
	return onsubmit;
}

const quantity = () => screen.getByLabelText(/Quantity/) as HTMLInputElement;
const listButton = () =>
	screen.getByRole('button', { name: 'List on auction' }) as HTMLButtonElement;

describe('CreateListingModal over a capture of the sell queue', () => {
	it('offers each queued item and records the one accepted as read and as edited', async () => {
		const onsubmit = open(QUEUE);
		await fireEvent.click(screen.getByRole('button', { name: 'Capture from game' }));

		expect(await screen.findByText('Read 2 of 4 queued items. 1 is out of view.')).not.toBeNull();
		expect(screen.getByText('Not read: the opening bid did not read clearly')).not.toBeNull();
		expect(
			screen.getAllByText('52 · opening bid 42 PED · no buyout · 7 days · fee 1.54 PED'),
		).toHaveLength(2);
		// What nothing on screen fully confirms is marked, beside the list's
		// names and the form's fields alike, and says why.
		const marks = screen.getAllByText('check');
		expect(marks.length).toBeGreaterThan(2);
		expect(marks.map((mark) => mark.getAttribute('title'))).toContain(
			'Nothing on screen fully confirms this; compare it with the sell window.',
		);

		await waitFor(() => expect(quantity().disabled).toBe(false));
		expect(quantity().value).toBe('52');
		await fireEvent.input(quantity(), { target: { value: '40' } });
		await fireEvent.click(listButton());

		await waitFor(() => expect(onsubmit).toHaveBeenCalledTimes(1));
		const input = onsubmit.mock.calls[0][0];
		expect(input.occurredAt).toBe('2026-11-03T19:42:07');
		expect(input.fields).toMatchObject({ quantity: 40, startingBid: 42, buyout: null });
		expect(input.fieldSources).toContainEqual({ field: 'quantity', source: { kind: 'typed' } });
		expect(input.fieldSources).toContainEqual({
			field: 'startingBid',
			source: { kind: 'readChecked', text: '42' },
		});
		expect(input.fieldSources).toContainEqual({
			field: 'buyout',
			source: { kind: 'readChecked', text: null },
		});
		// The draft says it is recorded, and the next queued item comes up
		// for review in a form that stays open.
		expect(await screen.findByText('Recorded')).not.toBeNull();
		await waitFor(() => expect(quantity().value).toBe('52'));
	});

	it('lists nothing while the name is still being matched to a holding, then lists the asset', async () => {
		const resolution = pending<{
			candidates: { kind: string; holdingId: string; name: string; score: number }[];
			resolved: { kind: string; holdingId: string; name: string; score: number } | null;
		}>();
		const asset = {
			kind: 'equipment',
			holdingId: 'asset-1',
			name: 'Output Amplifier Component',
			score: 100,
		};
		const onsubmit = open(QUEUE, vi.fn().mockReturnValue(resolution.promise));
		await fireEvent.click(screen.getByRole('button', { name: 'Capture from game' }));
		await screen.findByText('Read 2 of 4 queued items. 1 is out of view.');

		// The button cannot be pressed until the match answers.
		expect(listButton().disabled).toBe(true);
		await fireEvent.click(listButton());
		expect(onsubmit).not.toHaveBeenCalled();

		resolution.settle({ candidates: [asset], resolved: asset });
		await waitFor(() => expect(listButton().disabled).toBe(false));
		await fireEvent.click(listButton());
		await waitFor(() => expect(onsubmit).toHaveBeenCalledTimes(1));
		expect(onsubmit.mock.calls[0][0].holding).toMatchObject({
			kind: 'equipment',
			holdingId: 'asset-1',
		});
	});

	it('moves on to the waiting drafts after a listing entered by hand', async () => {
		const onsubmit = open(QUEUE);
		await fireEvent.click(screen.getByRole('button', { name: 'Capture from game' }));
		await waitFor(() => expect(quantity().value).toBe('52'));

		await fireEvent.click(screen.getByRole('button', { name: 'Enter by hand' }));
		await waitFor(() => expect(quantity().value).toBe(''));
		const name = document.getElementById('create-listing-item') as HTMLInputElement;
		await fireEvent.input(name, { target: { value: 'Animal Oil Residue' } });
		await fireEvent.focusOut(name);
		await waitFor(() => expect(quantity().disabled).toBe(false));
		await fireEvent.input(quantity(), { target: { value: '5' } });
		await fireEvent.input(screen.getByLabelText(/Starting bid/), { target: { value: '3' } });
		await fireEvent.click(listButton());

		await waitFor(() => expect(onsubmit).toHaveBeenCalledTimes(1));
		expect(onsubmit.mock.calls[0][0].fieldSources).toBeNull();
		// The form stays open on the first draft still waiting.
		await waitFor(() => expect(quantity().value).toBe('52'));
		expect(screen.queryByText('Recorded')).toBeNull();
	});

	it('says plainly why a capture read nothing', async () => {
		open({ kind: 'refused', captureId: 5, reason: 'sellWindowNotOpen' });
		await fireEvent.click(screen.getByRole('button', { name: 'Capture from game' }));
		expect(
			await screen.findByText('Open the Auction sell window in the game, with its title in view'),
		).not.toBeNull();
	});
});
