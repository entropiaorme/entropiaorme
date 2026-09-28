// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlanetMap } from '$lib/api';

vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
	emit: vi.fn(async () => {}),
}));

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(async (_key: string, fallback: unknown) => fallback),
	setPreference: vi.fn(async () => {}),
}));

vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	getPlanetMaps: vi.fn(),
	getMapViews: vi.fn(async () => []),
	getMapPins: vi.fn(async () => []),
	planetMapImage: vi.fn(async () => 'data:image/png;base64,'),
	getNavigationSnapshot: vi.fn(async () => null),
	showNavigationOverlays: vi.fn(async () => {}),
	toggleCartographyOverlay: vi.fn(async () => {}),
	startRadarCalibration: vi.fn(async () => 'awaitCentre'),
	cancelRadarCalibration: vi.fn(async () => {}),
	getRadarCalibrationStatus: vi.fn(async () => 'awaitCentre'),
	getRadarGeometry: vi.fn(async () => null),
	cancelMapsCalibration: vi.fn(async () => {}),
}));

import * as api from '$lib/api';
import MapsSurface from './MapsSurface.svelte';

const mocked = vi.mocked(api);

const calypso: PlanetMap = {
	name: 'Calypso',
	technicalName: 'Calypso',
	imageMime: 'image/jpeg',
	imageWidthPx: 4608,
	imageHeightPx: 4608,
	calibration: null,
};

// happy-dom has no Web Animations API; the dialogs' transitions need one
// that settles instantly.
beforeAll(() => {
	Element.prototype.animate = function animate() {
		const animation = {
			cancel() {},
			finish() {},
			effect: null,
			currentTime: 0,
			playState: 'finished',
			onfinish: null as (() => void) | null,
			oncancel: null as (() => void) | null,
		};
		queueMicrotask(() => animation.onfinish?.());
		return animation as unknown as Animation;
	};
});

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getPlanetMaps.mockResolvedValue([calypso]);
});

describe('maps surface', () => {
	it('offers the map actions in one compact bar', async () => {
		render(MapsSurface);

		expect(await screen.findByRole('button', { name: 'Pin overlay' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Route' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Select pins' })).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Setup' })).toBeTruthy();
	});

	it('toggles the pin overlay and opens route planning in the overlay', async () => {
		render(MapsSurface);

		await fireEvent.click(await screen.findByRole('button', { name: 'Pin overlay' }));
		await waitFor(() => expect(mocked.toggleCartographyOverlay).toHaveBeenCalledOnce());

		await fireEvent.click(screen.getByRole('button', { name: 'Route' }));
		await waitFor(() => expect(mocked.showNavigationOverlays).toHaveBeenCalledOnce());
	});

	it('opens its setup dialogs over the window, not inside the host panel', async () => {
		// The dashboard's panel carries a backdrop-filter, which would re-anchor
		// a fixed-position dialog to the panel's box if it rendered in place.
		const panel = document.createElement('section');
		panel.className = 'panel';
		document.body.appendChild(panel);
		try {
			render(MapsSurface, { target: panel });

			await fireEvent.click(await screen.findByRole('button', { name: 'Setup' }));
			await fireEvent.click(screen.getByRole('menuitem', { name: 'Calibrate radar guidance' }));

			const dialog = await screen.findByRole('dialog');
			expect(panel.contains(dialog)).toBe(false);
			expect(mocked.startRadarCalibration).toHaveBeenCalledOnce();
		} finally {
			panel.remove();
		}
	});

	it('explains an installation with no bundled maps instead of offering controls', async () => {
		mocked.getPlanetMaps.mockResolvedValue([]);
		render(MapsSurface);

		expect(
			await screen.findByText(
				'No planet maps are bundled with this installation, so the maps surface is unavailable.',
			),
		).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Pin overlay' })).toBeNull();
	});
});
