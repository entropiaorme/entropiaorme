// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import SearchInput from './SearchInput.svelte';

describe('SearchInput', () => {
	it('empties a filled box on Escape without letting the key reach the window', async () => {
		render(SearchInput, { props: { value: 'opalo' } });
		const input = screen.getByRole('textbox') as HTMLInputElement;
		const onWindowKeydown = vi.fn();
		window.addEventListener('keydown', onWindowKeydown);
		try {
			await fireEvent.keyDown(input, { key: 'Escape' });
		} finally {
			window.removeEventListener('keydown', onWindowKeydown);
		}
		expect(input.value).toBe('');
		expect(onWindowKeydown).not.toHaveBeenCalled();
	});

	it('lets Escape through when the box is already empty', async () => {
		render(SearchInput, { props: { value: '' } });
		const onWindowKeydown = vi.fn();
		window.addEventListener('keydown', onWindowKeydown);
		try {
			await fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
		} finally {
			window.removeEventListener('keydown', onWindowKeydown);
		}
		expect(onWindowKeydown).toHaveBeenCalled();
	});

	it('leaves the text alone when its owner handles Escape itself', async () => {
		const onkeydown = vi.fn((e: KeyboardEvent) => e.preventDefault());
		render(SearchInput, { props: { value: 'opalo', onkeydown } });
		const input = screen.getByRole('textbox') as HTMLInputElement;

		await fireEvent.keyDown(input, { key: 'Escape' });
		expect(onkeydown).toHaveBeenCalled();
		expect(input.value).toBe('opalo');
	});

	it('clears from the button and keeps focus in the box', async () => {
		render(SearchInput, { props: { value: 'opalo' } });
		const input = screen.getByRole('textbox') as HTMLInputElement;

		await fireEvent.click(screen.getByRole('button', { name: 'Clear search' }));
		expect(input.value).toBe('');
		expect(document.activeElement).toBe(input);
	});
});
