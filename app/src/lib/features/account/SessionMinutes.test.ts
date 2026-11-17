// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	AccountSessionMinutes,
	ArrivalDisposition,
	AssetPurchase,
	InventoryItem,
} from '$lib/api/commands.gen';
import { formatTime } from '$lib/utils/format';
import { arrival, minutes, missing, open, step, stepSpan, stretch } from './__fixtures__/minutes';

const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getSessionMinutes: vi.fn(),
	recordArrivalUnknownOrigin: vi.fn(),
	undoArrivalUnknownOrigin: vi.fn(),
	unignoreArrival: vi.fn(),
	dismissProposal: vi.fn(),
	claimProposals: vi.fn(),
	releaseProposals: vi.fn(),
	linkProposals: vi.fn(),
}));
const inventory = vi.hoisted(() => ({
	getEquipmentInventory: vi.fn(),
	addAssetPurchase: vi.fn(),
	addEquipmentHolding: vi.fn(),
	updateEquipmentHolding: vi.fn(),
}));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/account', () => account);
vi.mock('$lib/api/inventory', () => inventory);
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import SessionMinutes from './SessionMinutes.svelte';

beforeAll(() => {
	// The forms' transitions run on the Web Animations API.
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

const at = (index: number) => formatTime(stepSpan(index).to);

/** Six minutes: an arrival nothing explains in the second, loot that never
 * arrived flagged in the third, the weapon drifting from the third on, and
 * three quiet minutes. */
function series(overrides: Partial<AccountSessionMinutes> = {}): AccountSessionMinutes {
	return minutes({
		steps: [
			step(0),
			step(1, { arrivedNotLooted: [arrival('Foo Rifle')] }),
			step(2, { lootedNeverArrived: [missing('Shrapnel', 2)] }),
			step(3),
			step(4),
			step(5),
		],
		weaponStretches: [stretch(2, 5, 10, 11.8, -1.5)],
		...overrides,
	});
}

/** The same minutes once the arrival was decided. */
function decided(disposition: ArrivalDisposition): AccountSessionMinutes {
	const shown = series();
	return {
		...shown,
		steps: shown.steps.map((minute, index) =>
			index === 1
				? { ...minute, arrivedNotLooted: [arrival('Foo Rifle', { disposition })] }
				: minute,
		),
	};
}

/** The minutes with the arrival bought at the Trade Terminal: the PED Card
 * fell by its TT. */
function boughtAtTt(): AccountSessionMinutes {
	const shown = series();
	return {
		...shown,
		steps: shown.steps.map((minute, index) =>
			index === 1
				? { ...minute, arrivedNotLooted: [arrival('Foo Rifle', { boughtAtTt: true })] }
				: minute,
		),
	};
}

function asset(id: string, name: string, purchases: AssetPurchase[] = []): InventoryItem {
	return {
		id,
		name,
		ttValue: 5,
		markupPaid: 0,
		notes: null,
		acquiredAt: '2026-11-01',
		quantity: 1,
		divisible: true,
		purchases,
		origins: [],
		derivedFrom: [],
	};
}

function purchase(id: number): AssetPurchase {
	return {
		id,
		quantity: 1,
		ttValue: 5,
		markupPaid: 0,
		acquiredAt: '2026-11-15',
		fromConversion: false,
		editable: true,
	};
}

/** A read the test answers when it chooses. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

interface Shown {
	onremoveloot?: ((itemName: string) => Promise<void>) | null;
	reviews?: { weapon?: (() => void) | null; healing?: (() => void) | null } | null;
}

async function show(props: Shown = {}) {
	render(SessionMinutes, { props: { sessionId: 's1', ...props } });
	const toggle = await screen.findByRole('button', { name: /^Minute by minute/ });
	return toggle;
}

async function opened(props: Shown = {}) {
	const toggle = await show(props);
	await fireEvent.click(toggle);
	return screen.getByTestId('minutes-detail');
}

/** Turn the filter off: list every minute. */
async function everyMinute() {
	await fireEvent.click(screen.getByRole('button', { name: 'Only minutes that need a look' }));
}

/** The flag that names `text`. */
function flag(text: string | RegExp): HTMLElement {
	const found = screen.getByText(text).closest<HTMLElement>('[data-flag]');
	if (!found) throw new Error(`no flag for ${text}`);
	return found;
}

const FOO = /Foo Rifle, 5\.00 TT/;

/** What the status line says now. */
const statusText = () => screen.getByTestId('minutes-status').textContent;

/** Type what was paid above TT into the asset form open. */
async function payMarkup(label: string, ped: string) {
	await fireEvent.input(await screen.findByLabelText(label), { target: { value: ped } });
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
	account.getSessionMinutes.mockResolvedValue(series());
	account.dismissProposal.mockResolvedValue(undefined);
	account.claimProposals.mockResolvedValue(undefined);
	account.releaseProposals.mockResolvedValue(undefined);
	account.linkProposals.mockResolvedValue(undefined);
	inventory.getEquipmentInventory.mockResolvedValue([]);
});

describe('the session minute by minute', () => {
	it('renders nothing for a session the readings give no step of', async () => {
		account.getSessionMinutes.mockResolvedValue(minutes());
		const { container } = render(SessionMinutes, { props: { sessionId: 's1' } });
		await waitFor(() => expect(account.getSessionMinutes).toHaveBeenCalledWith('s1'));
		await account.getSessionMinutes.mock.results[0].value;
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});

	it('is collapsed, counting only what the player can act on in its toggle', async () => {
		const toggle = await show();
		// The arrival and the loot the correction can remove; the drifting
		// stretch is listed, not counted.
		expect(toggle.textContent?.trim()).toBe('Minute by minute, 2 need a look');
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(screen.queryByTestId('minutes-detail')).toBeNull();
	});

	it('lists only the minutes worth a look until asked for every minute', async () => {
		await opened();
		const filter = screen.getByRole('button', { name: 'Only minutes that need a look' });
		expect(filter.getAttribute('aria-pressed')).toBe('true');
		expect(screen.getAllByTestId('minute-row')).toHaveLength(2);

		await fireEvent.click(filter);
		expect(filter.getAttribute('aria-pressed')).toBe('false');
		expect(screen.getAllByTestId('minute-row')).toHaveLength(6);
	});

	it('lists every minute, with no filter, when nothing is worth a look', async () => {
		account.getSessionMinutes.mockResolvedValue(minutes({ steps: [step(0), step(1), step(2)] }));
		const toggle = await show();
		expect(toggle.textContent?.trim()).toBe('Minute by minute');
		await fireEvent.click(toggle);
		expect(screen.queryByRole('button', { name: 'Only minutes that need a look' })).toBeNull();
		expect(screen.getAllByTestId('minute-row')).toHaveLength(3);
	});

	it('lists a long session 120 minutes at a time', async () => {
		const steps = Array.from({ length: 130 }, (_, index) => step(index));
		account.getSessionMinutes.mockResolvedValue(minutes({ steps }));
		await opened();
		expect(screen.getAllByTestId('minute-row')).toHaveLength(120);
		await fireEvent.click(screen.getByRole('button', { name: 'Show 10 more minutes' }));
		expect(screen.getAllByTestId('minute-row')).toHaveLength(130);
		expect(screen.queryByRole('button', { name: /more minutes/ })).toBeNull();
	});

	it('says each flag as a sentence', async () => {
		await opened();
		expect(
			screen.getByText(`Foo Rifle, 5.00 TT, arrived at ${at(1)}: no loot line names it`),
		).toBeTruthy();
		// Flagged a step after the one it was looted in.
		expect(
			screen.getByText(`3 Shrapnel, 0.30 TT, looted at ${at(1)}: it never arrived`),
		).toBeTruthy();
		expect(
			screen.getByText(
				`From ${formatTime(stepSpan(2).from)} the tracked weapon cost ran 15% further above what your account measured than over the rest of the session: check the weapon's cost settings`,
			),
		).toBeTruthy();
	});

	it('names every figure for a screen reader', async () => {
		await opened();
		const row = screen.getAllByTestId('minute-row')[0];
		for (const label of [
			'Minute to',
			'Loot arrived / looted:',
			'Weapon measured / tracked:',
			'Healing measured / tracked:',
			'Armour measured:',
		]) {
			expect(within(row).getByText(label).className).toContain('sr-only');
		}
	});

	it('shows a figure the readings could not measure as absent, with why', async () => {
		account.getSessionMinutes.mockResolvedValue(
			minutes({
				steps: [
					step(0, {
						weapon: { measured: { kind: 'unmeasured', reason: 'ammoUnsettled' }, tracked: 1.5 },
					}),
				],
			}),
		);
		await opened();
		const row = screen.getByTestId('minute-row');
		expect(
			within(row).getByTitle('How much of the ammo was fired in this minute could not be settled'),
		).toBeTruthy();
		expect(within(row).getByText('/ 1.50')).toBeTruthy();
	});
});

describe('deciding an arrival', () => {
	it('ignores it, reads the minutes again, and says so', async () => {
		await opened();
		const status = screen.getByTestId('minutes-status');
		expect(status.textContent).toBe('');
		account.getSessionMinutes.mockResolvedValue(decided({ state: 'ignored', proposalId: 7 }));
		await fireEvent.click(screen.getByRole('button', { name: 'Ignore Foo Rifle' }));
		await waitFor(() => expect(account.dismissProposal).toHaveBeenCalledWith({ id: 7 }));
		await waitFor(() => expect(status.textContent).toBe('Foo Rifle ignored.'));
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(2);
		expect(screen.getByText('Ignored: not offered again.')).toBeTruthy();
		// Decided here, it stays in view though it no longer needs a look,
		// and focus is back on it.
		await waitFor(() => expect(document.activeElement).toBe(flag(FOO)));
	});

	it("says the backend's reason when it refuses to ignore it", async () => {
		await opened();
		account.dismissProposal.mockRejectedValue(
			new Error('stock recorded from this arrival still stands; take it back first'),
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Ignore Foo Rifle' }));
		const shown = await screen.findByText(
			'stock recorded from this arrival still stands; take it back first',
		);
		expect(shown.closest('[data-flag]')).toBe(flag(FOO));
		expect(statusText()).toBe('');
	});

	it('offers an ignored arrival again, then reads the minutes again', async () => {
		account.getSessionMinutes.mockResolvedValue(decided({ state: 'ignored', proposalId: 7 }));
		account.unignoreArrival.mockResolvedValue(undefined);
		await opened();
		await everyMinute();
		account.getSessionMinutes.mockResolvedValue(series());
		await fireEvent.click(screen.getByRole('button', { name: 'Offer Foo Rifle again' }));
		await waitFor(() => expect(account.unignoreArrival).toHaveBeenCalledWith({ id: 7 }));
		expect(await screen.findByRole('button', { name: 'Ignore Foo Rifle' })).toBeTruthy();
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(2);
	});

	it('adds it to stock as unknown origin, and takes it back', async () => {
		await opened();
		account.recordArrivalUnknownOrigin.mockResolvedValue({ recordId: 'r-1', linked: true });
		account.getSessionMinutes.mockResolvedValue(
			decided({ state: 'recorded', proposalId: 7, recordId: 'r-1', as: 'unknownOrigin' }),
		);
		await fireEvent.click(
			screen.getByRole('button', { name: 'Add Foo Rifle to stock as unknown origin' }),
		);
		await waitFor(() =>
			expect(account.recordArrivalUnknownOrigin).toHaveBeenCalledWith({ id: 7, retake: false }),
		);
		expect(await screen.findByText('Added to stock as unknown origin.')).toBeTruthy();
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(2);

		account.undoArrivalUnknownOrigin.mockResolvedValue(undefined);
		account.getSessionMinutes.mockResolvedValue(series());
		await fireEvent.click(screen.getByRole('button', { name: 'Take back Foo Rifle from stock' }));
		await waitFor(() =>
			expect(account.undoArrivalUnknownOrigin).toHaveBeenCalledWith({ id: 'r-1' }),
		);
		expect(
			await screen.findByRole('button', { name: 'Add Foo Rifle to stock as unknown origin' }),
		).toBeTruthy();
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(3);
	});

	it('takes back stock of unknown origin recorded before, by its record', async () => {
		account.getSessionMinutes.mockResolvedValue(
			decided({ state: 'recorded', proposalId: 7, recordId: 'r-5', as: 'unknownOrigin' }),
		);
		account.undoArrivalUnknownOrigin.mockResolvedValue(undefined);
		await opened();
		await everyMinute();
		expect(screen.getByText('Added to stock as unknown origin.')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Take back Foo Rifle from stock' }));
		await waitFor(() =>
			expect(account.undoArrivalUnknownOrigin).toHaveBeenCalledWith({ id: 'r-5' }),
		);
	});

	it('points a purchase recorded before to the asset in Inventory, with no undo here', async () => {
		account.getSessionMinutes.mockResolvedValue(
			decided({
				state: 'recorded',
				proposalId: 7,
				recordId: '55',
				as: 'purchase',
				assetName: 'Foo Rifle',
			}),
		);
		await opened();
		await everyMinute();
		const said = screen.getByTestId('recorded-purchase');
		expect(said.textContent).toMatch(
			/Recorded as a purchase of Foo Rifle\.\s+To take it back, remove that purchase from the asset in\s+Inventory\./,
		);
		expect(within(said).getByRole('link', { name: 'Inventory' }).getAttribute('href')).toBe(
			'/inventory',
		);
		expect(within(flag(FOO)).queryAllByRole('button')).toHaveLength(0);
	});

	it('says why an arrival is not offered, with nothing to do about it here', async () => {
		account.getSessionMinutes.mockResolvedValue(
			decided({ state: 'notProposed', reason: 'outsideSession' }),
		);
		await opened();
		await everyMinute();
		expect(
			screen.getByText("After the session: any purchase is offered with your account's purchases."),
		).toBeTruthy();
		expect(within(flag(FOO)).queryAllByRole('button')).toHaveLength(0);
	});

	it('offers only marking done or taking back while its stock stands unlinked', async () => {
		await opened();
		account.recordArrivalUnknownOrigin.mockResolvedValue({ recordId: 'r-3', linked: false });
		account.getSessionMinutes.mockResolvedValue(decided(open(7, true)));
		await fireEvent.click(
			screen.getByRole('button', { name: 'Add Foo Rifle to stock as unknown origin' }),
		);
		expect(await screen.findByText('Added to stock, but not marked done.')).toBeTruthy();
		const buttons = within(flag(FOO)).getAllByRole('button');
		const labels = buttons.map((button) => button.textContent?.trim());
		expect(labels).toEqual(['Mark as recorded', 'Undo']);

		await fireEvent.click(screen.getByRole('button', { name: 'Mark as recorded: Foo Rifle' }));
		await waitFor(() =>
			expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: 'r-3' }),
		);
		// Marking it done adds no stock again.
		expect(account.recordArrivalUnknownOrigin).toHaveBeenCalledTimes(1);
	});

	it('takes back stock the read says still stands behind an open arrival', async () => {
		// The record the stock stands behind, as the read carries it.
		const standing = {
			state: 'open' as const,
			proposalId: 7,
			possiblyRecorded: true,
			unlinkedRecordId: 'r-8',
		};
		account.getSessionMinutes.mockResolvedValue(decided(standing));
		account.undoArrivalUnknownOrigin.mockResolvedValue(undefined);
		await opened();
		expect(screen.getByText('Added to stock, but not marked done.')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Ignore Foo Rifle' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Record Foo Rifle as a purchase' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Take back Foo Rifle from stock' }));
		await waitFor(() =>
			expect(account.undoArrivalUnknownOrigin).toHaveBeenCalledWith({ id: 'r-8' }),
		);
	});

	it("says the backend's reason when it cannot be added to stock, and reads again", async () => {
		await opened();
		account.recordArrivalUnknownOrigin.mockRejectedValue(
			new Error('the stock could not be recorded; nothing was changed'),
		);
		await fireEvent.click(
			screen.getByRole('button', { name: 'Add Foo Rifle to stock as unknown origin' }),
		);
		const shown = await screen.findByText('the stock could not be recorded; nothing was changed');
		expect(shown.closest('[data-flag]')).toBe(flag(FOO));
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(2);
	});

	it('asks first when it may be recorded already, then takes over the old claim', async () => {
		account.getSessionMinutes.mockResolvedValue(decided(open(7, true)));
		await opened();
		expect(screen.getByText('This may already be recorded; check your assets first.')).toBeTruthy();
		account.recordArrivalUnknownOrigin.mockResolvedValue({ recordId: 'r-2', linked: true });
		await fireEvent.click(
			screen.getByRole('button', { name: 'Add Foo Rifle to stock as unknown origin' }),
		);
		expect(account.recordArrivalUnknownOrigin).not.toHaveBeenCalled();
		await fireEvent.click(screen.getByRole('button', { name: 'Record again' }));
		await waitFor(() =>
			expect(account.recordArrivalUnknownOrigin).toHaveBeenCalledWith({ id: 7, retake: true }),
		);
	});

	it('asks first on the purchase path too, with focus on the answer, and Escape cancels', async () => {
		account.getSessionMinutes.mockResolvedValue(decided(open(7, true)));
		await opened();
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		const again = screen.getByRole('button', { name: 'Record again' });
		await waitFor(() => expect(document.activeElement).toBe(again));
		expect(inventory.getEquipmentInventory).not.toHaveBeenCalled();

		await fireEvent.keyDown(again, { key: 'Escape' });
		expect(screen.queryByRole('button', { name: 'Record again' })).toBeNull();
		await waitFor(() => expect(document.activeElement).toBe(flag(FOO)));

		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Record again' }));
		inventory.addEquipmentHolding.mockResolvedValue(asset('a-9', 'Foo Rifle', [purchase(55)]));
		await payMarkup('MU paid in PED', '2');
		await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
		await waitFor(() =>
			expect(account.claimProposals).toHaveBeenCalledWith({ ids: [7], retake: [7] }),
		);
	});

	it('records it as a new asset through the asset form, then links the purchase', async () => {
		await opened();
		inventory.addEquipmentHolding.mockResolvedValue(asset('a-9', 'Foo Rifle', [purchase(55)]));
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await waitFor(() => expect(inventory.getEquipmentInventory).toHaveBeenCalled());

		const name = (await screen.findByLabelText('Name')) as HTMLInputElement;
		expect(name.value).toBe('Foo Rifle');
		// What was paid starts empty and must be given.
		const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement;
		expect((screen.getByLabelText('MU paid in PED') as HTMLInputElement).value).toBe('');
		expect(add.disabled).toBe(true);
		await payMarkup('MU paid in PED', '2');
		expect(add.disabled).toBe(false);

		account.getSessionMinutes.mockResolvedValue(
			decided({ state: 'recorded', proposalId: 7, recordId: '55', as: 'purchase' }),
		);
		await fireEvent.click(add);

		await waitFor(() =>
			expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: '55' }),
		);
		expect(account.claimProposals).toHaveBeenCalledWith({ ids: [7], retake: [] });
		expect(inventory.addEquipmentHolding).toHaveBeenCalledWith(
			expect.objectContaining({ name: 'Foo Rifle', quantity: 1, tt_value: 5, markup_paid: 2 }),
		);
		const recorded = await within(flag(FOO)).findByTestId('recorded-purchase');
		expect(recorded.textContent).toContain('Recorded as a purchase of Foo Rifle.');
		await waitFor(() => expect(statusText()).toBe('Foo Rifle recorded as a purchase.'));
	});

	it('starts what was paid at nothing for something the PED Card bought at its TT', async () => {
		account.getSessionMinutes.mockResolvedValue(boughtAtTt());
		await opened();
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await screen.findByLabelText('Name');
		expect((screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement).disabled).toBe(false);
	});

	it('returns focus to the flag when the asset form is cancelled', async () => {
		await opened();
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await screen.findByLabelText('Name');
		await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
		await waitFor(() => expect(document.activeElement).toBe(flag(FOO)));
		expect(account.claimProposals).not.toHaveBeenCalled();
	});

	it('offers to mark it recorded when the purchase was saved but could not be marked done', async () => {
		await opened();
		inventory.addEquipmentHolding.mockResolvedValue(asset('a-9', 'Foo Rifle', [purchase(55)]));
		account.linkProposals.mockRejectedValue(new Error('the database is busy'));
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await payMarkup('MU paid in PED', '2');
		await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
		expect(await screen.findByText('Recorded in Foo Rifle, but not marked done.')).toBeTruthy();
		expect(account.linkProposals).toHaveBeenCalledTimes(2);

		account.linkProposals.mockResolvedValue(undefined);
		await fireEvent.click(screen.getByRole('button', { name: 'Mark as recorded: Foo Rifle' }));
		await waitFor(() => expect(account.linkProposals).toHaveBeenCalledTimes(3));
		expect(account.linkProposals).toHaveBeenLastCalledWith({ ids: [7], recordId: '55' });
		// Saved once: marking it recorded saves nothing again.
		expect(inventory.addEquipmentHolding).toHaveBeenCalledTimes(1);
	});

	it('adds to the asset held under its name, through the purchase form', async () => {
		inventory.getEquipmentInventory.mockResolvedValue([asset('a-3', 'Foo Rifle')]);
		inventory.addAssetPurchase.mockResolvedValue(asset('a-3', 'Foo Rifle', [purchase(61)]));
		await opened();
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		const add = (await screen.findByRole('button', { name: 'Add purchase' })) as HTMLButtonElement;
		expect(add.disabled).toBe(true);
		await payMarkup('MU paid, all units in PED', '1.5');
		await fireEvent.click(add);
		await waitFor(() =>
			expect(inventory.addAssetPurchase).toHaveBeenCalledWith(
				'a-3',
				expect.objectContaining({ quantity: 1, ttValue: 5, markupPaid: 1.5 }),
			),
		);
		await waitFor(() =>
			expect(account.linkProposals).toHaveBeenCalledWith({ ids: [7], recordId: '61' }),
		);
	});

	it('releases the claim and keeps the form open when the save fails', async () => {
		await opened();
		inventory.addEquipmentHolding.mockRejectedValue(new Error('the name is already taken'));
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		await payMarkup('MU paid in PED', '2');
		await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
		expect(await screen.findByText('the name is already taken')).toBeTruthy();
		expect(account.releaseProposals).toHaveBeenCalledWith({ ids: [7] });
		expect(account.linkProposals).not.toHaveBeenCalled();
	});

	it('says why when more than one asset has its name', async () => {
		inventory.getEquipmentInventory.mockResolvedValue([
			asset('a-1', 'Foo Rifle'),
			asset('a-2', 'foo rifle'),
		]);
		await opened();
		await fireEvent.click(screen.getByRole('button', { name: 'Record Foo Rifle as a purchase' }));
		expect(
			await screen.findByText(
				"More than one asset has this name: add it from the right asset's row in Inventory, then ignore this.",
			),
		).toBeTruthy();
		expect(account.claimProposals).not.toHaveBeenCalled();
	});
});

describe('loot that never arrived', () => {
	/** Shrapnel that never arrived, flagged in two minutes. */
	function twice(): AccountSessionMinutes {
		return series({
			steps: [
				step(0),
				step(1),
				step(2, { lootedNeverArrived: [missing('Shrapnel', 2)] }),
				step(3),
				step(4, { lootedNeverArrived: [missing('Shrapnel', 4, { units: 2 })] }),
				step(5),
			],
		});
	}

	it("asks with the correction's own question, counting every line it archives", async () => {
		account.getSessionMinutes.mockResolvedValue(twice());
		const onremoveloot = vi.fn().mockResolvedValue(undefined);
		await opened({ onremoveloot });
		const remove = screen.getAllByRole('button', {
			name: "Remove Shrapnel from this session's loot",
		});
		expect(remove).toHaveLength(2);
		await fireEvent.click(remove[0]);
		expect(screen.getByText('Archive all 5 "Shrapnel" entries?')).toBeTruthy();
		await waitFor(() =>
			expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Cancel' })),
		);

		await fireEvent.click(screen.getByRole('button', { name: 'Yes' }));
		await waitFor(() => expect(onremoveloot).toHaveBeenCalledWith('Shrapnel'));
		await waitFor(() => expect(account.getSessionMinutes).toHaveBeenCalledTimes(2));
		await waitFor(() => expect(statusText()).toBe("Shrapnel removed from this session's loot."));
	});

	it('cancels the question with Escape, and focus returns to the flag', async () => {
		await opened({ onremoveloot: vi.fn() });
		await fireEvent.click(
			screen.getByRole('button', { name: "Remove Shrapnel from this session's loot" }),
		);
		const cancel = screen.getByRole('button', { name: 'Cancel' });
		await waitFor(() => expect(document.activeElement).toBe(cancel));
		await fireEvent.keyDown(cancel, { key: 'Escape' });
		expect(screen.queryByText('Archive all 3 "Shrapnel" entries?')).toBeNull();
		await waitFor(() => expect(document.activeElement).toBe(flag(/3 Shrapnel/)));
	});

	it('says why removing it failed', async () => {
		const onremoveloot = vi.fn().mockRejectedValue(new Error('Session not found'));
		await opened({ onremoveloot });
		await fireEvent.click(
			screen.getByRole('button', { name: "Remove Shrapnel from this session's loot" }),
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Yes' }));
		expect(await screen.findByText('Session not found')).toBeTruthy();
	});

	it('offers no removal where the correction is not offered, and says why', async () => {
		account.getSessionMinutes.mockResolvedValue(
			series({
				steps: [
					step(0),
					step(1),
					step(2, {
						lootedNeverArrived: [
							missing('Shrapnel', 2, {
								correction: { kind: 'notOffered', reason: 'otherLinesArrived' },
							}),
						],
					}),
					step(3),
					step(4),
					step(5),
				],
			}),
		);
		await opened({ onremoveloot: vi.fn() });
		expect(
			screen.queryByRole('button', { name: "Remove Shrapnel from this session's loot" }),
		).toBeNull();
		expect(
			screen.getByText(
				'Other loot of this name in the session did arrive, and removing it takes out every line of it.',
			),
		).toBeTruthy();
	});

	it('offers no removal for a correction that names another session', async () => {
		account.getSessionMinutes.mockResolvedValue(
			series({
				steps: [
					step(0),
					step(1),
					step(2, {
						lootedNeverArrived: [
							missing('Shrapnel', 2, {
								correction: { kind: 'offered', sessionId: 's2', itemName: 'Shrapnel' },
							}),
						],
					}),
					step(3),
					step(4),
					step(5),
				],
			}),
		);
		await opened({ onremoveloot: vi.fn() });
		expect(
			screen.queryByRole('button', { name: "Remove Shrapnel from this session's loot" }),
		).toBeNull();
	});
});

describe('a drifting cost', () => {
	it("links to the session's own review of the lane where it has one", async () => {
		const weapon = vi.fn();
		await opened({ reviews: { weapon, healing: null } });
		await fireEvent.click(screen.getByRole('button', { name: 'Open weapon attribution' }));
		expect(weapon).toHaveBeenCalledTimes(1);
	});

	it('offers no link where the session shows no review of the lane', async () => {
		await opened();
		expect(screen.queryByRole('button', { name: 'Open weapon attribution' })).toBeNull();
	});
});

describe('reading the minutes', () => {
	it('says why the first read failed, and reads again when asked', async () => {
		account.getSessionMinutes.mockRejectedValueOnce(
			new Error('the account connection was not composed'),
		);
		render(SessionMinutes, { props: { sessionId: 's1' } });
		expect(await screen.findByText('the account connection was not composed')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
		expect(await screen.findByRole('button', { name: /^Minute by minute/ })).toBeTruthy();
	});

	it('reads again on an account frame', async () => {
		await show();
		expect(account.getSessionMinutes).toHaveBeenCalledTimes(1);
		account.getSessionMinutes.mockResolvedValue(minutes({ steps: [step(0)] }));
		for (const handler of frames.handlers) handler();
		const label = () => screen.getByRole('button', { name: /^Minute by minute/ }).textContent;
		await waitFor(() => expect(label()?.trim()).toBe('Minute by minute'));
	});

	it('never shows minutes read for a session no longer shown, and starts afresh', async () => {
		const late = deferred<AccountSessionMinutes>();
		const quiet = minutes({ sessionId: 's2', steps: [step(0)] });
		account.getSessionMinutes.mockImplementation((id: string) =>
			id === 's1' ? late.promise : Promise.resolve(quiet),
		);
		const { rerender } = render(SessionMinutes, { props: { sessionId: 's1' } });
		await waitFor(() => expect(account.getSessionMinutes).toHaveBeenCalledWith('s1'));
		await rerender({ sessionId: 's2' });

		late.resolve(series());
		await waitFor(() => expect(account.getSessionMinutes).toHaveBeenCalledWith('s2'));
		const toggle = await screen.findByRole('button', { name: /^Minute by minute/ });
		await waitFor(() => expect(toggle.textContent?.trim()).toBe('Minute by minute'));
		expect(screen.queryByText(/need a look/)).toBeNull();
	});

	it('closes the list and clears what it said when another session is shown', async () => {
		account.getSessionMinutes.mockImplementation((id: string) =>
			Promise.resolve(id === 's1' ? series() : minutes({ sessionId: 's2', steps: [step(0)] })),
		);
		const { rerender } = render(SessionMinutes, { props: { sessionId: 's1' } });
		await fireEvent.click(await screen.findByRole('button', { name: /^Minute by minute/ }));
		await fireEvent.click(screen.getByRole('button', { name: 'Ignore Foo Rifle' }));
		await waitFor(() => expect(statusText()).toBe('Foo Rifle ignored.'));

		await rerender({ sessionId: 's2' });
		const toggle = await screen.findByRole('button', { name: 'Minute by minute' });
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(statusText()).toBe('');
	});
});
