// @vitest-environment happy-dom

import { render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountStatus } from '$lib/api/commands.gen';
import { status } from '$lib/features/account/__fixtures__/measure';

// The in-development channel, switchable per test: a published build hides
// surfaces still in development.
const channel = vi.hoisted(() => ({ visible: true }));
vi.mock('$lib/inDevelopment/channel', () => ({
	isInDevelopmentVisible: (stable: boolean) => !stable,
	inDevelopment: {
		get visible() {
			return channel.visible;
		},
	},
}));

const account = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
	getMeasuredSummary: vi.fn(),
}));
vi.mock('$lib/api/account', () => account);
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));

// The tabs themselves are not under test here.
vi.mock('./OverviewTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./LedgerTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./MarketTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./HuntingTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./TreeCuttingTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./QuestsTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));
vi.mock('./MeasuredTab.svelte', async () => ({
	default: (await import('$lib/features/account/__fixtures__/Blank.svelte')).default,
}));

import AnalyticsPage from './+page.svelte';

function tabNames(): string[] {
	return screen.getAllByRole('tab').map((tab) => tab.textContent?.trim() ?? '');
}

async function answered() {
	await waitFor(() => expect(account.getAccountStatus).toHaveBeenCalled());
	await account.getAccountStatus.mock.results[0].value;
	await new Promise((settle) => setTimeout(settle, 0));
}

function answer(next: AccountStatus) {
	account.getAccountStatus.mockResolvedValue(next);
}

beforeEach(() => {
	vi.clearAllMocks();
	channel.visible = true;
	answer(status());
});

describe('the Measured tab', () => {
	it('is absent, and the account never asked, in a published build', async () => {
		channel.visible = false;
		answer(status({ state: 'connected', hasConnected: true, hasReadings: true }));
		render(AnalyticsPage);
		await new Promise((settle) => setTimeout(settle, 0));
		expect(tabNames()).not.toContain('Measured');
		expect(account.getAccountStatus).not.toHaveBeenCalled();
	});

	it('is absent before the player has connected or anything was read', async () => {
		render(AnalyticsPage);
		await answered();
		expect(tabNames()).toEqual([
			'Overview',
			'Ledger',
			'Market',
			'Hunting',
			'Tree Cutting',
			'Quests',
		]);
	});

	it('stays after a disconnect while readings remain', async () => {
		answer(status({ hasReadings: true }));
		render(AnalyticsPage);
		await waitFor(() => expect(tabNames()).toContain('Measured'));
	});

	it('is present while the player is connected, or has connected before', async () => {
		answer(status({ state: 'connected', hasConnected: true }));
		const first = render(AnalyticsPage);
		await waitFor(() => expect(tabNames()).toContain('Measured'));
		first.unmount();
		answer(status({ hasConnected: true }));
		render(AnalyticsPage);
		await waitFor(() => expect(tabNames()).toContain('Measured'));
	});
});
