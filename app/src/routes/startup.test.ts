// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

// Every main page mounted while the backend is still starting. The real
// facade, typed transport, and readiness gate run with only Tauri's `invoke`
// mocked, and the readiness answer never arrives, so every read a page makes
// on mount is held. A page must mount into that state without an error
// surface: a startup read that fails instead of waiting is the defect this
// suite exists to keep out. Where a page has an empty state that would read
// as an answer, it is listed below and must not show while reads are held
// (the dashboard's regions are covered in `page.test.ts`).
const seams = vi.hoisted(() => ({
	invoke: vi.fn((command: string, _args?: unknown) =>
		command === 'substrate_ready' ? new Promise(() => {}) : Promise.resolve([]),
	),
}));

vi.mock('@tauri-apps/api/core', () => ({
	invoke: (command: string, args?: unknown) => seams.invoke(command, args),
	convertFileSrc: (path: string) => path,
}));
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {}),
}));

const PAGES = {
	analytics: () => import('./analytics/+page.svelte'),
	inventory: () => import('./inventory/+page.svelte'),
	character: () => import('./character/+page.svelte'),
	quests: () => import('./quests/+page.svelte'),
	equipment: () => import('./equipment/+page.svelte'),
	market: () => import('./market/+page.svelte'),
	maps: () => import('./maps/+page.svelte'),
	settings: () => import('./settings/+page.svelte'),
};

/** Empty-state claims a page must not make before its first read answers. */
const PREMATURE_CLAIMS: Partial<Record<keyof typeof PAGES, string[]>> = {
	equipment: [
		'Add your first weapon to enable automatic cost tracking.',
		'No consumables configured.',
	],
};

/** Let every queued microtask and zero-delay timer run. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('main pages while the backend starts', () => {
	it.each(
		Object.keys(PAGES) as (keyof typeof PAGES)[],
	)('%s mounts with its reads held and no error', async (name) => {
		const { default: Page } = await PAGES[name]();
		render(Page);
		await flush();
		await flush();

		const facadeCalls = seams.invoke.mock.calls.filter(
			([command]) => command !== 'substrate_ready',
		);
		expect(facadeCalls).toEqual([]);
		expect(screen.queryByRole('alert')).toBeNull();
		for (const claim of PREMATURE_CLAIMS[name] ?? []) {
			expect(screen.queryByText(claim)).toBeNull();
		}
	});
});
