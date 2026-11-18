// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import SegmentedControl from './SegmentedControl.svelte';

describe('an option asking for attention', () => {
	it('carries a count beside its label, and says so to assistive technology', () => {
		render(SegmentedControl, {
			props: {
				options: [
					{ id: 'account', label: 'From your account', attention: 3 },
					{ id: 'yours', label: 'Your entries' },
				],
				active: 'yours',
				onchange: vi.fn(),
			},
		});
		const marks = screen.getAllByTestId('segment-attention');
		expect(marks).toHaveLength(1);
		expect(marks[0].textContent).toBe('3');
		expect(
			screen.getByRole('button', { name: 'From your account, 3 new or waiting' }),
		).toBeTruthy();
		expect(screen.getByRole('button', { name: 'Your entries' })).toBeTruthy();
	});

	it('shows nothing at zero', () => {
		render(SegmentedControl, {
			props: {
				options: [{ id: 'account', label: 'From your account', attention: 0 }],
				active: 'account',
				onchange: vi.fn(),
			},
		});
		expect(screen.queryByTestId('segment-attention')).toBeNull();
		expect(screen.getByRole('button', { name: 'From your account' })).toBeTruthy();
	});
});
