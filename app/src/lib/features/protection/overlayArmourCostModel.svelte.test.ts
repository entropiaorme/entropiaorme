// @vitest-environment happy-dom

import { describe, expect, it, vi } from 'vitest';
import type { SatelliteWindow } from '$lib/windows/satellite';
import { createOverlayArmourCostModel } from './overlayArmourCostModel.svelte';

// Placement reads the native window; the anchor's own position is not what
// these cover, so it stands in as a fixed point.
vi.mock('$lib/windows/anchor', () => ({
	anchorCentreBelow: vi.fn(async () => ({ centerX: 0, top: 0 })),
	createAnchorTracker: () => ({ schedule: () => {}, cancel: () => {}, stop: () => {} }),
}));

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

function model(window: SatelliteWindow = satellite(), repairOcrEnabled = false) {
	return createOverlayArmourCostModel({
		window,
		anchorGap: 4,
		repairOcrEnabled: () => repairOcrEnabled,
	});
}

function click(target: HTMLElement): MouseEvent {
	const event = new MouseEvent('click');
	Object.defineProperty(event, 'currentTarget', { value: target });
	return event;
}

describe('overlay armour cost model', () => {
	it('opens against the Cost control with no session running', async () => {
		const window = satellite();
		const armour = model(window, true);
		await armour.toggle(click(anchorElement()));
		expect(armour.open).toBe(true);
		expect(armour.error).toBeNull();
		expect(window.show).toHaveBeenCalledWith(
			{ repairOcrEnabled: true, anchor: { centerX: 0, top: 0 } },
			undefined,
			{ reveal: false },
		);
	});

	it('does not open against a control that has left the document', async () => {
		const armour = model();
		const detached = document.createElement('button');
		expect(await armour.show(detached)).toBe(false);
		expect(armour.open).toBe(false);
	});

	it('closes on a second press', async () => {
		const window = satellite();
		const armour = model(window);
		const target = anchorElement();
		await armour.toggle(click(target));
		await armour.toggle(click(target));
		expect(armour.open).toBe(false);
		expect(window.hide).toHaveBeenCalledTimes(1);
	});

	it('does not reopen from the press that its own close raced', async () => {
		const window = satellite();
		const armour = model(window);
		const target = anchorElement();
		await armour.toggle(click(target));
		armour.noteClosed();
		await armour.toggle(click(target));
		expect(armour.open).toBe(false);
		expect(window.show).toHaveBeenCalledTimes(1);
	});
});
