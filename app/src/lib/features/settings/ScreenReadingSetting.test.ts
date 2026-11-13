// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import ScreenReadingSetting from './ScreenReadingSetting.svelte';
import { SCREEN_READINGS } from './screenReadingSetting';

const heldTool = SCREEN_READINGS[1];

function props(overrides: Partial<Record<string, unknown>> = {}) {
	return {
		reading: heldTool,
		enabled: false,
		capture: 'notRunning' as const,
		disabled: false,
		saved: false,
		onchange: vi.fn(),
		...overrides,
	};
}

describe('a screen-reading setting', () => {
	it('shows its title, its copy and the in-development mark', () => {
		render(ScreenReadingSetting, { props: props() });
		expect(screen.getByText('Read the held tool from the screen')).toBeTruthy();
		expect(screen.getByText(/Hotbar keys still start consumables\./)).toBeTruthy();
		expect(screen.getByText(/in development/i)).toBeTruthy();
	});

	it('says nothing about capture while it is off', () => {
		render(ScreenReadingSetting, { props: props({ enabled: false }) });
		expect(screen.queryByText(/Screen capture is not running/)).toBeNull();
	});

	it('says why it cannot watch once it is on and capture cannot run', () => {
		render(ScreenReadingSetting, { props: props({ enabled: true }) });
		expect(screen.getByText(/Screen capture is not running/)).toBeTruthy();
	});

	it('hands the new state to its owner', async () => {
		const onchange = vi.fn();
		render(ScreenReadingSetting, { props: props({ onchange }) });
		await fireEvent.click(screen.getByRole('switch', { name: heldTool.title }));
		expect(onchange).toHaveBeenCalledWith(true);
	});
});
