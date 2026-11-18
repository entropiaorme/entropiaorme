// @vitest-environment happy-dom

import { act, render, screen, waitFor } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The shell of every sub-window, against a faked window: what it is handed
// is drawn, the window is sized and placed before it is revealed, and what
// happens in it is reported to its host.
const seams = vi.hoisted(() => {
	const listeners = new Map<string, (event: { payload: unknown }) => void>();
	let focusChanged: ((event: { payload: boolean }) => void) | null = null;
	const order: string[] = [];
	const note = (name: string) => vi.fn(async (..._args: unknown[]) => void order.push(name));
	return {
		listeners,
		order,
		focus(focused: boolean) {
			focusChanged?.({ payload: focused });
		},
		window: {
			label: 'overlay-menu',
			listen: vi.fn(async (event: string, handler: (event: { payload: unknown }) => void) => {
				listeners.set(event, handler);
				return () => listeners.delete(event);
			}),
			onFocusChanged: vi.fn(async (handler: (event: { payload: boolean }) => void) => {
				focusChanged = handler;
				return () => {
					focusChanged = null;
				};
			}),
			emitTo: vi.fn(async (_target: string, _event: string, _payload?: unknown) => {}),
			setSize: note('setSize'),
			setPosition: note('setPosition'),
			show: note('show'),
			hide: note('hide'),
			setFocus: note('setFocus'),
		},
	};
});

vi.mock('@tauri-apps/api/webviewWindow', () => ({
	getCurrentWebviewWindow: () => seams.window,
}));
vi.mock('@tauri-apps/api/window', () => ({
	getCurrentWindow: () => seams.window,
}));
vi.mock('@tauri-apps/api/dpi', () => ({
	LogicalSize: class {
		constructor(
			public width: number,
			public height: number,
		) {}
	},
	LogicalPosition: class {
		constructor(
			public x: number,
			public y: number,
		) {}
	},
}));
vi.mock('$lib/api/shell', () => ({ setWindowHitRegion: vi.fn(async () => {}) }));

import { type SubWindowEnvelope, subWindowChannel } from '$lib/windows/subWindow';
import SubWindowShell from './SubWindowShell.svelte';

const channel = subWindowChannel('overlay-menu', '/overlay-menu');

function envelope(overrides: Partial<SubWindowEnvelope<{ text: string }>> = {}) {
	return {
		id: 'picker',
		state: { text: 'Pick one' },
		anchor: { left: 400, right: 520, below: 420, above: 300 },
		bounds: { x: 0, y: 0, width: 1920, height: 1080 },
		gap: 6,
		align: 'start' as const,
		focus: true,
		stays: false,
		...overrides,
	};
}

const body = createRawSnippet<[{ id: string; state: { text: string } }]>((held) => ({
	render: () => '<p data-testid="body"><span></span><input aria-label="Type" /></p>',
	setup: (node) => {
		const text = node.querySelector('span');
		// An effect of its own root: it follows what the shell hands on.
		return $effect.root(() => {
			$effect(() => {
				if (text) text.textContent = held().state.text;
			});
		});
	},
}));

async function mount() {
	// The shell is generic in what it shows; the test's body fixes it.
	render(SubWindowShell, { props: { channel, children: body as never } });
	await waitFor(() => expect(seams.listeners.has(channel.show)).toBe(true));
}

const emit = (event: string, payload?: unknown) =>
	act(async () => {
		seams.listeners.get(event)?.({ payload });
		await new Promise((resolve) => setTimeout(resolve, 60));
	});

/** What the shell told its host, in order. */
const reports = () =>
	(seams.window.emitTo.mock.calls as [string, string, unknown][])
		.filter((call) => call[1] === channel.report)
		.map((call) => call[2]);

let rect: ReturnType<typeof vi.spyOn>;
let scrollHeight: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
	seams.listeners.clear();
	seams.order.length = 0;
	for (const fn of Object.values(seams.window)) {
		if (typeof fn === 'function') (fn as ReturnType<typeof vi.fn>).mockClear();
	}
	rect = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
		x: 0,
		y: 0,
		left: 0,
		top: 0,
		right: 272,
		bottom: 180,
		width: 272,
		height: 180,
	} as DOMRect);
	scrollHeight = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(180);
});

afterEach(() => {
	rect.mockRestore();
	scrollHeight.mockRestore();
});

describe('showing', () => {
	it('tells its host it is listening, and draws nothing until it is handed something', async () => {
		await mount();
		await waitFor(() =>
			expect(seams.window.emitTo).toHaveBeenCalledWith('overlay', channel.ready, {
				label: 'overlay-menu',
			}),
		);
		expect(screen.queryByTestId('sub-window')).toBeNull();
	});

	it('sizes itself to what it draws and hangs below its control before it is revealed', async () => {
		await mount();
		await emit(channel.show, envelope());
		expect(screen.getByTestId('body').textContent).toContain('Pick one');
		expect(seams.window.setSize).toHaveBeenCalledWith({ width: 272, height: 180 });
		expect(seams.window.setPosition).toHaveBeenLastCalledWith({ x: 400, y: 426 });
		// Sized and placed first, then shown, then given the keyboard.
		expect(seams.order).toEqual(['setSize', 'setPosition', 'show', 'setFocus']);
		expect(screen.getByTestId('sub-window').dataset.subWindow).toBe('picker');
	});

	it('reveals itself without an animation frame, which a window not on screen is never given', async () => {
		const frames = vi.spyOn(window, 'requestAnimationFrame').mockReturnValue(1);
		try {
			await mount();
			await emit(channel.show, envelope());
			expect(seams.order).toEqual(['setSize', 'setPosition', 'show', 'setFocus']);
		} finally {
			frames.mockRestore();
		}
	});

	it('sits above its control where there is no room below', async () => {
		await mount();
		await emit(
			channel.show,
			envelope({ anchor: { left: 400, right: 520, below: 1000, above: 880 } }),
		);
		expect(seams.window.setPosition).toHaveBeenLastCalledWith({ x: 400, y: 880 - 6 - 180 });
	});

	it('takes no keyboard when it is shown without it', async () => {
		await mount();
		await emit(channel.show, envelope({ focus: false }));
		expect(seams.window.show).toHaveBeenCalled();
		expect(seams.window.setFocus).not.toHaveBeenCalled();
	});

	it('follows its control and new state without being revealed again', async () => {
		await mount();
		await emit(channel.show, envelope());
		await emit(
			channel.update,
			envelope({
				state: { text: 'Now three' },
				anchor: { left: 500, right: 620, below: 420, above: 300 },
			}),
		);
		expect(screen.getByTestId('body').textContent).toContain('Now three');
		expect(seams.window.setPosition).toHaveBeenLastCalledWith({ x: 500, y: 426 });
		expect(seams.window.show).toHaveBeenCalledTimes(1);
		// An update for something it does not show changes nothing.
		await emit(channel.update, envelope({ id: 'other', state: { text: 'Not mine' } }));
		expect(screen.getByTestId('body').textContent).toContain('Now three');
	});
});

describe('closing', () => {
	it('hides on Escape and says so', async () => {
		await mount();
		await emit(channel.show, envelope());
		await act(async () => {
			window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
			await new Promise((resolve) => setTimeout(resolve, 20));
		});
		expect(screen.queryByTestId('sub-window')).toBeNull();
		expect(seams.window.hide).toHaveBeenCalled();
		expect(reports()).toContainEqual({ type: 'closed', id: 'picker', reason: 'escape' });
	});

	it('closes when a click elsewhere takes the keyboard from it, but not as it settles', async () => {
		await mount();
		await emit(channel.show, envelope());
		// Losing the keyboard in the moment it is shown is the show settling.
		await act(async () => seams.focus(false));
		expect(screen.queryByTestId('sub-window')).not.toBeNull();
		await act(async () => {
			await new Promise((resolve) => setTimeout(resolve, 220));
			seams.focus(false);
			await new Promise((resolve) => setTimeout(resolve, 20));
		});
		expect(screen.queryByTestId('sub-window')).toBeNull();
		expect(reports()).toContainEqual({ type: 'closed', id: 'picker', reason: 'blur' });
	});

	it('stays through losing the keyboard when it is a task, or never took the keyboard', async () => {
		for (const kind of [{ stays: true }, { focus: false }]) {
			await mount();
			await emit(channel.show, envelope(kind));
			await act(async () => {
				await new Promise((resolve) => setTimeout(resolve, 220));
				seams.focus(false);
				await new Promise((resolve) => setTimeout(resolve, 20));
			});
			expect(screen.queryAllByTestId('sub-window').length).toBeGreaterThan(0);
			document.body.replaceChildren();
		}
	});

	it('hides when its host says so, without reporting a close of its own', async () => {
		await mount();
		await emit(channel.show, envelope());
		await emit(channel.hide);
		expect(screen.queryByTestId('sub-window')).toBeNull();
		expect(seams.window.hide).toHaveBeenCalled();
		expect(reports().filter((r) => (r as { type: string }).type === 'closed')).toEqual([]);
	});
});

describe('what happens in it', () => {
	it('reports the pointer coming and going, typing, and anything done in it', async () => {
		await mount();
		await emit(channel.show, envelope());
		const shell = screen.getByTestId('sub-window');
		const input = screen.getByLabelText('Type');
		await act(async () => {
			shell.dispatchEvent(new Event('pointerenter'));
			shell.dispatchEvent(new Event('pointerdown', { bubbles: true }));
			input.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
			input.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
			shell.dispatchEvent(new Event('pointerleave'));
		});
		expect(reports()).toEqual([
			{ type: 'pointer', id: 'picker', inside: true },
			{ type: 'interact', id: 'picker' },
			{ type: 'held', id: 'picker', held: true },
			{ type: 'held', id: 'picker', held: false },
			{ type: 'pointer', id: 'picker', inside: false },
		]);
	});
});
