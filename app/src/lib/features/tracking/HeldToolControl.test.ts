// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { flushSync } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ToolHeld, ToolSetupView, ToolSourceView } from '$lib/api';
import { createHoverPanel } from '$lib/components/overlay/hoverPanel.svelte';
import { overlaySurfaces } from '$lib/windows/windowSize';
import HeldToolControl from './HeldToolControl.svelte';
import { type HeldToolDisplay, type HeldToolFeedback, heldToolDisplay } from './heldTool';

function display(held: ToolHeld, manual = false): HeldToolDisplay {
	const source: ToolSourceView = { readingScreen: true, notReading: null, manual, held };
	const shown = heldToolDisplay(source);
	if (!shown) throw new Error('the screen is the source');
	return shown;
}

const twins: ToolHeld = {
	kind: 'uncertain',
	why: {
		kind: 'twins',
		entries: [
			{ id: 1, name: 'Pistol', label: 'Pistol + A101' },
			{ id: 2, name: 'Pistol', label: 'Pistol' },
		],
	},
};

const hunting: HeldToolFeedback = {
	text: 'Hunting',
	tone: 'text-white/35',
	detail: 'Held tool: recording as Hunting',
	testId: 'activity-feedback',
};

const boosted: HeldToolFeedback = {
	text: 'Boosted',
	tone: 'text-sky-300/90',
	detail:
		"Recent hits are above this weapon's listed damage. Check which set-up is on in Equipment.",
	testId: 'weapon-hit-cue',
};

const chip: ToolSetupView = {
	per: 'shot',
	parts: [
		{ role: 'amplifier', name: 'Mayhem MF-Amplifier Delta (L)' },
		{ role: 'absorber', name: 'ArMatrix Extender P20 (L)' },
		{ role: 'implant', name: 'NeoPsion 85-B Mindforce Implant (L)' },
	],
	enhancers: null,
	costPerUsePed: 0.128_4,
	ttPerUsePed: 0.036_2,
};

const CHIP = 'Exceptional Mayhem Electric Attack Chip 15, Perfected (L)';

function mount(shown: HeldToolDisplay, feedback: HeldToolFeedback | null = null) {
	const panel = createHoverPanel();
	const onDeclare = vi.fn();
	const onChoose = vi.fn();
	const result = render(HeldToolControl, {
		props: { display: shown, panel, feedback, onDeclare, onChoose },
	});
	return { ...result, panel, onDeclare, onChoose };
}

function label(): HTMLElement {
	return screen.getByTestId('held-tool-label');
}

describe('what the control shows by default', () => {
	it('shows a tool by name in the ordinary tone, with the activity it implies under it', () => {
		mount(display({ kind: 'tool', name: 'Pistol', provisional: false, setup: null }), hunting);
		expect(label().textContent?.trim()).toBe('Pistol');
		expect(label().className).toContain('text-white/70');
		expect(screen.getByTestId('activity-feedback').textContent?.trim()).toBe('Hunting');
	});

	it('puts a standing hit cue ahead of the state marker, and says it in the panel', async () => {
		mount(
			display({ kind: 'notVisible', last: 'Pistol', provisional: false, setup: null }),
			boosted,
		);
		expect(screen.getByTestId('weapon-hit-cue').textContent?.trim()).toBe('Boosted');
		expect(screen.queryByTestId('held-tool-marker')).toBeNull();
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByText(/Check which set-up is on in Equipment\./)).toBeTruthy();
	});

	it('shows a provisional tool in the strip informational tone, covered or not', () => {
		const { unmount } = mount(
			display({ kind: 'tool', name: 'Rusty Rifle', provisional: true, setup: null }),
		);
		expect(label().className).toContain('text-sky-300/90');
		unmount();
		mount(display({ kind: 'notVisible', last: 'Rusty Rifle', provisional: true, setup: null }));
		expect(label().dataset.tone).toBe('provisional');
	});

	it('says the tool is unclear in the attention tone, and nothing more until expanded', () => {
		mount(display(twins));
		expect(label().textContent?.trim()).toBe('Tool unclear');
		expect(label().className).toContain('text-amber-300/90');
		expect(screen.queryByTestId('held-tool-panel')).toBeNull();
	});

	it('marks a covered bar quietly, and a chosen tool', () => {
		const { unmount } = mount(
			display({ kind: 'notVisible', last: 'Pistol', provisional: false, setup: null }),
		);
		expect(screen.getByTestId('held-tool-marker').textContent?.trim()).toBe('Covered');
		expect(screen.getByTestId('held-tool-marker').className).toContain('text-white/35');
		unmount();
		mount(display({ kind: 'tool', name: 'Pistol', provisional: false, setup: null }, true));
		expect(screen.getByTestId('held-tool-marker').textContent?.trim()).toBe('Chosen');
	});

	it('keeps the line under the tool laid out when it has nothing to say, so the strip does not move', () => {
		const { container } = mount(display({ kind: 'nothing' }));
		expect(label().textContent?.trim()).toBe('No tool');
		expect(container.querySelector('.h-3')).not.toBeNull();
	});

	it('has a title true in every state', () => {
		mount(display({ kind: 'tool', name: 'Pistol', provisional: false, setup: null }, true));
		expect(screen.getByTestId('held-tool').getAttribute('title')).toBe('The tool in hand');
	});
});

describe('expanding it', () => {
	it('says why the tool is unclear and offers what settles it, drawing its panel only while open', async () => {
		const { container, onDeclare, onChoose } = mount(display(twins));
		expect(overlaySurfaces(container)).toHaveLength(0);
		await fireEvent.click(screen.getByTestId('held-tool'));
		const panel = screen.getByTestId('held-tool-panel');
		expect(overlaySurfaces(container)).toEqual([panel]);
		expect(
			screen.getByText('Two entries look the same on the bar: Pistol + A101 and Pistol.'),
		).toBeTruthy();

		await fireEvent.click(screen.getByRole('button', { name: 'Use Pistol + A101' }));
		expect(onDeclare).toHaveBeenCalledWith(1);
		// Declaring is done with the panel.
		expect(screen.queryByTestId('held-tool-panel')).toBeNull();

		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByRole('button', { name: 'Use Pistol' })).toBeTruthy();
		await fireEvent.click(screen.getByRole('button', { name: 'Choose a tool' }));
		expect(onChoose).toHaveBeenCalledTimes(1);
		expect(onChoose.mock.calls[0][0]).toBeInstanceOf(HTMLElement);
	});

	it('offers the picker over a covered bar', async () => {
		mount(display({ kind: 'notVisible', last: 'Pistol', provisional: false, setup: null }));
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByText('The tool bar is covered.')).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Choose a tool' })).toBeTruthy();
	});

	it('carries no in-development mark inside the panel: reading the held tool is not in development', async () => {
		mount(display({ kind: 'nothing' }));
		await fireEvent.click(screen.getByTestId('held-tool'));
		const panel = screen.getByTestId('held-tool-panel');
		expect(panel.querySelector('[data-testid="in-development-mark"]')).toBeNull();
		expect(screen.queryByText('In development')).toBeNull();
	});

	it('is keyboard-operable: the toggle is a button, and Escape closes it', async () => {
		mount(display(twins));
		const toggle = screen.getByTestId('held-tool');
		expect(toggle.tagName).toBe('BUTTON');
		await fireEvent.click(toggle);
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		screen.getByRole('button', { name: 'Use Pistol' }).focus();
		await fireEvent.keyDown(window, { key: 'Escape' });
		flushSync();
		expect(screen.queryByTestId('held-tool-panel')).toBeNull();
		expect(document.activeElement).toBe(toggle);
	});

	it('opens on hover and lets go once the pointer leaves', async () => {
		vi.useFakeTimers();
		try {
			mount(display(twins));
			const facet = screen.getByTestId('held-tool-facet');
			await fireEvent.pointerEnter(facet);
			expect(screen.getByTestId('held-tool-panel')).toBeTruthy();
			await fireEvent.pointerLeave(facet);
			vi.advanceTimersByTime(300);
			flushSync();
			expect(screen.queryByTestId('held-tool-panel')).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});
});

describe('the set-up of the tool in hand', () => {
	it('marks each attachment fitted beside the name, said for the marks', () => {
		mount(display({ kind: 'tool', name: CHIP, provisional: false, setup: chip }));
		const marks = screen.getByTestId('held-tool-fitted');
		expect(marks.dataset.count).toBe('3');
		expect(marks.children).toHaveLength(3);
		expect(marks.getAttribute('aria-hidden')).toBe('true');
		expect(screen.getByText(', 3 attachments').className).toContain('sr-only');
	});

	it('marks nothing on a bare tool, or where the parts are not known', () => {
		const { unmount } = mount(
			display({
				kind: 'tool',
				name: 'Rusty Rifle',
				provisional: true,
				setup: { ...chip, parts: [] },
			}),
		);
		expect(screen.queryByTestId('held-tool-fitted')).toBeNull();
		unmount();
		mount(
			display({ kind: 'tool', name: 'FAP', provisional: false, setup: { ...chip, parts: null } }),
		);
		expect(screen.queryByTestId('held-tool-fitted')).toBeNull();
	});

	it('dims the marks with the name while the bar is covered', () => {
		mount(display({ kind: 'notVisible', last: CHIP, provisional: false, setup: chip }));
		expect(screen.getByTestId('held-tool-fitted').className).toContain('text-white/25');
	});

	it('names the tool in full, each part, and the price of a shot with its TT when expanded', async () => {
		mount(display({ kind: 'tool', name: CHIP, provisional: false, setup: chip }));
		expect(screen.queryByTestId('held-tool-setup')).toBeNull();
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByTestId('held-tool-name').textContent?.trim()).toBe(CHIP);
		const setup = screen.getByTestId('held-tool-setup');
		expect([...setup.querySelectorAll('dt')].map((term) => term.textContent?.trim())).toEqual([
			'Amplifier',
			'Absorber',
			'Implant',
		]);
		expect([...setup.querySelectorAll('dd')].map((value) => value.textContent?.trim())).toEqual([
			'Mayhem MF-Amplifier Delta (L)',
			'ArMatrix Extender P20 (L)',
			'NeoPsion 85-B Mindforce Implant (L)',
		]);
		const cost = screen.getByTestId('held-tool-cost').textContent?.replace(/\s+/g, ' ').trim();
		expect(cost).toBe('12.84 PEC per shot 3.62 PEC at TT');
	});

	it('gives a bare tool its price under the line saying how it is costed, with no parts listed', async () => {
		mount(
			display({
				kind: 'tool',
				name: 'Rusty Rifle',
				provisional: true,
				setup: { ...chip, parts: [], costPerUsePed: 0.021, ttPerUsePed: null },
			}),
		);
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.getByTestId('held-tool-name').className).toContain('text-sky-300/90');
		expect(screen.getByText('Not in your equipment: costed without attachments.')).toBeTruthy();
		expect(screen.queryByTestId('held-tool-setup')).toBeNull();
		expect(screen.getByTestId('held-tool-cost').textContent?.replace(/\s+/g, ' ').trim()).toBe(
			'2.10 PEC per shot',
		);
	});

	it('has no set-up to show while the tool is unclear', async () => {
		mount(display(twins));
		await fireEvent.click(screen.getByTestId('held-tool'));
		expect(screen.queryByTestId('held-tool-name')).toBeNull();
		expect(screen.queryByTestId('held-tool-cost')).toBeNull();
	});
});
