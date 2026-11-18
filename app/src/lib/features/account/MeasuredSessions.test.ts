// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountMeasuredSummary, MeasuredSessionPage } from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { exact, gain, measure, row, summary, totals, tracked } from './__fixtures__/measure';

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

import MeasuredSessions from './MeasuredSessions.svelte';
import { createMeasuredSessionsModel } from './measuredSessionsModel.svelte';

function page(overrides: Partial<MeasuredSessionPage> = {}): MeasuredSessionPage {
	return { sessions: [], nextCursor: null, total: 0, ...overrides };
}

/** A promise the test settles by hand. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

const SESSIONS = [
	row({
		id: 'a',
		name: 'Atrox hunt',
		tracked: tracked({ weaponCost: 10, lootTt: 9, armourCost: 0.4, armourUnrecorded: true }),
		measured: totals({
			ammo: { kind: 'settled', burned: exact(10) },
			weaponDecay: exact(0.5),
			armourDecay: exact(0.3),
			gained: exact(9),
		}),
	}),
	row({
		id: 'b',
		name: 'Argonaut run',
		startTime: '2026-11-15T10:00:00Z',
		coverage: 'partial',
		tracked: tracked({ weaponCost: 5 }),
	}),
	row({
		id: 'c',
		name: 'Test run',
		startTime: '2026-11-15T09:00:00Z',
		exclusion: {
			economyExcludedAt: '2026-11-15T15:00:00Z',
			progressionExcludedAt: null,
			note: null,
		},
		tracked: tracked({ weaponCost: 50, lootTt: 40 }),
	}),
];

const SUMMARY: AccountMeasuredSummary = summary({
	weapon: { sessions: 1, tracked: 10, measured: exact(10.5) },
	healing: { sessions: 1, tracked: 0, measured: exact(0) },
	armour: { sessions: 0, tracked: 0, measured: exact(0) },
	loot: { sessions: 1, tracked: 9, measured: exact(9) },
	other: { sessions: 1, measured: exact(0.25) },
	included: 1,
	leftOut: [
		{ reason: 'partialCoverage', sessions: 1 },
		{ reason: 'excluded', sessions: 1 },
		{ reason: 'notSealed', sessions: 0 },
	],
});

async function frame() {
	await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
	for (const handler of frames.handlers) handler();
}

function mount() {
	const model = createMeasuredSessionsModel();
	render(MeasuredSessions, { props: { model } });
	model.connect();
	return model;
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
	api.getMeasuredSessions.mockResolvedValue(page({ sessions: SESSIONS, total: 3 }));
	api.getMeasuredSummary.mockResolvedValue(SUMMARY);
	api.getSessionMeasure.mockResolvedValue(
		measure({
			sessionId: 'a',
			gained: [gain('Oil', exact(9))],
			gainedTotal: exact(9),
			totals: totals({ armourDecay: exact(0.3), gained: exact(9) }),
		}),
	);
});

describe('the review of measured sessions', () => {
	it('says when readings are taken while no session has been read', async () => {
		api.getMeasuredSessions.mockResolvedValue(page());
		api.getMeasuredSummary.mockResolvedValue(summary());
		mount();
		expect(
			await screen.findByText(
				'Readings are taken while a session is tracked with the account connected.',
			),
		).toBeTruthy();
	});

	it('lists complete readings only by default, and marks partial and excluded sessions', async () => {
		const model = mount();
		await screen.findByText('Atrox hunt');
		expect(screen.queryByText('Argonaut run')).toBeNull();
		expect(screen.getByText('Not in PED stats')).toBeTruthy();
		model.completeOnly = false;
		const partial = (await screen.findByText('Argonaut run')).closest('tr');
		if (!partial) throw new Error('no partial row');
		expect(within(partial).getByText('Partial')).toBeTruthy();
	});

	it('heads the page with the whole-history pairs, each shown as tracked and measured beside the difference', async () => {
		mount();
		const sums = await screen.findByTestId('measured-summary');
		expect(within(sums).getByText('Measured minus tracked estimate')).toBeTruthy();
		expect(within(sums).getByTestId('sum-tracked-weapon').textContent?.trim()).toBe('10.0000');
		expect(within(sums).getByTestId('sum-measured-weapon').textContent?.trim()).toBe('10.5000');
		expect(within(sums).getByTestId('sum-difference-weapon').textContent?.trim()).toBe('+0.5000');
		expect(within(sums).getByTestId('sum-share-weapon').textContent?.trim()).toBe('+5.0%');
		// A share of nothing tracked is no data, and so is a pair no session went into.
		expect(within(sums).getByTestId('sum-share-healing').textContent?.trim()).toBe(NO_DATA);
		expect(within(sums).getByTestId('sum-measured-armour').textContent?.trim()).toBe(NO_DATA);
		// Other stands apart, measured only.
		expect(within(sums).getByTestId('sum-other').textContent).toMatch(
			/Other, measured only:\s*0\.2500\s*PED over 1\s*session/,
		);
		expect(within(sums).getByTestId('sum-left-out').textContent).toBe(
			'Not in these figures: 2 sessions with readings (1 with partial readings and 1 left out of the PED stats).',
		);
		expect(api.getMeasuredSummary).toHaveBeenCalled();
	});

	it('never sums over no session beside a sentence saying there is nothing to sum', async () => {
		api.getMeasuredSummary.mockResolvedValue(summary());
		mount();
		const sums = await screen.findByTestId('measured-summary');
		await waitFor(() => expect(sums.textContent).toMatch(/nothing to sum/));
		expect(sums.textContent).not.toMatch(/over 0/);
		expect(within(sums).queryByText('Measured minus tracked estimate')).toBeNull();
	});

	it('shows no tracked armour for a session whose armour is not recorded, in its row and opened', async () => {
		mount();
		const atrox = (await screen.findByText('Atrox hunt')).closest('tr');
		if (!atrox) throw new Error('no row');
		const armour = within(atrox).getByTestId('cell-armour');
		expect(armour.querySelector('p')?.textContent?.trim()).toBe(NO_DATA);
		await fireEvent.click(within(atrox).getByRole('button', { name: 'Atrox hunt' }));
		const block = await screen.findByTestId('measure-block');
		const opened = within(block).getByText('Armour', { selector: 'td span' }).closest('tr');
		if (!opened) throw new Error('no armour row');
		expect(within(opened).getAllByRole('cell')[1].textContent?.trim()).toBe(NO_DATA);
	});

	it('opens the same measure through a real button, announcing what it controls only once it exists', async () => {
		mount();
		const button = await screen.findByRole('button', { name: 'Atrox hunt' });
		expect(button.getAttribute('aria-expanded')).toBe('false');
		expect(button.getAttribute('aria-controls')).toBeNull();
		await fireEvent.click(button);
		expect(button.getAttribute('aria-expanded')).toBe('true');
		const region = button.getAttribute('aria-controls');
		expect(region && document.getElementById(region)).toBeTruthy();
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('a'));
		expect(await screen.findByTestId('measure-block')).toBeTruthy();
	});

	it('loads older sessions a backend page at a time', async () => {
		api.getMeasuredSessions
			.mockResolvedValueOnce(page({ sessions: [SESSIONS[0]], total: 2, nextCursor: 'next' }))
			.mockResolvedValueOnce(
				page({
					sessions: [row({ id: 'd', name: 'Older hunt', startTime: '2026-11-01T10:00:00Z' })],
					total: 2,
					nextCursor: null,
				}),
			);
		mount();
		await fireEvent.click(await screen.findByRole('button', { name: 'Load older sessions' }));
		expect(api.getMeasuredSessions).toHaveBeenLastCalledWith('next', null);
		expect(await screen.findByText('Older hunt')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Load older sessions' })).toBeNull();
	});

	it('re-reads on the account topic: a new session joins, a deleted one goes', async () => {
		mount();
		await screen.findByText('Atrox hunt');
		api.getMeasuredSessions.mockResolvedValue(
			page({ sessions: [row({ id: 'n', name: 'New hunt' }), SESSIONS[2]], total: 2 }),
		);
		await frame();
		expect(await screen.findByText('New hunt')).toBeTruthy();
		expect(screen.queryByText('Atrox hunt')).toBeNull();
		await waitFor(() => expect(api.getMeasuredSummary).toHaveBeenCalledTimes(2));
	});

	it('never lets an older answer overwrite a newer one when events overlap', async () => {
		mount();
		await screen.findByText('Atrox hunt');
		const first = deferred<MeasuredSessionPage>();
		const second = deferred<MeasuredSessionPage>();
		api.getMeasuredSessions.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
		const before = api.getMeasuredSessions.mock.calls.length;
		await frame();
		await frame();
		// One read at a time: the second event waits for the first answer.
		expect(api.getMeasuredSessions.mock.calls.length).toBe(before + 1);
		first.resolve(page({ sessions: [row({ id: 'old', name: 'Older answer' })], total: 1 }));
		await waitFor(() => expect(api.getMeasuredSessions.mock.calls.length).toBe(before + 2));
		second.resolve(page({ sessions: [row({ id: 'new', name: 'Newer answer' })], total: 1 }));
		expect(await screen.findByText('Newer answer')).toBeTruthy();
		await new Promise((settle) => setTimeout(settle, 0));
		expect(screen.queryByText('Older answer')).toBeNull();
	});

	it('does not lose an event that arrives during the first read', async () => {
		const firstRead = deferred<MeasuredSessionPage>();
		api.getMeasuredSessions
			.mockReturnValueOnce(firstRead.promise)
			.mockResolvedValue(page({ sessions: [row({ id: 'n', name: 'After the event' })], total: 1 }));
		mount();
		await frame();
		firstRead.resolve(page({ sessions: SESSIONS, total: 3 }));
		expect(await screen.findByText('After the event')).toBeTruthy();
	});

	it('says why a read failed, and clears that once a later read succeeds', async () => {
		api.getMeasuredSessions
			.mockRejectedValueOnce(new Error('the account connection was not composed'))
			.mockResolvedValue(page({ sessions: SESSIONS, total: 3 }));
		mount();
		expect((await screen.findByRole('alert')).textContent).toMatch(/not composed/);
		await frame();
		expect(await screen.findByText('Atrox hunt')).toBeTruthy();
		expect(screen.queryByRole('alert')).toBeNull();
	});

	it('says how many loaded sessions are not complete, claiming no more', async () => {
		api.getMeasuredSessions.mockResolvedValue(page({ sessions: [SESSIONS[1]], total: 1 }));
		mount();
		expect(
			await screen.findByText('The one session with readings is not complete yet.'),
		).toBeTruthy();
	});
});
