// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import PickerInput from './PickerInput.svelte';

type Item = { id: string; name: string };

// The presenter is decoupled from any typeahead factory: it accepts anything
// matching the structural model, so the tests drive it with a plain object.
type FakeModel = {
	query: string;
	results: Item[];
	selected: Item | null;
	loading: boolean;
	error: string | null;
	settledQuery?: string | null;
	select: ReturnType<typeof vi.fn<(item: Item) => void>>;
	clear: ReturnType<typeof vi.fn<() => void>>;
};

function fakeModel(overrides: Partial<FakeModel> = {}): FakeModel {
	return {
		query: 'atr',
		results: [],
		selected: null,
		loading: false,
		error: null,
		select: vi.fn<(item: Item) => void>(),
		clear: vi.fn<() => void>(),
		...overrides,
	};
}

// The test render callsite cannot infer the component's generic from props,
// so the snippets are typed against `unknown` and narrow to Item inside.
const resultSnippet = createRawSnippet((args: () => { item: unknown; query: string }) => ({
	render: () => `<span data-query="${args().query}">${(args().item as Item).name}</span>`,
}));

const selectionSnippet = createRawSnippet((args: () => { item: unknown }) => ({
	render: () => `<span>Selected: ${(args().item as Item).name}</span>`,
}));

const extraRowSnippet = createRawSnippet(() => ({
	render: () => '<span>Add custom entry</span>',
}));

const twoResults: Item[] = [
	{ id: 'a', name: 'Atrox Young' },
	{ id: 'b', name: 'Atrax Mature' },
];

function renderPicker(model: ReturnType<typeof fakeModel>, extra: Record<string, unknown> = {}) {
	return render(PickerInput, {
		props: {
			id: 'mob-picker',
			placeholder: 'Search mobs…',
			model,
			result: resultSnippet,
			selection: selectionSnippet,
			...extra,
		},
	});
}

describe('combobox semantics', () => {
	it('marks the input as a collapsed combobox when there are no results', () => {
		renderPicker(fakeModel());

		const input = screen.getByRole('combobox');
		expect(input.getAttribute('aria-expanded')).toBe('false');
		expect(input.getAttribute('aria-autocomplete')).toBe('list');
		expect(screen.queryByRole('listbox')).toBeNull();
	});

	it('expands over a listbox of options wired through aria-controls', () => {
		renderPicker(fakeModel({ results: twoResults }));

		const input = screen.getByRole('combobox');
		const listbox = screen.getByRole('listbox');
		expect(input.getAttribute('aria-expanded')).toBe('true');
		expect(input.getAttribute('aria-controls')).toBe(listbox.id);
		expect(screen.getAllByRole('option').map((el) => el.textContent?.trim())).toEqual([
			'Atrox Young',
			'Atrax Mature',
		]);
	});

	it('shows the model error text', () => {
		renderPicker(fakeModel({ error: 'Search failed' }));
		expect(screen.getByText('Search failed')).toBeTruthy();
	});
});

describe('keyboard interaction', () => {
	it('moves the highlight with the arrows and reflects it in ARIA', async () => {
		renderPicker(fakeModel({ results: twoResults }));
		const input = screen.getByRole('combobox');

		expect(input.getAttribute('aria-activedescendant')).toBeNull();

		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		expect(input.getAttribute('aria-activedescendant')).toBe('mob-picker-option-0');
		expect(screen.getAllByRole('option')[0].getAttribute('aria-selected')).toBe('true');

		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		expect(input.getAttribute('aria-activedescendant')).toBe('mob-picker-option-1');

		// Wraps back to the top, and ArrowUp walks the other way.
		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		expect(input.getAttribute('aria-activedescendant')).toBe('mob-picker-option-0');
		await fireEvent.keyDown(input, { key: 'ArrowUp' });
		expect(input.getAttribute('aria-activedescendant')).toBe('mob-picker-option-1');
	});

	it('selects the highlighted result on Enter', async () => {
		const model = fakeModel({ results: twoResults });
		renderPicker(model);
		const input = screen.getByRole('combobox');

		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		await fireEvent.keyDown(input, { key: 'Enter' });
		expect(model.select).toHaveBeenCalledWith(twoResults[1]);
	});

	it('selects the first result on Enter without a highlight', async () => {
		const model = fakeModel({ results: twoResults });
		renderPicker(model);

		await fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' });
		expect(model.select).toHaveBeenCalledWith(twoResults[0]);
	});

	it('dismisses the dropdown on Escape', async () => {
		renderPicker(fakeModel({ results: twoResults }));
		const input = screen.getByRole('combobox');

		await fireEvent.keyDown(input, { key: 'Escape' });
		expect(screen.queryByRole('listbox')).toBeNull();
		expect(input.getAttribute('aria-expanded')).toBe('false');
	});

	it('keeps the Escape that dismisses the dropdown from reaching the window', async () => {
		renderPicker(fakeModel({ results: twoResults }));
		const onWindowKeydown = vi.fn();
		window.addEventListener('keydown', onWindowKeydown);
		try {
			await fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
		} finally {
			window.removeEventListener('keydown', onWindowKeydown);
		}
		expect(onWindowKeydown).not.toHaveBeenCalled();
	});
});

describe('selection and rows', () => {
	it('selects a result on click', async () => {
		const model = fakeModel({ results: twoResults });
		renderPicker(model);

		await fireEvent.click(screen.getAllByRole('option')[1]);
		expect(model.select).toHaveBeenCalledWith(twoResults[1]);
	});

	it('renders the selected chip instead of the dropdown', () => {
		renderPicker(fakeModel({ results: twoResults, selected: twoResults[0] }));

		expect(screen.getByText('Selected: Atrox Young')).toBeTruthy();
		expect(screen.queryByRole('listbox')).toBeNull();
	});

	it('hands each row the query it answers, for highlighting', () => {
		renderPicker(fakeModel({ query: 'atr', results: twoResults }));

		const rows = screen.getAllByRole('option');
		expect(rows[0].querySelector('[data-query]')?.getAttribute('data-query')).toBe('atr');
	});

	it('says so when a settled search found nothing', () => {
		renderPicker(fakeModel({ query: 'zeta ', results: [], settledQuery: 'zeta' }));

		expect(screen.getByRole('listbox').textContent).toContain('No matches for “zeta”');
	});

	it('stays quiet while a search for the current query is still pending', () => {
		renderPicker(fakeModel({ query: 'zetas', results: [], settledQuery: 'zeta' }));
		expect(screen.queryByRole('listbox')).toBeNull();

		renderPicker(fakeModel({ query: 'zeta', results: [], settledQuery: 'zeta', loading: true }));
		expect(screen.queryByRole('listbox')).toBeNull();
	});

	it('keeps the dropdown open for the extra row when there are no results', () => {
		renderPicker(fakeModel({ results: [] }), { extraRow: extraRowSnippet });

		expect(screen.getByRole('listbox')).toBeTruthy();
		expect(screen.getByText('Add custom entry')).toBeTruthy();
	});
});
