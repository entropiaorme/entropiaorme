// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountSessionMeasure } from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { carried, exact, gain, measure, SPAN, totals, wholePec } from './__fixtures__/measure';
import { NOTHING_TRACKED } from './measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
	getMeasuredSummary: vi.fn(),
}));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/account', () => api);
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import SessionMeasure from './SessionMeasure.svelte';

/** The comparison row for a category. */
function row(label: string): HTMLElement {
	const found = screen.getByText(label, { selector: 'td span' }).closest('tr');
	if (!found) throw new Error(`no ${label} row`);
	return found;
}

const tracked = { weapon: 10, healing: 1, armour: 0.5, loot: 9 };

/** A complete measure whose totals match its lines, as the backend
 * sends it. */
function measured(overrides: Partial<AccountSessionMeasure> = {}): AccountSessionMeasure {
	return measure({
		ammo: [
			{
				name: 'Universal Ammo',
				netUnits: -95000,
				net: exact(-9.5),
				split: {
					kind: 'settled',
					burnedUnits: 95000,
					burned: exact(9.5),
					strongboxUnits: 0,
					strongbox: exact(0),
					convertedUnits: 0,
					converted: exact(0),
				},
			},
		],
		decay: [
			{ item: carried('Rifle'), role: 'weapon', decay: exact(0.75) },
			{ item: carried('Harness'), role: 'armour', decay: wholePec(0.4) },
		],
		gained: [
			gain('Animal Oil Residue', exact(7)),
			gain('Fireworks', exact(2), 'possiblyStrongbox'),
		],
		gainedTotal: exact(9),
		gainedPossiblyStrongbox: exact(2),
		events: [
			{
				kind: 'sale',
				between: SPAN,
				items: [{ item: carried('Shrapnel'), units: 1240, amount: wholePec(12.4) }],
				amount: wholePec(12.4),
			},
		],
		totals: totals({
			ammo: { kind: 'settled', burned: exact(9.5) },
			weaponDecay: exact(0.75),
			armourDecay: wholePec(0.4),
			gained: exact(9),
			gainedPossiblyStrongbox: exact(2),
		}),
		...overrides,
	});
}

/** A promise the test settles by hand. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
});

describe("a session's measure in its detail", () => {
	it('renders nothing at all when the session has no measure', async () => {
		api.getSessionMeasure.mockResolvedValue(null);
		const { container } = render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s1'));
		// Wait for the absent answer itself before asserting nothing shows.
		await api.getSessionMeasure.mock.results[0].value;
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});

	it('renders nothing when the first read fails', async () => {
		api.getSessionMeasure.mockRejectedValue(new Error('the account connection was not composed'));
		const { container } = render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalled());
		await api.getSessionMeasure.mock.results[0].value.catch(() => undefined);
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});

	it('stands each tracked figure beside its measured one, read from the totals, with no total', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		expect(await screen.findByText('Measured from the account')).toBeTruthy();
		expect(screen.getByText('Complete: the readings cover the whole session.')).toBeTruthy();
		const weapon = row('Weapon');
		// Exact measured: tracked to four decimals too, so the row adds up.
		expect(within(weapon).getByText('10.0000')).toBeTruthy();
		expect(within(weapon).getByText('10.2500')).toBeTruthy();
		expect(within(weapon).getByText('+0.2500')).toBeTruthy();
		const armour = row('Armour');
		// Whole-PEC measured: tracked stays at two.
		expect(within(armour).getByText('0.50')).toBeTruthy();
		expect(within(armour).getByText('0.40')).toBeTruthy();
		expect(screen.queryByText(/^Total$/)).toBeNull();
		expect(screen.queryByText(/Net/)).toBeNull();
	});

	it('takes the comparison from the measure totals, not from its lines', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				totals: totals({ ammo: { kind: 'settled', burned: exact(3) }, gained: exact(9) }),
			}),
		);
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await screen.findByText('Measured from the account');
		expect(within(row('Weapon')).getByText('3.0000')).toBeTruthy();
	});

	it('sets each aside beneath the measured figure it qualifies', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await screen.findByText('Measured from the account');
		const loot = within(row('Loot')).getAllByRole('cell');
		expect(loot[2].textContent).toContain('9.0000');
		expect(loot[2].textContent).toContain("of which 2.0000 PED may be a strongbox's contents");
		expect(loot[0].textContent?.trim()).toBe('Loot');
		const armour = within(row('Armour')).getAllByRole('cell');
		expect(armour[2].textContent).toMatch(/repairs cost/);
	});

	it('marks a partial measure as partial, and differences none of it against the whole', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				coverage: {
					level: 'partial',
					opening: 'read',
					closing: 'read',
					readingCount: 50,
					gapCount: 3,
				},
			}),
		);
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		expect(
			await screen.findByText('Partial: the readings missed part of this session.'),
		).toBeTruthy();
		expect(screen.getByText('Measured, part of the session')).toBeTruthy();
		for (const label of ['Weapon', 'Healing', 'Armour', 'Other', 'Loot']) {
			const cells = within(row(label)).getAllByRole('cell');
			expect(cells[3].textContent?.trim()).toBe(NO_DATA);
		}
	});

	it('treats a measure still being read like a partial one, and says an ended session awaits its closing readings', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				sealed: false,
				coverage: {
					level: 'partial',
					opening: 'read',
					closing: 'unread',
					readingCount: 9,
					gapCount: 0,
				},
			}),
		);
		render(SessionMeasure, { props: { sessionId: 's1', tracked, ended: true } });
		expect(
			await screen.findByText('The session has ended; its closing readings are still being taken.'),
		).toBeTruthy();
		expect(within(row('Weapon')).getAllByRole('cell')[3].textContent?.trim()).toBe(NO_DATA);
	});

	it('keeps the block when a later read fails, and says why', async () => {
		api.getSessionMeasure
			.mockResolvedValueOnce(measured())
			.mockRejectedValueOnce(new Error('the readings could not be read'));
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await screen.findByText('Measured from the account');
		await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
		for (const handler of frames.handlers) handler();
		expect((await screen.findByRole('alert')).textContent).toMatch(/could not be read/);
		expect(screen.getByText('Measured from the account')).toBeTruthy();
		expect(within(row('Weapon')).getByText('10.2500')).toBeTruthy();
	});

	it('shows a dash, never zero, where nothing was tracked', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked: NOTHING_TRACKED } });
		const loot = (await screen.findByText('Loot')).closest('tr');
		if (!loot) throw new Error('no loot row');
		const cells = within(loot).getAllByRole('cell');
		expect(cells[1].textContent?.trim()).toBe(NO_DATA);
		expect(cells[3].textContent?.trim()).toBe(NO_DATA);
	});

	it('keeps the details collapsed until asked', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		const toggle = await screen.findByRole('button', { name: 'Details' });
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(toggle.getAttribute('aria-controls')).toBeNull();
		expect(screen.queryByText(/Sold to the Trade Terminal/)).toBeNull();
		await fireEvent.click(toggle);
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		const region = toggle.getAttribute('aria-controls');
		expect(region && document.getElementById(region)).toBeTruthy();
		expect(screen.getByText('Sold to the Trade Terminal: 12.40 PED; 1240 Shrapnel')).toBeTruthy();
		expect(screen.getByText('Rifle')).toBeTruthy();
	});

	it('re-reads when the account topic fires', async () => {
		api.getSessionMeasure.mockResolvedValueOnce(null).mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledTimes(1));
		await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
		for (const handler of frames.handlers) handler();
		expect(await screen.findByText('Measured from the account')).toBeTruthy();
	});

	it('never shows a measure read for one session under another', async () => {
		const first = deferred<AccountSessionMeasure | null>();
		const second = deferred<AccountSessionMeasure | null>();
		api.getSessionMeasure.mockImplementation((id: string) =>
			id === 's1' ? first.promise : second.promise,
		);
		const { rerender, container } = render(SessionMeasure, {
			props: { sessionId: 's1', tracked },
		});
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s1'));
		await rerender({ sessionId: 's2', tracked });
		// The first session's measure arrives after the player moved on.
		first.resolve(measured({ sessionId: 's1' }));
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s2'));
		expect(container.textContent?.trim()).toBe('');
		second.resolve(null);
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});
});

describe('the measure opened from the review', () => {
	it('shows its loading state, then the measure', async () => {
		const read = deferred<AccountSessionMeasure | null>();
		api.getSessionMeasure.mockReturnValue(read.promise);
		const { container } = render(SessionMeasure, {
			props: { sessionId: 's1', tracked, framed: false },
		});
		expect(container.querySelector('[aria-busy="true"]')).toBeTruthy();
		read.resolve(measured());
		expect(await screen.findByTestId('measure-block')).toBeTruthy();
		expect(screen.queryByText('Measured from the account')).toBeNull();
	});

	it('says why when the measure cannot be read', async () => {
		api.getSessionMeasure.mockRejectedValue(new Error('Session not found'));
		render(SessionMeasure, { props: { sessionId: 's1', tracked, framed: false } });
		expect((await screen.findByRole('alert')).textContent).toMatch(/Session not found/);
	});
});
