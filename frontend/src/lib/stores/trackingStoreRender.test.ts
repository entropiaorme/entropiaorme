// @vitest-environment happy-dom

import { render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// The realtime store->render path, end to end minus the wire: the REAL
// trackingStore feeds a minimal fixture component through real Svelte 5
// reactivity; only the snapshot read and the Tauri listener seams are mocked.
// The fixture mounts with the canonical subscribe-then-hydrate discipline.
const getTrackingSnapshot = vi.fn();
const listen = vi.fn();

vi.mock('$lib/api', () => ({
	getTrackingSnapshot: (...args: unknown[]) => getTrackingSnapshot(...args),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: (...args: unknown[]) => listen(...args),
}));

import TrackingConsumer from './__fixtures__/TrackingConsumer.svelte';

beforeEach(() => {
	getTrackingSnapshot.mockReset();
	listen.mockReset();
	listen.mockResolvedValue(vi.fn());
});

describe('realtime store->render path', () => {
	it('attaches the frame listener before the first read, then renders the hydrated snapshot', async () => {
		getTrackingSnapshot.mockResolvedValue({ status: 'active', kill_count: 3 });
		render(TrackingConsumer);

		expect(screen.getByTestId('status').textContent?.trim()).toBe('unhydrated');
		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('active:3');
		});
		// Subscription preceded the read: the listener seam was already attached
		// when the snapshot call went out.
		expect(listen).toHaveBeenCalledTimes(1);
		expect(listen.mock.calls[0][0]).toBe('tracking:session:updated');
	});

	it('re-renders from a fresh snapshot read on each relayed frame', async () => {
		getTrackingSnapshot.mockResolvedValue({ status: 'active', kill_count: 3 });
		render(TrackingConsumer);
		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('active:3');
		});

		// A backend frame lands: the store re-reads (frame payload ignored) and
		// the render follows the new snapshot.
		getTrackingSnapshot.mockResolvedValue({ status: 'active', kill_count: 4 });
		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		onFrame({ payload: { anything: 'ignored' } });

		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('active:4');
		});
	});

	it('treats a payload-less reconnect nudge as re-hydrate, never as idle', async () => {
		getTrackingSnapshot.mockResolvedValue({ status: 'active', kill_count: 4 });
		render(TrackingConsumer);
		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('active:4');
		});

		// The relay's reconnect nudge carries no payload; the consumer must
		// re-read rather than blank into an idle render.
		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		onFrame({});

		await waitFor(() => {
			expect(getTrackingSnapshot.mock.calls.length).toBeGreaterThanOrEqual(2);
		});
		expect(screen.getByTestId('status').textContent?.trim()).toBe('active:4');
	});

	it('renders the idle snapshot when a stop frame re-reads to idle', async () => {
		getTrackingSnapshot.mockResolvedValue({ status: 'active', kill_count: 4 });
		render(TrackingConsumer);
		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('active:4');
		});

		getTrackingSnapshot.mockResolvedValue({ status: 'idle' });
		const onFrame = listen.mock.calls[0][1] as (event: unknown) => void;
		onFrame({ payload: {} });

		await waitFor(() => {
			expect(screen.getByTestId('status').textContent?.trim()).toBe('idle:0');
		});
	});
});
