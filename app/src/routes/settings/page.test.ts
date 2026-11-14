// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppSettings } from '$lib/types';

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

const seams = vi.hoisted(() => ({ settings: null as AppSettings | null }));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	getSettings: vi.fn(async () => seams.settings),
	updateSettings: vi.fn(),
}));
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {}),
}));

import { updateSettings } from '$lib/api';
import SettingsPage from './+page.svelte';

function settings(): AppSettings {
	return {
		gameConnection: { chatLogPath: '/chat.log', chatLogValid: true, playerName: 'A B C' },
		hotbarHooksEnabled: true,
		keyboard: { observer: 'xinput', keyless: false, keylessAvailable: true },
		repairOcrEnabled: false,
		mobDetectionEnabled: false,
		mobDetectionCapture: 'ready',
		toolReadingEnabled: false,
		toolReadingCapture: 'ready',
		keepUnreadPlates: false,
		sessionName: '',
		declaredSkillBoostPercent: null,
		hotbar: {},
		activeHotbarPreset: 1,
		hotbarPresets: [{}, {}, {}],
		carriedWeaponIds: [],
		passiveEffectSources: [],
		lootFilterBlacklist: [],
		dbPath: '/data/db',
		appVersion: '0.0.0',
	} as unknown as AppSettings;
}

const HELD_TOOL = 'Read the held tool from the screen';
const CREATURES = 'Detect the mob I am fighting';

/** The toggles' accessible names, in page order. */
function toggles(): string[] {
	return screen.getAllByRole('switch').map((toggle) => toggle.getAttribute('aria-label') ?? '');
}

beforeEach(() => {
	channel.visible = true;
	seams.settings = settings();
});

describe('the screen-reading settings', () => {
	it('shows reading the held tool in a published build, unmarked, beside the hotbar key listener', async () => {
		channel.visible = false;
		render(SettingsPage);
		expect(await screen.findByText(HELD_TOOL)).toBeTruthy();
		const order = toggles();
		expect(order.indexOf(HELD_TOOL)).toBe(order.indexOf('Enable hotbar key listener') + 1);
		expect(screen.queryByText('In development')).toBeNull();
	});

	it('shows mob detection in a published build too, unmarked, right after the held tool', async () => {
		channel.visible = false;
		render(SettingsPage);
		expect(await screen.findByText(CREATURES)).toBeTruthy();
		expect(screen.queryByText('In development')).toBeNull();
		const order = toggles();
		expect(order.indexOf(CREATURES)).toBe(order.indexOf(HELD_TOOL) + 1);
	});

	it('shows both unmarked where surfaces in development show', async () => {
		render(SettingsPage);
		expect(await screen.findByText(CREATURES)).toBeTruthy();
		expect(screen.getByText(HELD_TOOL)).toBeTruthy();
		expect(screen.queryByText('In development')).toBeNull();
		const order = toggles();
		expect(order.indexOf(HELD_TOOL)).toBe(order.indexOf('Enable hotbar key listener') + 1);
		expect(order.indexOf(CREATURES)).toBe(order.indexOf(HELD_TOOL) + 1);
	});
});

describe('the diagnostics settings', () => {
	it('links to the metrics in every build, with no mode to turn on first', async () => {
		channel.visible = false;
		render(SettingsPage);
		const link = (await screen.findByText('Open')) as HTMLAnchorElement;
		expect(link.getAttribute('href')).toBe('/metrics');
		expect(screen.queryByText('Developer mode')).toBeNull();
	});

	it('keeps unread plates only once the player turns it on', async () => {
		vi.mocked(updateSettings).mockResolvedValue({ ...settings(), keepUnreadPlates: true });
		render(SettingsPage);
		const toggle = await screen.findByRole('switch', { name: 'Keep plates that did not read' });
		expect(toggle.getAttribute('aria-checked')).toBe('false');
		await fireEvent.click(toggle);
		expect(updateSettings).toHaveBeenCalledWith({ keep_unread_plates: true });
		await waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'));
	});
});
