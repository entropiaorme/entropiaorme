// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import type { MobSourceView } from '$lib/api';
import { OVERLAY_SURFACE_ATTRIBUTE, overlaySurfaces } from '$lib/windows/windowSize';
import MobSourceControl from './MobSourceControl.svelte';
import { createMobSourceModel } from './mobSourceModel.svelte';

const young = { name: 'Young Atrox', species: 'Atrox', maturity: 'Young' };
const atrox = { name: 'Old Atrox', species: 'Atrox', maturity: 'Old' };
const gloomrat = { name: 'Gloomrat', species: 'Gloomrat', maturity: '' };

function view(overrides: Partial<MobSourceView> = {}): MobSourceView {
	return {
		readingScreen: true,
		pin: null,
		typed: null,
		reading: null,
		continuity: null,
		...overrides,
	};
}

function mount(source: MobSourceView | null, steerable = true) {
	const deps = {
		view: () => source,
		steerable: () => steerable,
		pin: vi.fn(async () => ({})),
		unpin: vi.fn(async () => true),
		setTyped: vi.fn(async () => ({})),
		clearTyped: vi.fn(async () => ({})),
		refresh: vi.fn(async () => {}),
	};
	const model = createMobSourceModel(deps);
	const onReleaseMob = vi.fn();
	const result = render(MobSourceControl, { props: { view: source, model, onReleaseMob } });
	return { ...result, model, deps, onReleaseMob };
}

function shown(): HTMLElement {
	return screen.getByTestId('mob-shown');
}

/** The marked surfaces the control draws. */
function surfaces(container: HTMLElement): HTMLElement[] {
	return overlaySurfaces(container);
}

describe('what the control shows by default', () => {
	it('shows a pinned creature with Release', async () => {
		const { deps } = mount(
			view({ readingScreen: false, pin: { creature: atrox, by: 'read' }, typed: young }),
		);
		expect(shown().textContent).toContain('Old Atrox');
		expect(shown().dataset.standing).toBe('pinned');
		expect(screen.queryByRole('button', { name: 'Pin' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Release' }));
		expect(deps.unpin).toHaveBeenCalledTimes(1);
	});

	it('shows the creature read now with Pin, and pins that one', async () => {
		const { deps } = mount(view({ reading: atrox, continuity: gloomrat, typed: young }));
		expect(shown().textContent).toContain('Old Atrox');
		expect(shown().dataset.standing).toBe('reading');
		await fireEvent.click(screen.getByRole('button', { name: 'Pin' }));
		expect(deps.pin).toHaveBeenCalledWith({ kind: 'shown', creature: atrox });
	});

	it('shows what recent kills suggest when nothing is read', () => {
		mount(view({ continuity: gloomrat, typed: young }));
		expect(shown().textContent).toContain('Gloomrat');
		expect(shown().dataset.standing).toBe('continuity');
	});

	it('shows the backstop when only it names a creature', () => {
		mount(view({ typed: young }));
		expect(shown().textContent).toContain('Young Atrox');
		expect(shown().dataset.standing).toBe('backstop');
		expect(screen.getByRole('button', { name: 'Pin' })).toBeTruthy();
	});

	it('shows a quiet placeholder and no action when nothing names a creature', () => {
		mount(view());
		expect(shown().dataset.standing).toBe('none');
		expect(screen.queryByRole('button', { name: 'Pin' })).toBeNull();
		expect(screen.queryByRole('button', { name: 'Release' })).toBeNull();
	});

	it('shows nothing of the others by default, and no telemetry', () => {
		const { container } = mount(view({ reading: atrox, continuity: atrox, typed: young }));
		expect(screen.queryByText('Reading now')).toBeNull();
		expect(screen.queryByText('Your backstop')).toBeNull();
		expect(container.textContent).not.toMatch(/\d|%|grade|continuity/i);
	});

	it('is the typed mob alone with detection off or unavailable', async () => {
		const { onReleaseMob } = mount(view({ readingScreen: false, typed: young }));
		expect(screen.queryByTestId('mob-shown')).toBeNull();
		expect(screen.getByText('Young Atrox')).toBeTruthy();
		await fireEvent.click(screen.getByLabelText('Release mob'));
		expect(onReleaseMob).toHaveBeenCalledTimes(1);
	});

	it('takes a typed creature in the strip with detection off and none typed', () => {
		mount(view({ readingScreen: false }));
		expect(screen.getByPlaceholderText('Mob...')).toBeTruthy();
	});

	it('is the typed mob alone where the control cannot steer', () => {
		mount(view({ reading: atrox, typed: young }), false);
		expect(screen.queryByTestId('mob-shown')).toBeNull();
		expect(screen.getByText('Young Atrox')).toBeTruthy();
	});

	it('steers on any build: nothing but the control itself decides it', () => {
		mount(view({ reading: atrox, typed: young }));
		expect(shown().textContent).toContain('Old Atrox');
	});
});

describe('expanding the control', () => {
	it('reveals the others in plain words, and draws its panel as a surface only while open', async () => {
		const { container } = mount(view({ reading: atrox, continuity: atrox, typed: young }));
		// Collapsed: the control draws no surface of its own beyond the strip.
		expect(surfaces(container)).toEqual([]);
		expect(screen.queryByTestId('mob-source-panel')).toBeNull();

		await fireEvent.click(shown());
		expect(shown().getAttribute('aria-expanded')).toBe('true');
		const panel = screen.getByTestId('mob-source-panel');
		expect(panel.hasAttribute(OVERLAY_SURFACE_ATTRIBUTE)).toBe(true);
		expect(surfaces(container)).toEqual([panel]);
		const lines = screen.getAllByTestId('mob-line');
		expect(lines.map((line) => line.dataset.kind)).toEqual(['reading', 'continuity', 'backstop']);
		expect(panel.textContent).toContain('Reading now');
		expect(panel.textContent).toContain('Recent kills');
		expect(panel.textContent).toContain('Your backstop');
		expect(panel.textContent).toContain('Young Atrox');

		// Closed again: its area is released to the game.
		await fireEvent.click(shown());
		expect(screen.queryByTestId('mob-source-panel')).toBeNull();
		expect(surfaces(container)).toEqual([]);
	});

	it('is keyboard-operable: the toggle is a button, and Escape closes it', async () => {
		const { container } = mount(view({ reading: atrox }));
		const toggle = shown();
		expect(toggle.tagName).toBe('BUTTON');
		expect(toggle.getAttribute('aria-controls')).toBe('mob-source-panel');
		toggle.focus();
		await fireEvent.click(toggle);
		expect(surfaces(container)).toHaveLength(1);
		await fireEvent.keyDown(window, { key: 'Escape' });
		expect(surfaces(container)).toEqual([]);
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
	});

	it('opens on hover and lets go once the pointer leaves', async () => {
		vi.useFakeTimers();
		try {
			const { container } = mount(view({ reading: atrox }));
			const facet = screen.getByTestId('mob-facet');
			await fireEvent.pointerEnter(facet);
			expect(surfaces(container)).toHaveLength(1);
			await fireEvent.pointerLeave(facet);
			vi.advanceTimersByTime(1_000);
			flushSync();
			expect(surfaces(container)).toEqual([]);
		} finally {
			vi.useRealTimers();
		}
	});

	it('edits the backstop and takes a typed creature to pin or keep', async () => {
		const { model, deps } = mount(view({ reading: atrox, typed: young }));
		await fireEvent.click(shown());
		expect(screen.getByPlaceholderText('Type a creature...')).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
		expect(deps.clearTyped).toHaveBeenCalledTimes(1);
		await waitFor(() => expect(model.busy).toBeNull());

		await model.choose({ display: 'Gloomrat', species: 'Gloomrat', maturity: '' });
		flushSync();
		const draft = screen.getByTestId('mob-draft');
		expect(draft.textContent).toContain('Gloomrat');
		await fireEvent.click(screen.getByRole('button', { name: 'Use as backstop' }));
		expect(deps.setTyped).toHaveBeenCalledWith('Gloomrat', '');
		await waitFor(() => expect(model.busy).toBeNull());

		await model.choose({ display: 'Atrox Old', species: 'Atrox', maturity: 'Old' });
		flushSync();
		const pins = screen.getAllByRole('button', { name: 'Pin' });
		await fireEvent.click(pins[pins.length - 1]);
		expect(deps.pin).toHaveBeenCalledWith({ kind: 'typed', species: 'Atrox', maturity: 'Old' });
	});

	it('carries no in-development mark, and draws its panel inside the strip surface', async () => {
		// The strip the control sits in is itself a surface: render inside one.
		const strip = document.createElement('div');
		strip.setAttribute(OVERLAY_SURFACE_ATTRIBUTE, '');
		document.body.appendChild(strip);
		const deps = {
			view: () => view({ reading: atrox }),
			pin: vi.fn(async () => ({})),
			unpin: vi.fn(async () => true),
			setTyped: vi.fn(async () => ({})),
			clearTyped: vi.fn(async () => ({})),
			refresh: vi.fn(async () => {}),
		};
		const model = createMobSourceModel(deps);
		render(MobSourceControl, { target: strip, props: { view: deps.view(), model } });
		await fireEvent.click(shown());
		const panel = screen.getByTestId('mob-source-panel');
		expect(panel.closest(`[${OVERLAY_SURFACE_ATTRIBUTE}]`)).not.toBeNull();
		expect(screen.queryByTestId('in-development-mark')).toBeNull();
		expect(screen.queryByText('In development')).toBeNull();
		// Nothing drawn anywhere but on a marked surface.
		const outside = Array.from(document.body.querySelectorAll('*')).filter(
			(el) =>
				el.children.length === 0 &&
				(el.textContent ?? '').trim() !== '' &&
				el.closest(`[${OVERLAY_SURFACE_ATTRIBUTE}]`) === null,
		);
		expect(outside).toEqual([]);
		strip.remove();
	});

	it('returns focus to the toggle when Escape closes it from inside', async () => {
		mount(view({ reading: atrox, typed: young }));
		await fireEvent.click(shown());
		const input = screen.getByPlaceholderText('Type a creature...');
		input.focus();
		expect(document.activeElement).toBe(input);
		await fireEvent.keyDown(window, { key: 'Escape' });
		expect(screen.queryByTestId('mob-source-panel')).toBeNull();
		expect(document.activeElement).toBe(shown());
	});

	it('keeps open when the toggle is clicked while it is open only under the pointer', async () => {
		vi.useFakeTimers();
		try {
			const { container } = mount(view({ reading: atrox }));
			const facet = screen.getByTestId('mob-facet');
			await fireEvent.pointerEnter(facet);
			await fireEvent.click(shown());
			await fireEvent.pointerLeave(facet);
			vi.advanceTimersByTime(1_000);
			flushSync();
			expect(surfaces(container)).toHaveLength(1);
			// A second click closes it.
			await fireEvent.click(shown());
			expect(surfaces(container)).toEqual([]);
		} finally {
			vi.useRealTimers();
		}
	});

	it('while pinned, offers a typed creature only to pin (replacing the pin), never as a backstop', async () => {
		const { model, deps } = mount(
			view({ readingScreen: false, pin: { creature: atrox, by: 'read' }, typed: young }),
		);
		await fireEvent.click(shown());
		await model.choose({ display: 'Gloomrat', species: 'Gloomrat', maturity: '' });
		flushSync();
		expect(screen.getByTestId('mob-draft')).toBeTruthy();
		expect(screen.queryByRole('button', { name: 'Use as backstop' })).toBeNull();
		await fireEvent.click(screen.getByRole('button', { name: 'Pin' }));
		expect(deps.pin).toHaveBeenCalledWith({ kind: 'typed', species: 'Gloomrat', maturity: null });
	});
});
