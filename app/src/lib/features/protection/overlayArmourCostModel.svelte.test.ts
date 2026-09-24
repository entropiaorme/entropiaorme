// @vitest-environment happy-dom

import { describe, expect, it, vi } from 'vitest';
import type { ProtectionOverview } from '$lib/api';
import type { SatelliteWindow } from '$lib/windows/satellite';
import { createOverlayArmourCostModel } from './overlayArmourCostModel.svelte';

// Placement reads the native window; the anchor's own position is not what
// these cover, so it stands in as a fixed point.
vi.mock('$lib/windows/anchor', () => ({
	anchorCentreBelow: vi.fn(async () => ({ centerX: 0, top: 0 })),
	createAnchorTracker: () => ({ schedule: () => {}, stop: () => {} }),
}));

const overview: ProtectionOverview = {
	sets: [],
	loadouts: [
		{
			id: '10',
			name: 'Hyperion + 5B',
			armour: { id: '1', name: 'Hyperion', economyKind: 'unlimited', markupPercent: null },
			plates: null,
		},
	],
	activeLoadoutId: '10',
	recentReconciliations: [],
	recentCostWindows: [],
};

function satellite(): SatelliteWindow {
	return {
		ensure: vi.fn(async () => ({}) as never),
		show: vi.fn(async () => {}),
		hide: vi.fn(async () => {}),
		emitTo: vi.fn(async () => {}),
	};
}

/** A button that is in the document, as a rendered anchor would be. */
function anchorElement(): HTMLElement {
	const button = document.createElement('button');
	document.body.appendChild(button);
	return button;
}

function model(anchor: () => HTMLElement | null, sessionId: string | null = 's1') {
	return createOverlayArmourCostModel({
		window: satellite(),
		anchorGap: 4,
		sessionId: () => sessionId,
		repairOcrEnabled: () => false,
		bySegment: () => false,
		protection: () => overview,
		inSessionAnchor: anchor,
	});
}

describe('overlay armour cost model', () => {
	it('opens against an anchor the host renders a few flushes late', async () => {
		let target: HTMLElement | null = null;
		// The host renders its Cost control only once the readout has, which
		// is not guaranteed to be the flush the workflow is asked in, and a
		// readout that arrives off a timer settles after every flush alone
		// could observe.
		setTimeout(() => {
			target = anchorElement();
		}, 0);

		const armour = model(() => target);
		expect(await armour.showInSession()).toBe(true);
		expect(armour.open).toBe(true);
		expect(armour.error).toBeNull();
	});

	it('says so rather than failing quietly when the anchor never arrives', async () => {
		const armour = model(() => null);
		expect(await armour.showInSession()).toBe(false);
		expect(armour.open).toBe(false);
		expect(armour.error).toBe('The armour cost window could not be opened');
	});

	it('says so when there is no session left to record against', async () => {
		const target = anchorElement();
		const armour = model(() => target, null);
		expect(await armour.showInSession()).toBe(false);
		expect(armour.error).toBe('There is no session left to record an armour cost against');
	});
});
