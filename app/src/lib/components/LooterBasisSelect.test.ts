// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { looterBasis, setLooterBasis } from '$lib/looterBasis.svelte';
import LooterBasisSelect from './LooterBasisSelect.svelte';

const levels = { animal: 45, mutant: 38.1, robot: 40.5 };

beforeEach(async () => {
	await setLooterBasis('three_looter_mean');
});

describe('LooterBasisSelect', () => {
	it('names the current basis and its level on the trigger', () => {
		render(LooterBasisSelect, { props: { levels } });
		const trigger = screen.getByTestId('looter-basis-trigger');
		expect(trigger.textContent?.trim()).toBe('Looter mean 41.2');
		expect(trigger.getAttribute('aria-label')).toBe('Looter basis: Looter mean 41.2');
		expect(trigger.getAttribute('aria-haspopup')).toBe('menu');
	});

	it('lists the four bases as radio items, and is keyboard operable', async () => {
		render(LooterBasisSelect, { props: { levels } });
		const trigger = screen.getByTestId('looter-basis-trigger');
		await fireEvent.keyDown(trigger, { key: 'ArrowDown' });
		await tick();
		const options = screen.getAllByRole('menuitemradio');
		expect(options).toHaveLength(4);
		expect(options.map((option) => option.getAttribute('aria-checked'))).toEqual([
			'true',
			'false',
			'false',
			'false',
		]);
		expect(document.activeElement).toBe(options[0]);
		await fireEvent.keyDown(options[0], { key: 'ArrowDown' });
		expect(document.activeElement).toBe(options[1]);
		await fireEvent.keyDown(options[1], { key: 'End' });
		expect(document.activeElement).toBe(options[3]);
		await fireEvent.click(options[3]);
		expect(looterBasis.current).toBe('robot');
		expect(screen.queryAllByRole('menuitemradio')).toHaveLength(0);
		expect(trigger.textContent?.trim()).toBe('Robot Looter 40.5');
	});

	it('closes on Escape without changing the basis', async () => {
		render(LooterBasisSelect, { props: { levels } });
		await fireEvent.click(screen.getByTestId('looter-basis-trigger'));
		const options = screen.getAllByRole('menuitemradio');
		await fireEvent.keyDown(options[0], { key: 'Escape' });
		expect(screen.queryAllByRole('menuitemradio')).toHaveLength(0);
		expect(looterBasis.current).toBe('three_looter_mean');
	});

	it('follows a basis its caller owns instead of the app-wide one', async () => {
		const onchange = vi.fn();
		render(LooterBasisSelect, { props: { levels, basis: 'mutant', onchange } });
		expect(screen.getByTestId('looter-basis-trigger').textContent?.trim()).toBe(
			'Mutant Looter 38.1',
		);
		await fireEvent.click(screen.getByTestId('looter-basis-trigger'));
		await fireEvent.click(screen.getByRole('menuitemradio', { name: /Animal Looter/ }));
		expect(onchange).toHaveBeenCalledWith('animal');
		expect(looterBasis.current).toBe('three_looter_mean');
	});
});
