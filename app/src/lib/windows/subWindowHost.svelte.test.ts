// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { SatelliteWindow } from './satellite';
import { type SubWindowReport, subWindowChannel } from './subWindow';
import {
	createSubWindowHost,
	SUB_WINDOW_AWAY_MS,
	SUB_WINDOW_RECLICK_MS,
	type SubWindowRequest,
} from './subWindowHost.svelte';

function fakeWindow() {
	return {
		ensure: vi.fn(async () => ({}) as never),
		show: vi.fn(async (_state: unknown, _position?: unknown, _opts?: unknown) => {}),
		hide: vi.fn(async () => {}),
		emitTo: vi.fn(async (_event: string, _payload?: unknown) => {}),
		isFocused: vi.fn(async () => false),
	} satisfies SatelliteWindow;
}

function rig() {
	const menu = fakeWindow();
	const task = fakeWindow();
	const channels = {
		menu: subWindowChannel('menu', '/menu'),
		task: subWindowChannel('task', '/task'),
	};
	const handlers = new Map<string, (report: SubWindowReport) => void>();
	const closed: [string, string][] = [];
	const errors: [string, unknown][] = [];
	const interacted: string[] = [];
	let clock = 1000;
	const host = createSubWindowHost({
		slots: {
			menu: { window: menu, channel: channels.menu },
			task: { window: task, channel: channels.task },
		},
		gap: 6,
		anchorOf: async () => ({ left: 10, right: 60, below: 100, above: 20 }),
		bounds: async () => null,
		listen: async (event, handler) => {
			handlers.set(event, handler);
			return () => handlers.delete(event);
		},
		onClosed: (id, by) => closed.push([id, by]),
		onError: (id, error) => errors.push([id, error]),
		onInteract: (id) => interacted.push(id),
		now: () => clock,
	});
	const disconnect = host.connect();
	const anchor = () => {
		const el = document.createElement('button');
		document.body.append(el);
		return el;
	};
	const request = (id: string, extra: Partial<SubWindowRequest> = {}): SubWindowRequest => ({
		slot: 'menu',
		id,
		anchor: anchor(),
		state: { id },
		...extra,
	});
	return {
		host,
		menu,
		task,
		closed,
		errors,
		interacted,
		request,
		disconnect,
		tick(ms: number) {
			clock += ms;
		},
		report(slot: 'menu' | 'task', message: SubWindowReport) {
			handlers.get(channels[slot].report)?.(message);
		},
	};
}

/** Let the promises a call started settle. */
const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
	document.body.replaceChildren();
});

describe('opening', () => {
	it('hands the sub-window what to show and where its control is, and leaves the reveal to it', async () => {
		const r = rig();
		expect(await r.host.open(r.request('picker'))).toBe(true);
		expect(r.host.isOpen('picker')).toBe(true);
		expect(r.host.shownIn('menu')).toBe('picker');
		expect(r.menu.show).toHaveBeenCalledWith(
			{
				id: 'picker',
				state: { id: 'picker' },
				anchor: { left: 10, right: 60, below: 100, above: 20 },
				bounds: null,
				gap: 6,
				align: 'start',
				focus: true,
				stays: false,
			},
			undefined,
			{ reveal: false, focus: true },
		);
	});

	it('opens nothing from a control that is gone', async () => {
		const r = rig();
		const gone = r.request('picker');
		gone.anchor.remove();
		expect(await r.host.open(gone)).toBe(false);
		expect(r.menu.show).not.toHaveBeenCalled();
	});

	it('reports a sub-window that could not be opened, and is left with none', async () => {
		const r = rig();
		r.menu.ensure.mockRejectedValueOnce(new Error('no window'));
		expect(await r.host.open(r.request('picker'))).toBe(false);
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.errors).toEqual([['picker', new Error('no window')]]);
	});

	it('brings the one already open up to date without showing it again', async () => {
		const r = rig();
		const first = r.request('activities');
		await r.host.open(first);
		await r.host.open({ ...first, state: { rows: 3 } });
		expect(r.menu.show).toHaveBeenCalledTimes(1);
		expect(r.menu.emitTo).toHaveBeenCalledWith(
			'menu:update',
			expect.objectContaining({ id: 'activities', state: { rows: 3 } }),
		);
	});
});

describe('one at a time', () => {
	it('replaces what its window shows, and closes what another window shows', async () => {
		const r = rig();
		await r.host.open(r.request('picker'));
		await r.host.open(r.request('doses'));
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.host.isOpen('doses')).toBe(true);
		await r.host.open(r.request('recording', { slot: 'task' }));
		expect(r.host.isOpen('doses')).toBe(false);
		expect(r.menu.hide).toHaveBeenCalledTimes(1);
		expect(r.closed).toEqual([
			['picker', 'replaced'],
			['doses', 'replaced'],
		]);
	});

	it('leaves one that stays where it is while others come and go', async () => {
		const r = rig();
		await r.host.open(r.request('recording', { slot: 'task', stays: true }));
		await r.host.open(r.request('picker'));
		expect(r.host.isOpen('recording')).toBe(true);
		expect(r.host.isOpen('picker')).toBe(true);
		expect(r.task.hide).not.toHaveBeenCalled();
	});
});

describe('closing', () => {
	it('closes on a second click of its control', async () => {
		const r = rig();
		const picker = r.request('picker');
		expect(await r.host.toggle(picker)).toBe(true);
		expect(await r.host.toggle(picker)).toBe(false);
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.menu.hide).toHaveBeenCalledTimes(1);
		expect(r.closed).toEqual([['picker', 'toggle']]);
	});

	it('follows a sub-window that closed itself', async () => {
		const r = rig();
		await r.host.open(r.request('picker'));
		r.report('menu', { type: 'closed', id: 'picker', reason: 'escape' });
		expect(r.host.isOpen('picker')).toBe(false);
		// It hid itself: the host does not hide it again.
		expect(r.menu.hide).not.toHaveBeenCalled();
		expect(r.closed).toEqual([['picker', 'escape']]);
	});

	it('takes the click that took the keyboard from it as its close, once', async () => {
		const r = rig();
		const picker = r.request('picker');
		await r.host.open(picker);
		r.report('menu', { type: 'closed', id: 'picker', reason: 'blur' });
		r.tick(SUB_WINDOW_RECLICK_MS - 1);
		expect(await r.host.toggle(picker)).toBe(false);
		expect(r.menu.show).toHaveBeenCalledTimes(1);
		// The next click opens it.
		expect(await r.host.toggle(picker)).toBe(true);
		// And a click long after it closed itself is a new click.
		r.report('menu', { type: 'closed', id: 'picker', reason: 'blur' });
		r.tick(SUB_WINDOW_RECLICK_MS);
		expect(r.host.reclicked('picker')).toBe(false);
	});

	it('ignores a report about a sub-window it no longer shows', async () => {
		const r = rig();
		await r.host.open(r.request('picker'));
		await r.host.open(r.request('doses'));
		r.report('menu', { type: 'closed', id: 'picker', reason: 'blur' });
		expect(r.host.isOpen('doses')).toBe(true);
	});

	it('closes all but those it is told to keep', async () => {
		const r = rig();
		await r.host.open(r.request('recording', { slot: 'task', stays: true }));
		await r.host.open(r.request('picker'));
		await r.host.closeAll((id) => id === 'recording');
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.host.isOpen('recording')).toBe(true);
		await r.host.closeAll();
		expect(r.host.isOpen('recording')).toBe(false);
	});

	it('never reopens one that closed while its update was on the way', async () => {
		const r = rig();
		const panel = r.request('panel');
		await r.host.open(panel);
		// The update is asked for, then the panel gives way to a menu before
		// it lands.
		const updating = r.host.update('panel', { fresh: true });
		const menu = r.host.open(r.request('doses'));
		await Promise.all([updating, menu]);
		expect(r.host.shownIn('menu')).toBe('doses');
		expect(r.menu.show).toHaveBeenCalledTimes(2);
	});
});

describe('left alone', () => {
	async function opened(extra: Partial<SubWindowRequest> = {}) {
		const r = rig();
		const picker = r.request('picker', extra);
		await r.host.open(picker);
		return { ...r, picker };
	}
	const away = (el: HTMLElement) => el.dispatchEvent(new Event('pointerleave'));
	const back = (el: HTMLElement) => el.dispatchEvent(new Event('pointerenter'));

	it('closes a moment after the pointer leaves its control, unless it comes back', async () => {
		const r = await opened();
		away(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS - 1);
		expect(r.host.isOpen('picker')).toBe(true);
		back(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS * 2);
		expect(r.host.isOpen('picker')).toBe(true);
		away(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS);
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.closed).toEqual([['picker', 'away']]);
		expect(r.menu.hide).toHaveBeenCalledTimes(1);
	});

	it('is not kept open by being brought up to date', async () => {
		const r = await opened();
		away(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS - 10);
		// The overlay moved, and what the sub-window shows changed.
		await r.host.reanchor();
		await r.host.update('picker', { fresh: true });
		await vi.advanceTimersByTimeAsync(10);
		expect(r.host.isOpen('picker')).toBe(false);
	});

	it('stays while the pointer is over the sub-window itself', async () => {
		const r = await opened();
		away(r.picker.anchor);
		r.report('menu', { type: 'pointer', id: 'picker', inside: true });
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS * 3);
		expect(r.host.isOpen('picker')).toBe(true);
		r.report('menu', { type: 'pointer', id: 'picker', inside: false });
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS);
		expect(r.host.isOpen('picker')).toBe(false);
	});

	it('stays while the player is typing in it, and goes by the same rule once they stop', async () => {
		const r = await opened();
		r.report('menu', { type: 'held', id: 'picker', held: true });
		away(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS * 3);
		expect(r.host.isOpen('picker')).toBe(true);
		r.report('menu', { type: 'held', id: 'picker', held: false });
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS);
		expect(r.host.isOpen('picker')).toBe(false);
	});

	it('never closes one that stays, nor one attached to a field', async () => {
		for (const kind of [{ stays: true }, { attached: true }]) {
			const r = await opened(kind);
			away(r.picker.anchor);
			await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS * 3);
			expect(r.host.isOpen('picker')).toBe(true);
		}
	});

	it('stops following the pointer over a control whose sub-window has closed', async () => {
		const r = await opened();
		await r.host.close('picker');
		away(r.picker.anchor);
		await vi.advanceTimersByTimeAsync(SUB_WINDOW_AWAY_MS);
		expect(r.closed).toEqual([['picker', 'host']]);
	});
});

describe('following the overlay', () => {
	it('lets every open sub-window follow its control, closing one whose control is gone', async () => {
		const r = rig();
		const recording = r.request('recording', { slot: 'task', stays: true });
		const picker = r.request('picker');
		await r.host.open(recording);
		await r.host.open(picker);
		await r.host.reanchor();
		expect(r.menu.emitTo).toHaveBeenCalledWith(
			'menu:update',
			expect.objectContaining({ id: 'picker' }),
		);
		expect(r.task.emitTo).toHaveBeenCalledWith(
			'task:update',
			expect.objectContaining({ id: 'recording' }),
		);
		// Following never closes anything else.
		expect(r.host.isOpen('recording')).toBe(true);
		picker.anchor.remove();
		await r.host.reanchor();
		expect(r.host.isOpen('picker')).toBe(false);
		expect(r.host.isOpen('recording')).toBe(true);
	});

	it('passes on what the player does in a sub-window', async () => {
		const r = rig();
		await r.host.open(r.request('mob', { attached: true, focus: false }));
		r.report('menu', { type: 'interact', id: 'mob' });
		expect(r.interacted).toEqual(['mob']);
		expect(r.menu.show).toHaveBeenLastCalledWith(
			expect.objectContaining({ focus: false }),
			undefined,
			{ reveal: false, focus: false },
		);
	});

	it('hears nothing more once disconnected', async () => {
		const r = rig();
		await settle();
		await r.host.open(r.request('picker'));
		r.disconnect();
		r.report('menu', { type: 'closed', id: 'picker', reason: 'escape' });
		expect(r.host.isOpen('picker')).toBe(true);
	});
});
