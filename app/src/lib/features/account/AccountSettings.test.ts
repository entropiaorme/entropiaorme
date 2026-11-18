// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountStatus, CostPlacements } from '$lib/api/commands.gen';
import { status } from './__fixtures__/measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
	getMeasuredSummary: vi.fn(),
	getCostPlacements: vi.fn(async (): Promise<CostPlacements> => ({ awaiting: [], placed: [] })),
	placeCostItem: vi.fn(),
	clearCostPlacement: vi.fn(),
}));
const shell = vi.hoisted(() => ({ signInToAccount: vi.fn() }));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/account', () => api);
vi.mock('$lib/api/shell', () => shell);
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import { ApiError } from '$lib/api/client';
import AccountSettings from './AccountSettings.svelte';

let current: AccountStatus;

/** The account topic fires, as the backend's would on a change. */
async function frame() {
	await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
	for (const handler of frames.handlers) handler();
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
	current = status();
	api.getAccountStatus.mockImplementation(async () => current);
	api.disconnectAccount.mockResolvedValue(undefined);
	shell.signInToAccount.mockResolvedValue(undefined);
});

describe('the account connection in Settings', () => {
	it('offers to connect, saying where signing in happens', async () => {
		render(AccountSettings);
		expect(await screen.findByRole('button', { name: 'Connect' })).toBeTruthy();
		expect(screen.getByText('Entropia Universe account')).toBeTruthy();
		expect(screen.getByText(/the app never sees your password/)).toBeTruthy();
		expect(screen.getByText('In development')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Disconnect' })).toBeNull();
		expect(screen.queryByText(/once a minute/)).toBeNull();
	});

	it('follows the sign-in window as the status reports it, with no press', async () => {
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(1);
		// The window opened.
		current = status({ signingIn: true });
		await frame();
		expect(await screen.findByText(/Finish signing in/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Show sign-in window' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(2);
		// The player closed it without signing in.
		current = status({ signingIn: false });
		await frame();
		expect(await screen.findByRole('button', { name: 'Connect' })).toBeTruthy();
		expect(screen.queryByText(/Finish signing in/)).toBeNull();
	});

	it('does not claim a window is open just because Connect was pressed', async () => {
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		await waitFor(() => expect(shell.signInToAccount).toHaveBeenCalled());
		expect(screen.queryByText(/Finish signing in/)).toBeNull();
	});

	it('shows the connection the moment the account topic says it changed', async () => {
		current = status({ signingIn: true });
		render(AccountSettings);
		await screen.findByText(/Finish signing in/);
		current = status({
			state: 'connected',
			hasConnected: true,
			readingSession: true,
			lastReadingAt: new Date().toISOString(),
		});
		await frame();
		expect(await screen.findByText('Connected')).toBeTruthy();
		expect(screen.getByText(/Reading this session/)).toBeTruthy();
		expect(screen.getByText(/once a minute/)).toBeTruthy();
		expect(screen.queryByText(/Finish signing in/)).toBeNull();
	});

	it('lists the items to place once the player has connected, and not before', async () => {
		render(AccountSettings);
		await screen.findByRole('button', { name: 'Connect' });
		expect(screen.queryByTestId('cost-placements')).toBeNull();
		current = status({ state: 'connected', hasConnected: true });
		await frame();
		expect(await screen.findByText('Nothing to place')).toBeTruthy();
		expect(api.getCostPlacements).toHaveBeenCalled();
	});

	it('keeps the items to place reachable after disconnecting while a placement exists', async () => {
		api.getCostPlacements.mockResolvedValueOnce({
			awaiting: [],
			placed: [{ item: 'Mind Essence', category: 'travel', placedAt: '2026-11-16T20:00:00Z' }],
		});
		render(AccountSettings);
		expect(await screen.findByTestId('placed-items')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Connect' })).toBeTruthy();
	});

	it('disconnects without a confirmation', async () => {
		current = status({ state: 'connected', hasConnected: true });
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Disconnect' }));
		expect(api.disconnectAccount).toHaveBeenCalledTimes(1);
		expect(screen.queryByRole('dialog')).toBeNull();
	});

	it('says why a disconnect failed, and clears it once the status changes', async () => {
		current = status({ state: 'connected', hasConnected: true });
		api.disconnectAccount.mockRejectedValue(
			new ApiError('internal', 'the connection did not answer'),
		);
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Disconnect' }));
		expect((await screen.findByRole('alert')).textContent).toMatch(/the connection did not answer/);
		current = status({ state: 'expired', hasConnected: true });
		await frame();
		await screen.findByText('Signed out by the site');
		expect(screen.queryByRole('alert')).toBeNull();
	});

	it('asks to sign in again, beside Disconnect, when the site signed the player out', async () => {
		current = status({ state: 'expired', hasConnected: true });
		render(AccountSettings);
		expect(await screen.findByText('Signed out by the site')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Sign in again' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Disconnect' })).toBeTruthy();
	});

	it('stays true when the site signs the player out while a sign-in window is open', async () => {
		current = status({ state: 'connected', hasConnected: true });
		render(AccountSettings);
		await screen.findByText('Connected');
		current = status({ state: 'expired', hasConnected: true, signingIn: true });
		await frame();
		expect(await screen.findByText('Signed out by the site')).toBeTruthy();
		expect(screen.getByText(/Finish signing in/)).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Show sign-in window' })).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Sign in again' })).toBeNull();
	});

	it('asks a player who connected before simply to sign in', async () => {
		current = status({ hasConnected: true });
		render(AccountSettings);
		expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy();
		expect(screen.getByText(/once a minute/)).toBeTruthy();
	});

	it('offers only Disconnect while a kept sign-in is checked, and says when the site is unreachable', async () => {
		current = status({ state: 'unconfirmed', hasConnected: true, confirming: 'checking' });
		render(AccountSettings);
		expect(await screen.findByText('Checking your sign-in')).toBeTruthy();
		expect(screen.queryByText(/could not be reached/)).toBeNull();
		expect(screen.getByRole('button', { name: 'Disconnect' })).toBeTruthy();
		for (const name of ['Connect', 'Sign in', 'Sign in again', 'Show sign-in window']) {
			expect(screen.queryByRole('button', { name })).toBeNull();
		}
		current = status({ state: 'unconfirmed', hasConnected: true, confirming: 'siteUnreachable' });
		await frame();
		expect(
			await screen.findByText('The account site could not be reached; trying again'),
		).toBeTruthy();
	});

	it("shows the sign-in command's own reason when it refuses", async () => {
		shell.signInToAccount.mockRejectedValue(
			new ApiError('conflict', 'the account connection is not available'),
		);
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		expect((await screen.findByRole('alert')).textContent).toMatch(
			/the account connection is not available/,
		);
	});

	it('says so when the connection cannot be read', async () => {
		api.getAccountStatus.mockRejectedValue(new Error('the account connection was not composed'));
		render(AccountSettings);
		await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/not composed/));
	});
});
