// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getTrackingSnapshot: vi.fn(),
	getLootInventory: vi.fn(),
	getEquipmentInventory: vi.fn(),
	getStrongboxOpenings: vi.fn(),
	readStrongboxExport: vi.fn(),
	recordStrongboxOpening: vi.fn(),
	undoStrongboxOpening: vi.fn(),
	getMarketItemMarkups: vi.fn(),
}));
const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	getProposals: vi.fn(),
	claimProposals: vi.fn(),
	releaseProposals: vi.fn(),
	linkProposals: vi.fn(),
	dismissProposal: vi.fn(),
	beginAccountRun: vi.fn(),
	finishAccountRun: vi.fn(),
	cancelAccountRun: vi.fn(),
	readRunWhole: vi.fn(),
}));
vi.mock('$lib/api', () => ({ getTrackingSnapshot: api.getTrackingSnapshot }));
vi.mock('$lib/api/account', () => account);
vi.mock('$lib/api/inventory', () => api);
vi.mock('$lib/api/market', () => ({ getMarketItemMarkups: api.getMarketItemMarkups }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));

import { five, seven, unsettled } from './__fixtures__/proposals';
import { createOpeningModel } from './openingModel.svelte';
import StrongboxOpening from './StrongboxOpening.svelte';

const EXPORT = [
	'Name\tQuantity\tValue(PED)',
	'Red Heart Fireworks\t1\t2.0000',
	'Skeleton Shirt White (F)\t1\t10.0000',
	'Skeleton Shirt White (M)\t1\t10.0000',
	'Halloween Strongbox\t-8\t0.0000',
	'Strongbox Key\t-8\t-80.0000',
	'Universal Ammo\t580000\t58.0000',
].join('\n');

const read = {
	boxesOpened: 8,
	keys: { count: 8, ttValue: 80 },
	ammoPed: 58,
	contents: [
		{ line: 2, itemName: 'Red Heart Fireworks', quantity: 1, valuePed: 2 },
		{ line: 3, itemName: 'Skeleton Shirt White (F)', quantity: 1, valuePed: 10 },
		{ line: 4, itemName: 'Skeleton Shirt White (M)', quantity: 1, valuePed: 10 },
	],
	unexpected: [],
	skipped: [],
	balanceGap: 0,
};

function marketItem(itemName: string, markupPct: number) {
	return {
		itemName,
		markupPct,
		marketObservedAt: 1,
		manual: null,
		preferred: 'market',
		nanocubeMarked: false,
		unitPricePed: null,
		horizon: 'month',
		salesPed: 50_000,
		recommendedPacketTt: null,
		readings: [
			{ horizon: 'week', markupPct, salesPed: 20_000 },
			{ horizon: 'month', markupPct, salesPed: 50_000 },
		],
	};
}

function mount() {
	const model = createOpeningModel();
	render(StrongboxOpening, { props: { model } });
	void model.load();
	return model;
}

beforeEach(() => {
	vi.clearAllMocks();
	account.readRunWhole.mockResolvedValue({ readWhole: false, boxesLeft: 0 });
	api.getTrackingSnapshot.mockResolvedValue({ status: 'idle' });
	api.getLootInventory.mockResolvedValue([
		{
			itemName: 'Halloween Strongbox',
			quantity: 402,
			ttValue: 0,
			listedQuantity: 0,
			unitTt: 0,
			paidMarkup: 0,
			origins: [],
			derivedFrom: [],
		},
	]);
	api.getEquipmentInventory.mockResolvedValue([]);
	api.getStrongboxOpenings.mockResolvedValue([]);
	api.readStrongboxExport.mockResolvedValue(read);
	api.getMarketItemMarkups.mockResolvedValue({
		nanocubeMarkupPct: 102,
		items: [marketItem('Skeleton Shirt White (F)', 250), marketItem('Red Heart Fireworks', 101)],
	});
	api.recordStrongboxOpening.mockResolvedValue({ id: 'run-1' });
	account.getAccountStatus.mockResolvedValue(status('disconnected'));
	account.getProposals.mockResolvedValue([]);
	account.claimProposals.mockResolvedValue(undefined);
	account.releaseProposals.mockResolvedValue(undefined);
	account.linkProposals.mockResolvedValue(undefined);
	account.dismissProposal.mockResolvedValue(undefined);
	account.beginAccountRun.mockResolvedValue({ outcome: 'read', readingId: 10, takenAt: 'x' });
	account.finishAccountRun.mockResolvedValue({ outcome: 'read', readingId: 11, takenAt: 'y' });
	account.cancelAccountRun.mockResolvedValue(undefined);
});

function status(state: 'connected' | 'disconnected') {
	return {
		state,
		connectedAt: null,
		lastReadingAt: null,
		readingSession: false,
		hasConnected: state === 'connected',
		confirming: null,
		signingIn: false,
		hasReadings: true,
	};
}

async function openTheBoxes() {
	await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
	await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
}

describe('the strongbox opening view', () => {
	it('will not begin a run while tracking is on', async () => {
		api.getTrackingSnapshot.mockResolvedValue({ status: 'active' });
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		expect(screen.getByText('Stop tracking to open boxes.')).not.toBeNull();
		expect((screen.getByRole('button', { name: 'Continue' }) as HTMLButtonElement).disabled).toBe(
			true,
		);
	});

	it('reads a pasted export into a review and records the run', async () => {
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));

		const zone = await screen.findByLabelText('Inventory Changes export');
		await fireEvent.paste(zone, { clipboardData: { getData: () => EXPORT } });

		await waitFor(() =>
			expect(api.readStrongboxExport).toHaveBeenCalledWith({
				text: EXPORT,
				boxItem: 'Halloween Strongbox',
			}),
		);
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
		// The shirt with a reading is the hit and leads; the firework sells as
		// cubes and is cubed by default; the unvalued shirt is kept.
		expect(screen.getByText('1 hit worth more than cubes', { exact: false })).not.toBeNull();
		expect(screen.getByText('8 from your loot')).not.toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Record opening' }));
		await waitFor(() => expect(api.recordStrongboxOpening).toHaveBeenCalledTimes(1));
		const input = api.recordStrongboxOpening.mock.calls[0][0];
		expect(input.sources).toEqual([{ kind: 'held', quantity: 8 }]);
		expect(input.keyUnitTt).toBe(10);
		expect(input.ammoPed).toBe(58);
		expect(input.intake).toBe('export');
		expect(
			input.contents.map((row: { itemName: string; recycle: boolean }) => [
				row.itemName,
				row.recycle,
			]),
		).toEqual([
			// Every content is kept: recycling is the game's, and the
			// readings record it when it happens.
			['Skeleton Shirt White (F)', false],
			['Red Heart Fireworks', false],
			['Skeleton Shirt White (M)', false],
		]);
	});

	it('says why an export cannot be read, keeping the paste', async () => {
		api.readStrongboxExport.mockResolvedValue({
			...read,
			boxesOpened: 0,
			unexpected: [
				{
					row: { line: 5, itemName: 'Easter Strongbox', quantity: -8, valuePed: 0 },
					reason: 'other_box',
				},
			],
		});
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
		const zone = await screen.findByLabelText('Inventory Changes export');
		await fireEvent.paste(zone, { clipboardData: { getData: () => EXPORT } });
		expect(await screen.findByText(/shows Easter Strongbox opened/)).not.toBeNull();
		expect(api.recordStrongboxOpening).not.toHaveBeenCalled();
	});

	it('takes a result by hand, working out the ammo', async () => {
		mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Continue' }));
		await fireEvent.click(await screen.findByRole('button', { name: 'Enter by hand' }));

		await fireEvent.input(screen.getByLabelText('Boxes opened'), { target: { value: '4' } });
		await fireEvent.input(screen.getByLabelText('Item'), { target: { value: 'Nanocube' } });
		await fireEvent.input(screen.getByLabelText('Quantity'), { target: { value: '500' } });
		await fireEvent.input(screen.getByLabelText('TT value'), { target: { value: '5' } });

		expect(await screen.findByText('35.00')).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Review' }));
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
	});
});

describe('openings read from the account', () => {
	it('offers what the readings found, reviewed as an export: claimed, recorded, then linked', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
		expect(screen.getByText('6 of 7 paid ammo alone')).not.toBeNull();
		expect(screen.getByText('7 from your loot')).not.toBeNull();

		await fireEvent.click(screen.getByRole('button', { name: 'Record opening' }));
		await waitFor(() => expect(account.linkProposals).toHaveBeenCalledTimes(1));
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [7], retake: [] });
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: 'run-1' });
		expect(account.claimProposals.mock.invocationCallOrder[0]).toBeLessThan(
			api.recordStrongboxOpening.mock.invocationCallOrder[0],
		);
		expect(account.releaseProposals).not.toHaveBeenCalled();
		const input = api.recordStrongboxOpening.mock.calls[0][0];
		expect(input.intake).toBe('export');
		expect(input.ammoPed).toBe(60);
		expect(input.openedAt).toBe('2026-11-14');
	});

	it('may review a detected opening while tracking, and never records it twice', async () => {
		api.getTrackingSnapshot.mockResolvedValue({ status: 'active' });
		account.getProposals.mockResolvedValue([seven()]);
		let finish: (value: { id: string }) => void = () => {};
		api.recordStrongboxOpening.mockReturnValue(
			new Promise((resolve) => {
				finish = resolve;
			}),
		);
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		const first = model.record();
		const second = model.record();
		finish({ id: 'run-1' });
		await Promise.all([first, second]);
		expect(account.claimProposals).toHaveBeenCalledTimes(1);
		expect(api.recordStrongboxOpening).toHaveBeenCalledTimes(1);
		expect(account.linkProposals).toHaveBeenCalledTimes(1);
		expect(model.stage).toBe('done');
	});

	it('keeps an opening recorded when linking fails, leaving it claimed', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		account.linkProposals.mockRejectedValue(new Error('the database is busy'));
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		await model.record();
		expect(model.stage).toBe('done');
		expect(account.linkProposals).toHaveBeenCalledTimes(2);
		expect(account.releaseProposals).not.toHaveBeenCalled();
		expect(await screen.findByText('Opening recorded')).not.toBeNull();
	});

	it('shows a claimed proposal as possibly recorded, after a remount too', async () => {
		account.getProposals.mockResolvedValue([{ ...seven(), possiblyRecorded: true }]);
		const earlier = createOpeningModel();
		const first = render(StrongboxOpening, { props: { model: earlier } });
		void earlier.load();
		expect(
			await screen.findByText('This may already be recorded; check your openings first.'),
		).not.toBeNull();
		first.unmount();
		const model = mount();
		expect(
			await screen.findByText('This may already be recorded; check your openings first.'),
		).not.toBeNull();
		// Reviewed anyway: the old claim is taken over in the same claim.
		await fireEvent.click(screen.getByRole('button', { name: 'Review 7 Halloween Strongbox' }));
		await screen.findByText('Estimated markup');
		await model.record();
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [7], retake: [7] });
		expect(account.releaseProposals).not.toHaveBeenCalled();
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: 'run-1' });
	});

	it('releases the claim when the opening is refused, and it can be recorded again', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		api.recordStrongboxOpening.mockRejectedValueOnce(new Error('What came out does not add up'));
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		await model.record();
		expect(model.stage).toBe('review');
		expect(model.recordError).toBe('What came out does not add up');
		expect(account.releaseProposals).toHaveBeenCalledWith({ ids: [7] });
		expect(account.linkProposals).not.toHaveBeenCalled();
		await model.record();
		expect(account.claimProposals).toHaveBeenCalledTimes(2);
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: 'run-1' });
		expect(model.stage).toBe('done');
	});

	it('records nothing when the proposals cannot be claimed', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		account.claimProposals.mockRejectedValue(new Error('that proposal is no longer open'));
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		await model.record();
		expect(api.recordStrongboxOpening).not.toHaveBeenCalled();
		expect(model.recordError).toBe('that proposal is no longer open');
	});

	it('records the five with the keys less what else came out, saying what the rows read', async () => {
		account.getProposals.mockResolvedValue([five()]);
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 5 Halloween Strongbox' }),
		);
		expect(
			await screen.findByText(
				'Your account shows 49.95 PED of ammo; the keys less what else came out are 50.00 PED, and that is the figure recorded.',
			),
		).not.toBeNull();
		expect(screen.getByText('4 of 5 paid ammo alone')).not.toBeNull();
		// The fireworks have no TT: counted, never "0.00 TT".
		expect(screen.getByText('×1')).not.toBeNull();
		expect(document.body.textContent).not.toMatch(/0\.00 TT/);
		await model.record();
		expect(api.recordStrongboxOpening.mock.calls[0][0].ammoPed).toBe(50);
	});

	it('drops a detected opening reviewed after the player moved on and came back', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		let markets: (value: unknown) => void = () => {};
		api.getMarketItemMarkups.mockReturnValue(
			new Promise((resolve) => {
				markets = resolve;
			}),
		);
		const model = mount();
		await fireEvent.click(await screen.findByRole('radio', { name: /Halloween Strongbox/ }));
		const review = model.reviewProposal(7);
		model.begin();
		model.back();
		// Back at the first stage, as when the review was asked for; only
		// the run's generation has moved on.
		expect(model.stage).toBe('choose');
		markets({ nanocubeMarkupPct: 102, items: [] });
		await review;
		expect(model.stage).toBe('choose');
		expect(model.result).toBeNull();
	});

	it('keeps a possibly recorded proposal claimed when recording it again fails', async () => {
		account.getProposals.mockResolvedValue([{ ...seven(), possiblyRecorded: true }]);
		api.recordStrongboxOpening.mockRejectedValueOnce(new Error('the database is busy'));
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		await model.record();
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [7], retake: [7] });
		// It was claimed before this attempt: its claim, and its warning, stay.
		expect(account.releaseProposals).not.toHaveBeenCalled();
		model.back();
		expect(
			await screen.findByText('This may already be recorded; check your openings first.'),
		).not.toBeNull();
		// Reviewed and recorded again: taken over and linked.
		await fireEvent.click(screen.getByRole('button', { name: 'Review 7 Halloween Strongbox' }));
		await screen.findByText('Estimated markup');
		await model.record();
		expect(account.claimProposals).toHaveBeenLastCalledWith({ ids: [7], retake: [7] });
		expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: 'run-1' });
	});

	it('takes over its own claim when a failed recording could not release it', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		api.recordStrongboxOpening.mockRejectedValueOnce(new Error('the database is busy'));
		account.releaseProposals.mockRejectedValueOnce(new Error('the database is busy'));
		const model = mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Review 7 Halloween Strongbox' }),
		);
		await screen.findByText('Estimated markup');
		await model.record();
		expect(account.claimProposals).toHaveBeenLastCalledWith({ ids: [7], retake: [] });
		await model.record();
		expect(account.claimProposals).toHaveBeenLastCalledWith({ ids: [7], retake: [7] });
		expect(model.stage).toBe('done');
	});

	it("shows an unsettled opening's doubt and leaves what may be its own out", async () => {
		account.getProposals.mockResolvedValue([unsettled()]);
		mount();
		expect(await screen.findByText(', not settled', { exact: false })).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Review 7 Halloween Strongbox' }));
		expect(await screen.findByText('6 keys were used for 7 boxes.')).not.toBeNull();
		const add = screen.getByLabelText(/Add Skeleton Pants Red \(F\)/) as HTMLInputElement;
		expect(add.checked).toBe(false);
		await fireEvent.click(add);
		expect(add.checked).toBe(true);
	});

	it('dismisses one the player says is not an opening', async () => {
		account.getProposals.mockResolvedValue([seven()]);
		mount();
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Dismiss 7 Halloween Strongbox' }),
		);
		expect(account.dismissProposal).toHaveBeenCalledWith({ id: 7 });
	});

	it('reads a run from the connected account, offered first', async () => {
		account.getAccountStatus.mockResolvedValue(status('connected'));
		const run = { ...seven(), previousReadingId: 10, readingId: 11 };
		account.getProposals.mockResolvedValue([]);
		const model = mount();
		await waitFor(() => expect(model.accountConnected).toBe(true));
		await openTheBoxes();
		expect(account.beginAccountRun).toHaveBeenCalledTimes(1);
		expect(await screen.findByText('Open your Halloween Strongbox in game.')).not.toBeNull();
		account.getProposals.mockResolvedValue([run]);
		await fireEvent.click(screen.getByRole('button', { name: 'Done' }));
		expect(account.finishAccountRun).toHaveBeenCalledTimes(1);
		expect(await screen.findByText('Estimated markup')).not.toBeNull();
		expect(screen.getByText('6 of 7 paid ammo alone')).not.toBeNull();
	});

	it('says so plainly when the run shows nothing, and offers the other ways', async () => {
		account.getAccountStatus.mockResolvedValue(status('connected'));
		const model = mount();
		await waitFor(() => expect(model.accountConnected).toBe(true));
		await openTheBoxes();
		await fireEvent.click(await screen.findByRole('button', { name: 'Done' }));
		expect(
			await screen.findByText(/Your account does not show any Halloween Strongbox opened yet/),
		).not.toBeNull();
		// The site may be a minute behind: the next reading is the player's to ask for.
		expect(screen.getByRole('button', { name: 'Check the next reading' })).not.toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Paste the export' }));
		expect(await screen.findByLabelText('Inventory Changes export')).not.toBeNull();
	});

	it('drops a reading that answers after the player moved on, and stops the run with the view', async () => {
		account.getAccountStatus.mockResolvedValue(status('connected'));
		let answer: (value: unknown) => void = () => {};
		account.finishAccountRun.mockReturnValue(
			new Promise((resolve) => {
				answer = resolve;
			}),
		);
		const model = mount();
		await waitFor(() => expect(model.accountConnected).toBe(true));
		await openTheBoxes();
		await fireEvent.click(await screen.findByRole('button', { name: 'Done' }));
		expect(await screen.findByText(/which can take up to a minute/)).not.toBeNull();
		model.back();
		account.getProposals.mockResolvedValue([seven()]);
		answer({ outcome: 'read', readingId: 11, takenAt: 'y' });
		await Promise.resolve();
		expect(model.stage).toBe('choose');
		expect(model.result).toBeNull();
		expect(account.cancelAccountRun).toHaveBeenCalled();

		await openTheBoxes();
		await screen.findByText('Open your Halloween Strongbox in game.');
		const cancels = account.cancelAccountRun.mock.calls.length;
		model.dispose();
		expect(account.cancelAccountRun.mock.calls.length).toBe(cancels + 1);
	});

	it('starts again only once the run before it is closed', async () => {
		account.getAccountStatus.mockResolvedValue(status('connected'));
		account.beginAccountRun.mockResolvedValueOnce({ outcome: 'unreadable' });
		const model = mount();
		await waitFor(() => expect(model.accountConnected).toBe(true));
		await openTheBoxes();
		expect(await screen.findByText(/could not be read just now/)).not.toBeNull();
		let cancelled: (value?: unknown) => void = () => {};
		account.cancelAccountRun.mockReturnValueOnce(
			new Promise((resolve) => {
				cancelled = resolve;
			}),
		);
		const again = model.readAccountAgain();
		await Promise.resolve();
		expect(account.cancelAccountRun).toHaveBeenCalledTimes(1);
		expect(account.beginAccountRun).toHaveBeenCalledTimes(1);
		cancelled();
		await again;
		expect(account.beginAccountRun).toHaveBeenCalledTimes(2);
		expect(account.cancelAccountRun.mock.invocationCallOrder[0]).toBeLessThan(
			account.beginAccountRun.mock.invocationCallOrder[1],
		);
	});

	it('stops the run when the player leaves after it failed', async () => {
		account.getAccountStatus.mockResolvedValue(status('connected'));
		account.beginAccountRun.mockResolvedValue({ outcome: 'unreadable' });
		const model = mount();
		await waitFor(() => expect(model.accountConnected).toBe(true));
		await openTheBoxes();
		expect(await screen.findByText(/could not be read just now/)).not.toBeNull();
		model.back();
		expect(account.cancelAccountRun).toHaveBeenCalledTimes(1);
	});

	it('looks as it did without a connection', async () => {
		mount();
		await openTheBoxes();
		expect(screen.queryByRole('radio', { name: 'Read from your account' })).toBeNull();
		expect(await screen.findByLabelText('Inventory Changes export')).not.toBeNull();
		expect(account.beginAccountRun).not.toHaveBeenCalled();
	});
});
