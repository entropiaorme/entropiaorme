// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import HotbarPresetSwitcher from './HotbarPresetSwitcher.svelte';

describe('the hotbar preset switch', () => {
	it('offers the three presets as one group, with the live one pressed', () => {
		render(HotbarPresetSwitcher, { active: 2, onchange: vi.fn() });
		const group = screen.getByRole('group', { name: 'Hotbar preset' });
		const keys = Array.from(group.querySelectorAll('button'));
		expect(keys.map((key) => key.textContent?.trim())).toEqual(['1', '2', '3']);
		expect(keys.map((key) => key.getAttribute('aria-pressed'))).toEqual(['false', 'true', 'false']);
		expect(screen.getByRole('button', { name: 'Hotbar preset 2' }).className).toContain(
			'text-accent',
		);
		expect(screen.getByRole('button', { name: 'Hotbar preset 1' }).className).not.toContain(
			'text-accent',
		);
	});

	it('reports the preset chosen as its number', async () => {
		const onchange = vi.fn();
		render(HotbarPresetSwitcher, { active: 1, onchange });
		await fireEvent.click(screen.getByRole('button', { name: 'Hotbar preset 3' }));
		expect(onchange).toHaveBeenCalledWith(3);
	});

	it('lights the key that becomes live', async () => {
		const { rerender } = render(HotbarPresetSwitcher, { active: 1, onchange: vi.fn() });
		await rerender({ active: 3, onchange: vi.fn() });
		expect(
			screen.getByRole('button', { name: 'Hotbar preset 3' }).getAttribute('aria-pressed'),
		).toBe('true');
		expect(
			screen.getByRole('button', { name: 'Hotbar preset 1' }).getAttribute('aria-pressed'),
		).toBe('false');
	});
});
