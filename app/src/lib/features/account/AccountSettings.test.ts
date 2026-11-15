// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountStatus } from '$lib/api/commands.gen';
import { status } from './__fixtures__/measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
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

import AccountSettings from './AccountSettings.svelte';

let current: AccountStatus;

function frame() {
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

	it('opens the sign-in window, then says to finish there and brings the same window forward', async () => {
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(1);
		expect(await screen.findByText(/Finish signing in/)).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Show sign-in window' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(2);
	});

	it('shows the connection the moment the account topic says it changed', async () => {
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		current = status({
			state: 'connected',
			hasConnected: true,
			readingSession: true,
			lastReadingAt: '2026-11-15T14:32:00Z',
		});
		frame();
		expect(await screen.findByText('Connected')).toBeTruthy();
		expect(screen.getByText(/Reading this session/)).toBeTruthy();
		expect(screen.getByText(/once a minute/)).toBeTruthy();
		expect(screen.queryByText(/Finish signing in/)).toBeNull();
		expect(screen.queryByRole('button', { name: 'Connect' })).toBeNull();
	});

	it('disconnects without a confirmation', async () => {
		current = status({ state: 'connected', hasConnected: true });
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Disconnect' }));
		expect(api.disconnectAccount).toHaveBeenCalledTimes(1);
		expect(screen.queryByRole('dialog')).toBeNull();
	});

	it('asks to sign in again, beside Disconnect, when the site signed the player out', async () => {
		current = status({ state: 'expired', hasConnected: true });
		render(AccountSettings);
		expect(await screen.findByText('Signed out by the site')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Sign in again' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Disconnect' })).toBeTruthy();
	});

	it('asks a player who connected before simply to sign in', async () => {
		current = status({ hasConnected: true });
		render(AccountSettings);
		expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy();
		expect(screen.getByText(/once a minute/)).toBeTruthy();
	});

	it('offers only Disconnect while a kept sign-in is checked', async () => {
		current = status({ state: 'unconfirmed', hasConnected: true });
		render(AccountSettings);
		expect(await screen.findByText('Checking your sign-in')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Disconnect' })).toBeTruthy();
		for (const name of ['Connect', 'Sign in', 'Sign in again', 'Show sign-in window']) {
			expect(screen.queryByRole('button', { name })).toBeNull();
		}
	});

	it('says why when the sign-in window could not be opened', async () => {
		shell.signInToAccount.mockRejectedValue({ kind: 'conflict', message: 'not available' });
		render(AccountSettings);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect' }));
		expect(await screen.findByText('Could not open the sign-in window.')).toBeTruthy();
	});

	it('says so when the connection cannot be read', async () => {
		api.getAccountStatus.mockRejectedValue(new Error('the account connection was not composed'));
		render(AccountSettings);
		await waitFor(() => expect(screen.getByRole('alert').textContent).toMatch(/not composed/));
	});
});
