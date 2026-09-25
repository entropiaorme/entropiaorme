// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import DefinitionPicker from './DefinitionPicker.svelte';
import { createDefinitionsModel, type DefinitionsModelDeps } from './definitionsModel.svelte';

function definition(id: string, name: string): SessionDefinition {
	return {
		id,
		name,
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: false,
		isActive: true,
		instanceCount: 0,
		createdAt: 1000,
		updatedAt: null,
		roster: [],
	};
}

function makeDeps(definitions: SessionDefinition[]): DefinitionsModelDeps {
	return {
		listDefinitions: vi.fn(async () => definitions),
		createDefinition: vi.fn(async () => definitions[0]),
		updateDefinition: vi.fn(async () => definitions[0]),
		archiveDefinition: vi.fn(async () => definitions[0]),
		selectDefinition: vi.fn(async () => ({})),
		refreshTracking: vi.fn(async () => ({})),
		listFamilies: vi.fn(async () => []),
		listQuests: vi.fn(async () => []),
	};
}

async function mount(definitions: SessionDefinition[], selectedId: string | null) {
	const deps = makeDeps(definitions);
	const model = createDefinitionsModel(deps);
	await model.loadDefinitions();
	const onOpenAuthoring = vi.fn();
	render(DefinitionPicker, { props: { model, selectedId, onOpenAuthoring } });
	return { deps, model, onOpenAuthoring };
}

describe('DefinitionPicker', () => {
	it('titles the island with the selected session and offers the switch', async () => {
		await mount([definition('1', 'ARIS Dailies'), definition('2', 'General Hunting')], '1');

		expect(screen.getByText('Session:')).toBeTruthy();
		const trigger = screen.getByLabelText('Switch session (currently ARIS Dailies)');
		expect(trigger.getAttribute('aria-haspopup')).toBe('menu');
		expect(screen.queryByRole('menu')).toBeNull();

		await fireEvent.click(trigger);
		const menu = screen.getByRole('menu');
		expect(menu).toBeTruthy();
		expect(menu.parentElement).toBe(document.body);
		expect(menu.className).toContain('fixed');
		expect(screen.getByText('General Hunting')).toBeTruthy();
		expect(document.activeElement).toBe(screen.getByLabelText('Filter sessions'));
	});

	it('writes the selection when another session is picked', async () => {
		const { deps } = await mount(
			[definition('1', 'ARIS Dailies'), definition('2', 'General Hunting')],
			'1',
		);

		await fireEvent.click(screen.getByLabelText('Switch session (currently ARIS Dailies)'));
		await fireEvent.click(screen.getByText('General Hunting'));

		expect(deps.selectDefinition).toHaveBeenCalledWith(2);
	});

	it('offers only the create control until a session is authored', async () => {
		const { onOpenAuthoring } = await mount([], null);

		expect(screen.getByText('Session:')).toBeTruthy();
		expect(screen.queryByTitle('Switch session')).toBeNull();

		await fireEvent.click(screen.getByTitle('Create a session'));
		expect(onOpenAuthoring).toHaveBeenCalledWith(null);
	});

	it('edits a session in place from its row', async () => {
		const { onOpenAuthoring } = await mount([definition('1', 'ARIS Dailies')], '1');

		await fireEvent.click(screen.getByLabelText('Switch session (currently ARIS Dailies)'));
		await fireEvent.click(screen.getByRole('menuitem', { name: 'Edit current' }));

		expect(onOpenAuthoring).toHaveBeenCalledWith('1');
	});

	it('filters a long alphabetical catalogue without moving its fixed controls', async () => {
		await mount(
			[
				definition('4', 'Tree Cutting'),
				definition('1', 'ARIS Dailies'),
				definition('3', 'Cyrene Dailies'),
				definition('2', 'Bank Robber Skilling'),
			],
			'1',
		);
		await fireEvent.click(screen.getByLabelText('Switch session (currently ARIS Dailies)'));

		const results = screen.getByTestId('definition-results');
		expect(results.className).toContain('overflow-y-auto');
		expect(
			Array.from(results.querySelectorAll('[role="menuitem"]')).map((entry) =>
				entry.textContent?.trim(),
			),
		).toEqual(['ARIS Dailies', 'Bank Robber Skilling', 'Cyrene Dailies', 'Tree Cutting']);

		const input = screen.getByLabelText('Filter sessions');
		await fireEvent.input(input, { target: { value: 'tree' } });
		expect(screen.getByRole('menuitem', { name: 'Tree Cutting' })).toBeTruthy();
		expect(screen.queryByRole('menuitem', { name: 'ARIS Dailies' })).toBeNull();
		expect(screen.getByRole('menuitem', { name: 'Edit current' })).toBeTruthy();
		expect(screen.getByRole('menuitem', { name: '+ New session' })).toBeTruthy();
	});

	it('moves from search into results by keyboard and returns focus on Escape', async () => {
		await mount([definition('1', 'ARIS Dailies'), definition('2', 'Tree Cutting')], '1');
		const trigger = screen.getByLabelText('Switch session (currently ARIS Dailies)');
		await fireEvent.click(trigger);
		const input = screen.getByLabelText('Filter sessions');

		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'ARIS Dailies' }));
		await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
		expect(screen.queryByRole('menu')).toBeNull();
		expect(document.activeElement).toBe(trigger);
	});

	it('clears a stale filter when ArrowDown reopens the catalogue', async () => {
		await mount([definition('1', 'ARIS Dailies'), definition('2', 'Tree Cutting')], '1');
		const trigger = screen.getByLabelText('Switch session (currently ARIS Dailies)');
		await fireEvent.click(trigger);
		const input = screen.getByLabelText('Filter sessions');
		await fireEvent.input(input, { target: { value: 'tree' } });
		await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });

		await fireEvent.keyDown(trigger, { key: 'ArrowDown' });

		expect(screen.getByLabelText('Filter sessions')).toHaveProperty('value', '');
		expect(screen.getByRole('menuitem', { name: 'ARIS Dailies' })).toBeTruthy();
	});

	// The selection is a configuration facet, so an unselected picker is a
	// real state (a session cleared, or one archived since): it invites the
	// choice rather than rendering a stale name.
	it('invites a choice when nothing is selected', async () => {
		await mount([definition('1', 'ARIS Dailies')], null);

		expect(screen.getByLabelText('Choose a session')).toBeTruthy();
		await fireEvent.click(screen.getByLabelText('Choose a session'));
		expect(screen.queryByText('None')).toBeNull();
		const input = screen.getByLabelText('Filter sessions');
		await fireEvent.input(input, { target: { value: 'no match' } });
		await fireEvent.keyDown(input, { key: 'ArrowDown' });
		expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: '+ New session' }));
	});
});
