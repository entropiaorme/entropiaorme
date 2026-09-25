// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import DefinitionAuthoring from './DefinitionAuthoring.svelte';
import { createDefinitionsModel, type DefinitionsModelDeps } from './definitionsModel.svelte';

vi.mock('$lib/motion/testMotion', () => ({ shouldSettleInstantly: () => true }));

function definition(overrides: Partial<SessionDefinition> = {}): SessionDefinition {
	return {
		id: '2',
		name: 'Easter Mayhem 2026',
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: false,
		isActive: true,
		instanceCount: 35,
		createdAt: 1,
		updatedAt: null,
		roster: [],
		...overrides,
	};
}

function modelFor(entry: SessionDefinition) {
	const deps: DefinitionsModelDeps = {
		listDefinitions: vi.fn(async () => [entry]),
		createDefinition: vi.fn(async () => entry),
		updateDefinition: vi.fn(async () => entry),
		archiveDefinition: vi.fn(async () => ({ ...entry, isActive: false })),
		selectDefinition: vi.fn(async () => ({})),
		refreshTracking: vi.fn(async () => ({})),
		listFamilies: vi.fn(async () => []),
		listQuests: vi.fn(async () => []),
	};
	const model = createDefinitionsModel(deps);
	model.openEdit(entry);
	return { model, deps };
}

describe('DefinitionAuthoring lifecycle', () => {
	it('makes Archive deliberate, explains preservation, and lets Escape disarm it', async () => {
		const { model, deps } = modelFor(definition());
		render(DefinitionAuthoring, { props: { model } });

		await fireEvent.click(screen.getByRole('button', { name: 'Archive' }));
		expect(
			screen.getByText(
				'Remove from play choices? History and activities stay intact. Restore from the session review menu.',
			),
		).toBeTruthy();
		expect(deps.archiveDefinition).not.toHaveBeenCalled();

		await fireEvent.keyDown(window, { key: 'Escape' });
		expect(
			screen.queryByText(
				'Remove from play choices? History and activities stay intact. Restore from the session review menu.',
			),
		).toBeNull();
		expect(model.mode).toBe('edit');
	});

	it('archives only on confirmation and refreshes the play state', async () => {
		const { model, deps } = modelFor(definition());
		render(DefinitionAuthoring, { props: { model } });

		await fireEvent.click(screen.getByRole('button', { name: 'Archive' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Archive' }));

		await vi.waitFor(() => {
			expect(deps.archiveDefinition).toHaveBeenCalledWith('2');
			expect(deps.refreshTracking).toHaveBeenCalledTimes(1);
		});
		expect(model.mode).toBe('closed');
	});

	it('never offers Archive for the protected default', () => {
		const { model } = modelFor(
			definition({ id: '1', name: 'Default Tracking', isProtected: true, instanceCount: 0 }),
		);
		render(DefinitionAuthoring, { props: { model } });

		expect(screen.queryByRole('button', { name: 'Archive' })).toBeNull();
	});

	it('offers one armour-cost switch and no per-segment attribution', () => {
		const { model } = modelFor(definition({ trackProtectionCosts: false }));
		render(DefinitionAuthoring, { props: { model } });

		expect(screen.getByRole('switch', { name: 'Track armour costs' })).toBeTruthy();
		expect(screen.queryByText('Armour costs by segment')).toBeNull();
	});
});
