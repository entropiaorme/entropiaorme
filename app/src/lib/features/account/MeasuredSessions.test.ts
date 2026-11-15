// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MeasuredSessionPage } from '$lib/api/commands.gen';
import { exact, measure, row, totals } from './__fixtures__/measure';

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

import MeasuredSessions from './MeasuredSessions.svelte';
import { createMeasuredSessionsModel } from './measuredSessionsModel.svelte';

function page(overrides: Partial<MeasuredSessionPage> = {}): MeasuredSessionPage {
	return { sessions: [], nextCursor: null, total: 0, ...overrides };
}

const tracked = (weapon: number, loot: number) => ({
	weaponCost: weapon,
	healingCost: 0,
	armourCost: 0,
	lootTt: loot,
	totalCycled: weapon,
});

const SESSIONS = [
	row({
		id: 'a',
		name: 'Atrox hunt',
		tracked: tracked(10, 9),
		measured: totals({
			ammo: { kind: 'settled', burned: exact(10) },
			weaponDecay: exact(0.5),
			gained: exact(9),
		}),
	}),
	row({ id: 'b', name: 'Argonaut run', coverage: 'partial', tracked: tracked(5, 4) }),
	row({
		id: 'c',
		name: 'Test run',
		exclusion: {
			economyExcludedAt: '2026-11-15T15:00:00Z',
			progressionExcludedAt: null,
			note: null,
		},
		tracked: tracked(50, 40),
	}),
];

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
	api.getSessionMeasure.mockResolvedValue(measure({ sessionId: 'a' }));
});

describe('the review of measured sessions', () => {
	it('says when readings are taken while no session has been read', async () => {
		api.getMeasuredSessions.mockResolvedValue(page());
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

	it('sums the pair over complete sessions in the PED stats, with the difference as a share', async () => {
		mount();
		const weapon = await screen.findByTestId('measured-sum-weapon');
		expect(within(weapon).getByText('+0.5000')).toBeTruthy();
		expect(within(weapon).getByText('+5.0% of tracked')).toBeTruthy();
		expect(within(weapon).getByText('10.00 tracked, 10.5000 measured')).toBeTruthy();
		expect(screen.getByText(/summed over 1 complete session in the PED stats/)).toBeTruthy();
	});

	it('opens the same measure the session detail shows when a row is selected', async () => {
		mount();
		const atrox = (await screen.findByText('Atrox hunt')).closest('tr');
		if (!atrox) throw new Error('no row');
		expect(atrox.getAttribute('aria-expanded')).toBe('false');
		await fireEvent.keyDown(atrox, { key: 'Enter' });
		expect(atrox.getAttribute('aria-expanded')).toBe('true');
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('a'));
		expect(await screen.findByTestId('measure-block')).toBeTruthy();
	});

	it('loads older sessions a backend page at a time', async () => {
		api.getMeasuredSessions
			.mockResolvedValueOnce(page({ sessions: [SESSIONS[0]], total: 2, nextCursor: 'next' }))
			.mockResolvedValueOnce(
				page({ sessions: [row({ id: 'd', name: 'Older hunt' })], total: 2, nextCursor: null }),
			);
		mount();
		await fireEvent.click(await screen.findByRole('button', { name: 'Load older sessions' }));
		expect(api.getMeasuredSessions).toHaveBeenLastCalledWith('next', null);
		expect(await screen.findByText('Older hunt')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Load older sessions' })).toBeNull();
	});

	it('re-reads the newest page when the account topic fires', async () => {
		mount();
		await screen.findByText('Atrox hunt');
		api.getMeasuredSessions.mockResolvedValue(
			page({ sessions: [row({ id: 'n', name: 'New hunt' }), ...SESSIONS], total: 4 }),
		);
		for (const handler of frames.handlers) handler();
		expect(await screen.findByText('New hunt')).toBeTruthy();
		expect(screen.getByText('Atrox hunt')).toBeTruthy();
	});

	it('says why when the sessions cannot be read', async () => {
		api.getMeasuredSessions.mockRejectedValue(new Error('the account connection was not composed'));
		mount();
		expect((await screen.findByRole('alert')).textContent).toMatch(/not composed/);
	});
});
