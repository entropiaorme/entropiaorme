// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountStatus } from '$lib/api/commands.gen';
import { status } from './__fixtures__/measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
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

import AccountSignIn from './AccountSignIn.svelte';

let current: AccountStatus;

async function frame() {
	await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
	for (const handler of frames.handlers) handler();
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
	current = status();
	api.getAccountStatus.mockImplementation(async () => current);
	shell.signInToAccount.mockResolvedValue(undefined);
});

describe('the account control on the dashboard', () => {
	it('asks a player who has not connected to connect, and opens the sign-in window', async () => {
		render(AccountSignIn);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect account' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(1);
		expect(screen.queryByRole('status')).toBeNull();
	});

	it('asks a returning player to sign in for this run of the app', async () => {
		current = status({ signal: 'notSignedIn', hasConnected: true });
		render(AccountSignIn);
		expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy();
	});

	it('turns into quiet green text once signed in, with nothing to press', async () => {
		render(AccountSignIn);
		await screen.findByRole('button', { name: 'Connect account' });
		current = status({ state: 'connected', signal: 'signedIn', hasConnected: true });
		await frame();
		const signedIn = await screen.findByRole('status');
		expect(signedIn.textContent).toContain('Signed in');
		expect(signedIn.dataset.mark).toBe('live');
		expect(screen.queryByRole('button')).toBeNull();
	});

	it('pulses amber when the site signs the player out, and offers to sign in again', async () => {
		current = status({ state: 'connected', signal: 'signedIn', hasConnected: true });
		render(AccountSignIn);
		await screen.findByRole('status');
		current = status({ state: 'expired', signal: 'signedOut', hasConnected: true });
		await frame();
		await waitFor(() => expect(screen.getByRole('status').textContent).toContain('Signed out'));
		const signedOut = screen.getByRole('status');
		expect(signedOut.dataset.mark).toBe('alert');
		expect(signedOut.querySelector('.animate-pulse')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Sign in again' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(1);
	});

	it('brings the sign-in window back while it is open', async () => {
		current = status({ signal: 'notSignedIn', hasConnected: true, signingIn: true });
		render(AccountSignIn);
		await fireEvent.click(await screen.findByRole('button', { name: 'Finish signing in' }));
		expect(shell.signInToAccount).toHaveBeenCalledTimes(1);
	});

	it('says why the sign-in window could not open', async () => {
		shell.signInToAccount.mockRejectedValue(new Error('no window'));
		render(AccountSignIn);
		await fireEvent.click(await screen.findByRole('button', { name: 'Connect account' }));
		expect(await screen.findByRole('alert')).toBeTruthy();
	});
});
