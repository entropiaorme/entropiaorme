import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocked = vi.hoisted(() => ({
	beginSelection: vi.fn(),
	startNavigation: vi.fn(),
	scanCoordinates: vi.fn(),
	snapshot: vi.fn(),
	inputStatus: vi.fn(),
	updatePosition: vi.fn(),
	guidance: vi.fn(),
	resume: vi.fn(),
	emit: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({ emit: mocked.emit }));
vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn((_key: string, fallback: unknown) => Promise.resolve(fallback)),
	setPreference: vi.fn(),
}));
vi.mock('$lib/api', () => ({
	beginNavigationAreaSelection: mocked.beginSelection,
	endNavigation: vi.fn(),
	getInputStatus: mocked.inputStatus,
	getNavigationSnapshot: mocked.snapshot,
	getRadarGuidance: mocked.guidance,
	hideNavigationOverlays: vi.fn(),
	showNavigationOverlays: vi.fn(),
	markNavigationVisited: vi.fn(),
	resolveNavigationHarvest: vi.fn(),
	scanMapCoordinates: mocked.scanCoordinates,
	skipNavigationStop: vi.fn(),
	startNavigation: mocked.startNavigation,
	undoNavigationStop: vi.fn(),
	updateNavigationPosition: mocked.updatePosition,
	resumeNavigationPosition: mocked.resume,
}));
vi.mock('./cartographyOverlay.svelte', () => ({
	acceptCartographyContextBroadcast: (value: unknown) => value,
}));

import { createNavigationHudController, guidanceNote } from './navigationHudController.svelte';

beforeEach(() => {
	vi.clearAllMocks();
	mocked.beginSelection.mockResolvedValue(undefined);
	mocked.scanCoordinates.mockResolvedValue({ kind: 'read', lon: 100, lat: 200, source: 'radar' });
	mocked.startNavigation.mockResolvedValue({ status: 'active', stops: [] });
	mocked.snapshot.mockResolvedValue(null);
	mocked.inputStatus.mockResolvedValue({
		observer: 'xinput',
		keyless: false,
		keylessAvailable: true,
	});
});

describe('the F-key visit mode on a machine that cannot read keys', () => {
	async function startedRoute(keyless: boolean, preferred: boolean) {
		const preferences = await import('$lib/preferences');
		vi.mocked(preferences.getPreference).mockImplementation((key: string, fallback: unknown) =>
			Promise.resolve(key === 'navVisitOnKey' ? preferred : fallback),
		);
		mocked.inputStatus.mockResolvedValue({ observer: null, keyless, keylessAvailable: true });
		const controller = createNavigationHudController();
		await controller.loadPrefs();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		await controller.captureStart();
		await controller.beginRoute();
		return controller;
	}

	it('is withheld while keyless, whatever was preferred', async () => {
		const controller = await startedRoute(true, true);
		expect(controller.keyless).toBe(true);
		expect(controller.visitOnKey).toBe(false);
		expect(mocked.startNavigation.mock.calls[0][5]).toBe(false);
	});

	it('follows the preference where keys can be read', async () => {
		const controller = await startedRoute(false, true);
		expect(controller.visitOnKey).toBe(true);
		expect(mocked.startNavigation.mock.calls[0][5]).toBe(true);
	});
});

describe('navigation HUD route-area scope', () => {
	it('keeps all trees as the default and accepts only the current selection request', async () => {
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		await controller.chooseRouteArea();
		expect(mocked.beginSelection).toHaveBeenCalledWith(1, 'Calypso', null);

		controller.applyRouteAreaSelection({
			requestId: 99,
			planet: 'Calypso',
			mapViewId: null,
			pinIds: [9],
		});
		expect(controller.selectedTreeCount).toBeNull();

		controller.applyRouteAreaSelection({
			requestId: 1,
			planet: 'Calypso',
			mapViewId: null,
			pinIds: [9, 3, 9],
		});
		expect(controller.selectedTreeCount).toBe(2);
	});

	it('starts with the exact selection and consumes it back to the all-trees default', async () => {
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: 7 });
		await controller.chooseRouteArea();
		controller.applyRouteAreaSelection({
			requestId: 1,
			planet: 'Calypso',
			mapViewId: 7,
			pinIds: [8, 2],
		});
		await controller.captureStart();
		await controller.beginRoute();

		expect(mocked.startNavigation).toHaveBeenCalledWith('Calypso', 7, 100, 200, [2, 8], false);
		expect(controller.selectedTreeCount).toBeNull();
	});

	it('returns an area-scoped draft to all trees explicitly', async () => {
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		await controller.chooseRouteArea();
		controller.applyRouteAreaSelection({
			requestId: 1,
			planet: 'Calypso',
			mapViewId: null,
			pinIds: [4],
		});
		controller.useAllTrees();
		expect(controller.selectedTreeCount).toBeNull();
		expect(mocked.emit).toHaveBeenCalledWith('navigation-area-selection-reset');
	});
});

describe('navigation HUD visit mode', () => {
	it('plans a route that visits on F once the toggle is on', async () => {
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		controller.setVisitOnKey(true);
		await controller.captureStart();
		await controller.beginRoute();

		expect(mocked.startNavigation).toHaveBeenCalledWith('Calypso', null, 100, 200, null, true);
	});

	it('acknowledges a tree recorded by an F press as cut', async () => {
		const stop = (status: string, completionSource: string | null = null) => ({
			id: 1,
			status,
			completionSource,
		});
		mocked.startNavigation.mockResolvedValue({ status: 'active', stops: [stop('active')] });
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		await controller.captureStart();
		await controller.beginRoute();

		mocked.snapshot.mockResolvedValue({ status: 'completed', stops: [stop('visited', 'key')] });
		await controller.hydrate();

		expect(controller.badge).toBe('cut');
		controller.dispose();
	});
});

describe('capturing the route start', () => {
	it('takes only a read position, and says why a refusal gave none', async () => {
		mocked.scanCoordinates.mockResolvedValue({
			kind: 'refused',
			refusal: { reason: 'radarNotFound', lon: null, lat: null },
		});
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		await controller.captureStart();
		expect(controller.start).toBeNull();
		expect(controller.feedback).toBe('The radar is not on the screen.');
		expect(controller.canStart).toBe(false);
	});

	it('drops a read that lands after the planet changed', async () => {
		let land: (value: unknown) => void = () => {};
		mocked.scanCoordinates.mockReturnValue(new Promise((resolve) => (land = resolve)));
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		const capturing = controller.captureStart();
		controller.applyContext({ planet: 'Arkadia', mapViewId: null });
		land({ kind: 'read', lon: 100, lat: 200, source: 'radar' });
		await capturing;
		expect(controller.start).toBeNull();
	});
});

describe('the position poll', () => {
	const refused = (reason: string, lon: string | null = null) => ({
		status: 'notRead',
		run: null,
		refusal: { reason, lon, lat: null },
		stopped: null,
	});

	it('says why it cannot read only once the same failure has held for two polls', async () => {
		const controller = createNavigationHudController();
		controller.applyContext({ planet: 'Calypso', mapViewId: null });
		mocked.updatePosition.mockResolvedValue(refused('notRead', 'covered'));
		await controller.pollPosition();
		expect(controller.note).toBeNull();
		// A different failure starts the count again.
		mocked.updatePosition.mockResolvedValue(refused('radarNotFound'));
		await controller.pollPosition();
		expect(controller.note).toBeNull();
		await controller.pollPosition();
		expect(controller.note).toBe('The radar is not on the screen.');
		mocked.updatePosition.mockResolvedValue({
			status: 'updated',
			run: { status: 'active', stops: [], positionAgeS: 0 },
			refusal: null,
			stopped: null,
		});
		await controller.pollPosition();
		expect(controller.note).toBeNull();
	});

	it('stops when the backend stops it, says why, and resumes on request', async () => {
		const controller = createNavigationHudController();
		mocked.updatePosition.mockResolvedValue({
			status: 'pollStopped',
			run: null,
			refusal: null,
			stopped: 'captureNotAllowed',
		});
		await controller.pollPosition();
		expect(controller.note).toBe('Screen capture was not allowed, so position updates stopped.');
		expect(controller.pollStopped).toBe(true);
		await controller.pollPosition();
		await controller.pollPosition();
		expect(mocked.updatePosition).toHaveBeenCalledOnce();

		mocked.resume.mockResolvedValue(undefined);
		await controller.resumePolling();
		expect(mocked.resume).toHaveBeenCalledOnce();
		expect(controller.pollStopped).toBe(false);
		expect(controller.note).toBeNull();
	});

	it('takes one read at a time', async () => {
		let land: (value: unknown) => void = () => {};
		mocked.updatePosition.mockReturnValue(new Promise((resolve) => (land = resolve)));
		const controller = createNavigationHudController();
		const first = controller.pollPosition();
		await controller.pollPosition();
		land({ status: 'jumpRefused', run: null, refusal: null, stopped: null });
		await first;
		expect(mocked.updatePosition).toHaveBeenCalledOnce();
	});

	it('does not steer from a position grown old', async () => {
		vi.useFakeTimers();
		try {
			const controller = createNavigationHudController();
			mocked.updatePosition.mockResolvedValue({
				status: 'updated',
				run: { status: 'active', stops: [], positionAgeS: 0 },
				refusal: null,
				stopped: null,
			});
			await controller.pollPosition();
			expect(controller.positionCurrent).toBe(true);
			expect(controller.note).toBeNull();
			// Ten seconds on with no new position, at a one-second poll.
			vi.advanceTimersByTime(10_000);
			controller.tick();
			expect(controller.positionCurrent).toBe(false);
			expect(controller.note).toBe('Your position is not current.');
		} finally {
			vi.useRealTimers();
		}
	});
});

describe('the radar guidance note', () => {
	const placed = (
		scaleMissing: 'labelsUnread' | 'noRings' | 'noReader' | null,
		source: 'found' | 'calibrated' = 'found',
	) =>
		({
			kind: 'placed',
			centreX: 0,
			centreY: 0,
			radiusPx: 113,
			clearRightX: 130,
			source,
			scale: scaleMissing ? null : { metresPerPixel: 1.9, edgeRangeM: 219 },
			scaleMissing,
			labelStrip: { from: 0.1, to: 1, halfHeight: 0.1 },
		}) as const;

	it('asks for ring distances only when the rings were found without them', () => {
		expect(guidanceNote(placed('labelsUnread'))).toBe(
			"Show ring distances in the game's interface options to see distances on the radar.",
		);
		expect(guidanceNote(placed('noRings'))).toBe(
			"The radar's rings could not be seen, so distances are not shown on it.",
		);
		expect(guidanceNote(placed('noReader'))).toBe(
			'Distances on the radar need the text recogniser, which is not available.',
		);
		expect(guidanceNote(placed(null))).toBeNull();
		expect(guidanceNote(placed(null, 'calibrated'))).toBeNull();
	});

	it('says why guidance has no placement or stopped', () => {
		expect(guidanceNote({ kind: 'lost', reason: 'radarNotFound' })).toBe(
			'The radar is not on the screen.',
		);
		expect(guidanceNote({ kind: 'off', stopped: 'captureNotAllowed' })).toBe(
			'Screen capture was not allowed, so radar guidance stopped.',
		);
		expect(guidanceNote({ kind: 'off', stopped: 'failed' })).toBe(
			'Radar guidance stopped unexpectedly.',
		);
		expect(guidanceNote({ kind: 'off', stopped: null })).toBeNull();
		expect(guidanceNote({ kind: 'searching' })).toBeNull();
	});
});
