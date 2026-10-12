// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import BreakpointSlider from './BreakpointSlider.svelte';

/** The track's internal resolution: a position runs from 0 to this. */
const RESOLUTION = 10_000;

function slider(props: Record<string, unknown> = {}) {
	const onValue = vi.fn();
	const onMax = vi.fn();
	render(BreakpointSlider, {
		props: {
			label: 'Mobs killed',
			unit: 'mobs',
			value: 10_000,
			max: 1_000_000,
			breakpoint: 10_000,
			onValue,
			onMax,
			...props,
		},
	});
	const track = screen.getByRole('slider', { name: String(props.label ?? 'Mobs killed') });
	return { onValue, onMax, track: track as HTMLInputElement };
}

function slideTo(track: HTMLInputElement, position: number) {
	fireEvent.input(track, { target: { value: String(position) } });
}

describe('BreakpointSlider', () => {
	it('gives the first half of the track to the range below the breakpoint', () => {
		const { onValue, track } = slider();
		// The breakpoint sits at the middle of the track.
		expect(Number(track.value)).toBe(RESOLUTION / 2);
		slideTo(track, RESOLUTION / 4);
		expect(onValue).toHaveBeenLastCalledWith(5000);
		// Past the middle the track covers the breakpoint to the upper end,
		// rounded to two significant figures.
		slideTo(track, (RESOLUTION * 3) / 4);
		expect(onValue).toHaveBeenLastCalledWith(510_000);
		slideTo(track, RESOLUTION);
		expect(onValue).toHaveBeenLastCalledWith(1_000_000);
		slideTo(track, 0);
		expect(onValue).toHaveBeenLastCalledWith(0);
	});

	it('places a value above the breakpoint on the upper half', () => {
		const { track } = slider({ value: 505_000 });
		expect(Number(track.value)).toBeCloseTo((RESOLUTION * 3) / 4, 6);
	});

	it('keeps the track linear when the upper end is not past the breakpoint', () => {
		const { onValue, track } = slider({
			label: 'Mob HP',
			unit: 'HP',
			value: 250,
			max: 500,
			breakpoint: 500,
			significant: 3,
		});
		expect(Number(track.value)).toBe(RESOLUTION / 2);
		slideTo(track, 3333);
		expect(onValue).toHaveBeenLastCalledWith(167);
	});

	it('rounds to a fixed step when given one, and shows a fixed upper end as plain text', () => {
		const { onValue, track } = slider({
			label: 'Looter level',
			unit: '',
			value: 40,
			max: 100,
			breakpoint: undefined,
			step: 0.1,
			decimals: 1,
			fixedMax: true,
		});
		slideTo(track, 3746);
		expect(onValue.mock.lastCall?.[0]).toBeCloseTo(37.5, 9);
		expect(screen.getByRole('button', { name: '40.0' })).toBeTruthy();
		expect(screen.queryByTitle('Change the upper end')).toBeNull();
		expect(screen.getByText('100')).toBeTruthy();
	});

	it('takes an exact value typed in place of the label', async () => {
		const { onValue, onMax } = slider();
		await fireEvent.click(screen.getByRole('button', { name: '10,000 mobs' }));
		const field = screen.getByLabelText('Mobs killed, exact') as HTMLInputElement;
		expect(field.value).toBe('10000');
		await fireEvent.input(field, { target: { value: '25k' } });
		await fireEvent.keyDown(field, { key: 'Enter' });
		expect(onValue).toHaveBeenCalledWith(25_000);
		expect(onMax).not.toHaveBeenCalled();
		expect(screen.queryByLabelText('Mobs killed, exact')).toBeNull();
	});

	it('moves the upper end out to meet a typed value past it', async () => {
		const { onValue, onMax } = slider();
		await fireEvent.click(screen.getByRole('button', { name: '10,000 mobs' }));
		const field = screen.getByLabelText('Mobs killed, exact');
		await fireEvent.input(field, { target: { value: '2m' } });
		await fireEvent.keyDown(field, { key: 'Enter' });
		expect(onMax).toHaveBeenCalledWith(2_000_000);
		expect(onValue).toHaveBeenCalledWith(2_000_000);
	});

	it('drops a typed value that is not an amount, or one escaped', async () => {
		const { onValue } = slider();
		await fireEvent.click(screen.getByRole('button', { name: '10,000 mobs' }));
		let field = screen.getByLabelText('Mobs killed, exact');
		await fireEvent.input(field, { target: { value: 'lots' } });
		await fireEvent.keyDown(field, { key: 'Enter' });
		await fireEvent.click(screen.getByRole('button', { name: '10,000 mobs' }));
		field = screen.getByLabelText('Mobs killed, exact');
		await fireEvent.input(field, { target: { value: '5' } });
		await fireEvent.keyDown(field, { key: 'Escape' });
		expect(screen.queryByLabelText('Mobs killed, exact')).toBeNull();
		expect(onValue).not.toHaveBeenCalled();
	});

	it('edits the upper end in place', async () => {
		const { onMax } = slider();
		await fireEvent.click(screen.getByTitle('Change the upper end'));
		let field = screen.getByLabelText('Mobs killed upper end');
		await fireEvent.input(field, { target: { value: '0' } });
		await fireEvent.keyDown(field, { key: 'Enter' });
		expect(onMax).not.toHaveBeenCalled();
		await fireEvent.click(screen.getByTitle('Change the upper end'));
		field = screen.getByLabelText('Mobs killed upper end');
		await fireEvent.input(field, { target: { value: '5m' } });
		await fireEvent.keyDown(field, { key: 'Enter' });
		expect(onMax).toHaveBeenCalledWith(5_000_000);
	});

	it('slides back to its marker when the marker is clicked', async () => {
		const { onValue } = slider({
			label: 'Looter level',
			unit: '',
			value: 70,
			max: 100,
			breakpoint: undefined,
			step: 0.1,
			decimals: 1,
			fixedMax: true,
			marker: { value: 42.3, label: 'yours' },
		});
		await fireEvent.click(screen.getByRole('button', { name: 'yours' }));
		expect(onValue).toHaveBeenCalledWith(42.3);
	});
});
