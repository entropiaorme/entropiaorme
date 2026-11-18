// @vitest-environment happy-dom

import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { NavDashboard, NavInventory } from '$lib/icons';
import Sidebar from './Sidebar.svelte';

describe('a nav item asking for attention', () => {
	it('carries a mark of its own, apart from the unread one, and says so', () => {
		render(Sidebar, {
			props: {
				items: [
					{ id: '/', label: 'Dashboard', icon: NavDashboard },
					{ id: '/inventory', label: 'Inventory', icon: NavInventory, indicator: 'attention' },
				],
				active: '/',
				onnavigate: vi.fn(),
			},
		});
		const marks = screen.getAllByTestId('nav-attention');
		expect(marks).toHaveLength(1);
		expect(screen.getByRole('button', { name: /Inventory/ }).textContent).toContain(
			'something new',
		);
	});

	it('carries none when nothing asks for it', () => {
		render(Sidebar, {
			props: {
				items: [{ id: '/inventory', label: 'Inventory', icon: NavInventory }],
				active: '/',
				onnavigate: vi.fn(),
			},
		});
		expect(screen.queryByTestId('nav-attention')).toBeNull();
	});
});
