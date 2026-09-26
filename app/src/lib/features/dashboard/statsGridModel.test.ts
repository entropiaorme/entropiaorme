// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	DEFAULT_OVERLAY_PREFS,
	DEFAULT_STAT_PREFS,
	dashboardStats,
	overlayStats,
	type StatPref,
} from '$lib/statsCustomisation.svelte';
import type { StatId } from '$lib/statsRegistry';
import { createStatsGridModel } from './statsGridModel.svelte';

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/event', () => ({
	emit: vi.fn().mockResolvedValue(undefined),
	listen: vi.fn(),
}));

import { setPreference } from '$lib/preferences';

function prefsWith(enabled: StatId[]): StatPref[] {
	return DEFAULT_STAT_PREFS.map((p) => ({ id: p.id, enabled: enabled.includes(p.id) }));
}

function enabledIds(): string[] {
	return dashboardStats.current.filter((p) => p.enabled).map((p) => p.id);
}

/** A minimal pointer-event stand-in carrying only the fields the handlers read. */
function pointerEvent(
	target: EventTarget,
	overrides: Partial<{ button: number; clientX: number; clientY: number }> = {},
): PointerEvent {
	return {
		button: 0,
		pointerId: 1,
		clientX: 0,
		clientY: 0,
		currentTarget: target,
		...overrides,
	} as unknown as PointerEvent;
}

function cellTarget() {
	return {
		setPointerCapture: vi.fn(),
		hasPointerCapture: vi.fn().mockReturnValue(true),
		releasePointerCapture: vi.fn(),
	} as unknown as HTMLElement;
}

/** Mount fake grid cells whose bounding rects tile a 100px-tall column. */
function mountCells(count: number) {
	document.body.innerHTML = '';
	for (let i = 0; i < count; i++) {
		const cell = document.createElement('div');
		cell.dataset.statCell = String(i);
		cell.getBoundingClientRect = () =>
			({ left: 0, right: 100, top: i * 100, bottom: (i + 1) * 100 }) as DOMRect;
		document.body.appendChild(cell);
	}
}

beforeEach(() => {
	vi.clearAllMocks();
	document.body.innerHTML = '';
	document.body.classList.remove('stat-drag-active');
	dashboardStats.current = DEFAULT_STAT_PREFS;
	overlayStats.current = DEFAULT_OVERLAY_PREFS;
});

describe('enabledStats', () => {
	it('projects only the enabled prefs, in stored order', () => {
		dashboardStats.current = prefsWith(['net', 'cycled']);
		const model = createStatsGridModel();
		expect(model.enabledStats.map((p) => p.id)).toEqual(['cycled', 'net']);
	});
});

describe('drag reorder', () => {
	it('moves the dragged stat past the threshold while disabled stats keep their slots', () => {
		dashboardStats.current = prefsWith(['cycled', 'net', 'rate']); // loot_tt stays disabled between them
		mountCells(3);
		const model = createStatsGridModel();
		const target = cellTarget();

		model.handlePointerDown(pointerEvent(target, { clientX: 50, clientY: 50 }), 0);
		expect(document.body.classList.contains('stat-drag-active')).toBe(true);
		expect(model.dragFilteredIndex).toBe(0);

		// Drop the first enabled stat (cycled) onto the last cell.
		model.handlePointerMove(pointerEvent(target, { clientX: 50, clientY: 250 }));
		expect(enabledIds()).toEqual(['net', 'rate', 'cycled']);
		expect(model.dragFilteredIndex).toBe(2);

		model.handlePointerUp(pointerEvent(target, { clientX: 50, clientY: 250 }));
		expect(model.dragFilteredIndex).toBeNull();
		expect(document.body.classList.contains('stat-drag-active')).toBe(false);
		// A real move persists through the canonical setter.
		expect(setPreference).toHaveBeenCalled();
	});

	it('treats sub-threshold jitter as a click: no reorder, no persist', () => {
		dashboardStats.current = prefsWith(['cycled', 'net', 'rate']);
		mountCells(3);
		const model = createStatsGridModel();
		const target = cellTarget();

		model.handlePointerDown(pointerEvent(target, { clientX: 50, clientY: 50 }), 0);
		model.handlePointerMove(pointerEvent(target, { clientX: 52, clientY: 51 }));
		expect(enabledIds()).toEqual(['cycled', 'net', 'rate']);

		model.handlePointerUp(pointerEvent(target, { clientX: 52, clientY: 51 }));
		expect(setPreference).not.toHaveBeenCalled();
	});

	it('ignores non-primary buttons and restores cleanly on cancel', () => {
		dashboardStats.current = prefsWith(['cycled', 'net']);
		mountCells(2);
		const model = createStatsGridModel();
		const target = cellTarget();

		model.handlePointerDown(pointerEvent(target, { button: 2 }), 0);
		expect(model.dragFilteredIndex).toBeNull();

		model.handlePointerDown(pointerEvent(target, { clientX: 50, clientY: 50 }), 0);
		model.handlePointerCancel();
		expect(model.dragFilteredIndex).toBeNull();
		expect(document.body.classList.contains('stat-drag-active')).toBe(false);
	});
});
