// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountSessionMeasure } from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { carried, exact, measure, SPAN, wholePec } from './__fixtures__/measure';
import { NOTHING_TRACKED } from './measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
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

const tracked = { weapon: 10, healing: 1, armour: 0.5, loot: 9 };

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
		gainedTotal: exact(9),
		events: [{ kind: 'sale', between: SPAN, items: [], amount: wholePec(12.4) }],
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
		await Promise.resolve();
		expect(container.textContent?.trim()).toBe('');
	});

	it('renders nothing when the first read fails', async () => {
		api.getSessionMeasure.mockRejectedValue(new Error('the account connection was not composed'));
		const { container } = render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalled());
		await Promise.resolve();
		expect(container.textContent?.trim()).toBe('');
	});

	it('stands each tracked figure beside its measured one, with no total', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', tracked } });
		expect(await screen.findByText('Measured from the account')).toBeTruthy();
		expect(screen.getByText('Complete: the readings cover the whole session.')).toBeTruthy();
		const weapon = screen.getByText('Weapon').closest('tr');
		expect(weapon).toBeTruthy();
		if (!weapon) return;
		expect(within(weapon).getByText('10.00')).toBeTruthy();
		expect(within(weapon).getByText('10.2500')).toBeTruthy();
		expect(within(weapon).getByText('+0.2500')).toBeTruthy();
		const armour = screen.getByText('Armour').closest('tr');
		if (!armour) throw new Error('no armour row');
		expect(within(armour).getByText('0.40')).toBeTruthy();
		expect(within(armour).getByText(/repairs cost/)).toBeTruthy();
		expect(screen.queryByText(/^Total$/)).toBeNull();
		expect(screen.queryByText(/Net/)).toBeNull();
	});

	it('marks a partial measure as partial', async () => {
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
		expect(screen.queryByText('Sold to the Trade Terminal: 12.40 PED')).toBeNull();
		await fireEvent.click(toggle);
		expect(screen.getByText('Sold to the Trade Terminal: 12.40 PED')).toBeTruthy();
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
