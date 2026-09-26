// @vitest-environment happy-dom

import { render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import dashboardFixture from '../../e2e/fixtures/dashboard.json';

// The dashboard's first paint while the backend is still starting. Only
// Tauri's `invoke` is mocked: the real facade, typed transport, and startup
// readiness gate run, so a read the page makes before the backend is ready
// is genuinely held rather than stubbed. While held, no region may claim an
// answer it does not have (an empty list, an idle session, a start button,
// an error); once ready, each region fills from its first read.
const seams = vi.hoisted(() => {
	let settle: (outcome: unknown) => void = () => {};
	return {
		readiness: new Promise((resolve) => {
			settle = resolve;
		}),
		settleSubstrate: (outcome: unknown) => settle(outcome),
		invoke: vi.fn(),
	};
});

vi.mock('@tauri-apps/api/core', () => ({
	invoke: (command: string, args?: unknown) => seams.invoke(command, args),
}));
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {}),
}));

const ACTIVITY_OPTIONS = {
	definitionId: null,
	definitionName: null,
	visible: false,
	adHocSegments: false,
	readyCount: 0,
	options: [],
	active: [],
};

/** The backend's answers once it is up: the committed dashboard fixture for
 * the tracking snapshot, empty collections elsewhere. */
function answer(command: string): unknown {
	switch (command) {
		case 'substrate_ready':
			return seams.readiness;
		case 'tracking_snapshot':
			return Promise.resolve(dashboardFixture.snapshot);
		case 'tracking_activity_options':
			return Promise.resolve(ACTIVITY_OPTIONS);
		default:
			return Promise.resolve([]);
	}
}

const facadeCalls = (command?: string) =>
	seams.invoke.mock.calls.filter(
		([name]) => name !== 'substrate_ready' && (command === undefined || name === command),
	);

/** Let every queued microtask and zero-delay timer run. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
	seams.invoke.mockReset();
	seams.invoke.mockImplementation(answer);
});

describe('dashboard while the backend starts', () => {
	it('holds every read and claims nothing until the backend is ready', async () => {
		const { default: Dashboard } = await import('./+page.svelte');
		render(Dashboard);
		await flush();
		await flush();

		// Reads were attempted but none reached the backend.
		expect(seams.invoke).toHaveBeenCalledWith('substrate_ready', undefined);
		expect(facadeCalls()).toEqual([]);

		// Each data region stands in with a placeholder, never an answer.
		expect(screen.getByTestId('session-strip-pending')).toBeTruthy();
		expect(screen.getByTestId('recent-events-pending')).toBeTruthy();
		expect(screen.getByTestId('dashboard-widget-pending')).toBeTruthy();
		for (const claim of [
			'No recent events.',
			'Start tracking',
			'No active session',
			'Tracking active',
			'Choose a session type, then add its quests.',
		]) {
			expect(screen.queryByText(claim)).toBeNull();
		}
		expect(screen.queryByRole('alert')).toBeNull();

		// The backend comes up: the held reads dispatch and the page fills in.
		seams.settleSubstrate({ state: 'ready' });
		await waitFor(() => expect(screen.getByText('Tracking active')).toBeTruthy());
		expect(screen.getByText('Looted Animal Oil Residue')).toBeTruthy();
		expect(screen.queryByTestId('session-strip-pending')).toBeNull();
		expect(screen.queryByTestId('recent-events-pending')).toBeNull();
		expect(screen.queryByTestId('dashboard-widget-pending')).toBeNull();
		expect(screen.queryByRole('alert')).toBeNull();

		// One initial snapshot read, sent once, after readiness.
		expect(facadeCalls('tracking_snapshot')).toHaveLength(1);
	});
});
