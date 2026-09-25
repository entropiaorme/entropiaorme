// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TrackingSnapshot } from '$lib/api';
import { guideState, registerDemoApi, unregisterDemoApi } from '$lib/guide/state.svelte';
import { ARMOUR_POPUP_WIDTH, createGuideDemoModel } from './guideDemoModel.svelte';
import type { StatsGridModel } from './statsGridModel.svelte';

vi.mock('$lib/api', () => ({
	getTrackingSnapshot: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function statsGridStub(): StatsGridModel {
	return {
		snapshotStats: vi.fn().mockReturnValue({ dashboard: [], overlay: [] }),
		restoreStats: vi.fn(),
		setDemoStatsBaseline: vi.fn(),
		toggleDemoStatPill: vi.fn(),
		reorderDemoStat: vi.fn(),
		setDragVisualIndex: vi.fn(),
	} as unknown as StatsGridModel;
}

function snapshot(): TrackingSnapshot {
	return {
		status: 'active',
		elapsed: 120,
		currentTool: 'Sollomate Opalo',
		currentMob: 'Atrox Young',
		sessionName: 'Guide Demo',
		skillBoostPercent: null,
		currentActivity: 'hunting',
		hotbarKeysEnabled: true,
		repairOcrEnabled: true,
	} as TrackingSnapshot;
}

beforeEach(() => {
	vi.clearAllMocks();
	guideState.isActive = false;
	document.body.innerHTML = '';
});

describe('overlay spawn lifecycle', () => {
	it('resets the strip phase to idle whenever visibility flips', () => {
		const model = createGuideDemoModel(statsGridStub());
		const apiSurface = model.demoApi();

		apiSurface.setOverlayDemoVisible(true);
		apiSurface.setOverlayDemoTrackingStarted(true);
		expect(model.demoOverlayVisible).toBe(true);
		expect(model.overlayStripPhase).toBe('active');

		apiSurface.setOverlayDemoVisible(false);
		expect(model.demoOverlayVisible).toBe(false);
		expect(model.overlayStripPhase).toBe('idle');
	});
});

describe('armour popup', () => {
	it('positions the popup centred under the Cost button on show', () => {
		const btn = document.createElement('button');
		btn.dataset.guideAnchor = 'overlay-armour-cost-btn';
		btn.getBoundingClientRect = () =>
			({ left: 300, width: 40, bottom: 100, top: 80, right: 340 }) as DOMRect;
		document.body.appendChild(btn);

		const model = createGuideDemoModel(statsGridStub());
		model.demoApi().setOverlayArmourPopupVisible(true);

		expect(model.demoArmourPopupVisible).toBe(true);
		expect(model.armourPopupTop).toBe(108);
		expect(model.armourPopupLeft).toBe(300 + 20 - ARMOUR_POPUP_WIDTH / 2);
	});

	it('resets the recorded body when the popup hides', () => {
		const model = createGuideDemoModel(statsGridStub());
		const apiSurface = model.demoApi();

		apiSurface.setOverlayArmourPopupVisible(true);
		apiSurface.setOverlayArmourPopupRecorded(true);
		expect(model.demoArmourPopupRecorded).toBe(true);

		apiSurface.setOverlayArmourPopupVisible(false);
		expect(model.demoArmourPopupVisible).toBe(false);
		expect(model.demoArmourPopupRecorded).toBe(false);
	});
});

describe('refreshDemoTracking', () => {
	it('projects the demo snapshot while active and clears on close', async () => {
		mocked.getTrackingSnapshot.mockResolvedValue(snapshot());
		const model = createGuideDemoModel(statsGridStub());

		await model.refreshDemoTracking(true);
		expect(model.demoTrackingLive).toEqual({
			status: 'active',
			elapsed: 120,
			currentTool: 'Sollomate Opalo',
			currentMob: 'Atrox Young',
			sessionName: 'Guide Demo',
			skillBoostPercent: null,
			currentActivity: 'hunting',
			hotbarKeysEnabled: true,
			weaponGuardrail: undefined,
			unpricedShots: undefined,
			repairOcrEnabled: true,
		});

		await model.refreshDemoTracking(false);
		expect(model.demoTrackingLive).toBeNull();
	});

	it('keeps the last projection when the demo read fails', async () => {
		mocked.getTrackingSnapshot.mockResolvedValueOnce(snapshot());
		const model = createGuideDemoModel(statsGridStub());
		await model.refreshDemoTracking(true);

		mocked.getTrackingSnapshot.mockRejectedValue(new Error('demo route down'));
		await model.refreshDemoTracking(true);
		expect(model.demoTrackingLive?.status).toBe('active');
	});
});

describe('syncWidgetsTab', () => {
	it('forces pulse on open and restores the pre-guide tab on close', async () => {
		const setTab = vi.fn();
		guideState.isActive = true;
		registerDemoApi('dashboard-widgets', {
			setTab,
			getTab: () => 'quests',
		});
		try {
			const model = createGuideDemoModel(statsGridStub());
			model.syncWidgetsTab(true);
			expect(setTab).toHaveBeenLastCalledWith('pulse');

			// Re-entrant open keeps the original snapshot.
			model.syncWidgetsTab(true);

			model.syncWidgetsTab(false);
			await Promise.resolve(); // restore is microtask-deferred
			expect(setTab).toHaveBeenLastCalledWith('quests');
		} finally {
			unregisterDemoApi('dashboard-widgets');
			guideState.isActive = false;
		}
	});
});

describe('demoApi stats delegation', () => {
	it('routes every stat method to the stats-grid model', () => {
		const stub = statsGridStub();
		const apiSurface = createGuideDemoModel(stub).demoApi();

		apiSurface.snapshotStats();
		apiSurface.restoreStats({ dashboard: [], overlay: [] });
		apiSurface.setDemoStatsBaseline({ rate: false });
		apiSurface.toggleDemoStatPill('dashboard', 'pes');
		apiSurface.reorderDemoStat(3, 0);
		apiSurface.setDragVisualIndex(2);

		expect(stub.snapshotStats).toHaveBeenCalled();
		expect(stub.restoreStats).toHaveBeenCalledWith({ dashboard: [], overlay: [] });
		expect(stub.setDemoStatsBaseline).toHaveBeenCalledWith({ rate: false });
		expect(stub.toggleDemoStatPill).toHaveBeenCalledWith('dashboard', 'pes');
		expect(stub.reorderDemoStat).toHaveBeenCalledWith(3, 0);
		expect(stub.setDragVisualIndex).toHaveBeenCalledWith(2);
	});
});
