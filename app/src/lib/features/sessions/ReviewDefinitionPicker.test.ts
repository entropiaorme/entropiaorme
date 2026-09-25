// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import { createInstancesModel } from './instancesModel.svelte';
import ReviewDefinitionPicker from './ReviewDefinitionPicker.svelte';
import { createReviewModel } from './reviewModel.svelte';

vi.mock('$lib/api', () => ({
	getTrackingSessions: vi.fn(async () => ({ sessions: [], nextCursor: null, total: 0 })),
	getSessionDetail: vi.fn(),
	deleteSession: vi.fn(),
	reassignSession: vi.fn(),
	getAllSessionDefinitions: vi.fn(),
	restoreSessionDefinition: vi.fn(),
}));

function definition(
	id: string,
	name: string,
	overrides: Partial<SessionDefinition> = {},
): SessionDefinition {
	return {
		id,
		name,
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: false,
		isActive: true,
		instanceCount: 0,
		createdAt: Number(id),
		updatedAt: null,
		roster: [],
		...overrides,
	};
}

async function mount(definitions: SessionDefinition[]) {
	const model = createReviewModel({
		listAllDefinitions: vi.fn(async () => definitions),
		restoreDefinition: vi.fn(async (id) => {
			const found = definitions.find((entry) => entry.id === id);
			if (!found) throw new Error('missing');
			return { ...found, isActive: true };
		}),
		refreshPlayableDefinitions: vi.fn(async () => {}),
		createInstances: (definitionId) => createInstancesModel({ definitionId }),
	});
	await model.openReview('1');
	render(ReviewDefinitionPicker, { props: { model } });
	return model;
}

beforeEach(() => vi.clearAllMocks());

describe('ReviewDefinitionPicker', () => {
	it('keeps active and archived definitions searchable in one top-layer catalogue', async () => {
		await mount([
			definition('1', 'Default Tracking', { isProtected: true }),
			definition('2', 'Tree Cutting'),
			definition('3', 'Easter Mayhem 2026', { isActive: false, instanceCount: 35 }),
			definition('4', 'Archived Empty', { isActive: false }),
		]);

		await fireEvent.click(
			screen.getByLabelText('Review another session (currently Default Tracking)'),
		);
		const menu = screen.getByRole('menu');
		expect(menu.parentElement).toBe(document.body);
		expect(screen.getByText('Active')).toBeTruthy();
		expect(screen.getByText('Archived')).toBeTruthy();
		expect(screen.getByRole('menuitem', { name: 'Archived Empty 0' })).toBeTruthy();

		const input = screen.getByLabelText('Filter review sessions');
		expect(document.activeElement).toBe(input);
		await fireEvent.input(input, { target: { value: 'mayhem' } });
		expect(screen.getByRole('menuitem', { name: 'Easter Mayhem 2026 35' })).toBeTruthy();
		expect(screen.queryByRole('menuitem', { name: 'Default Tracking 0' })).toBeNull();
	});

	it('moves from the filter to the first visible result with ArrowDown', async () => {
		await mount([
			definition('1', 'Default Tracking', { isProtected: true }),
			definition('2', 'Tree Cutting'),
		]);
		await fireEvent.click(
			screen.getByLabelText('Review another session (currently Default Tracking)'),
		);
		const input = screen.getByLabelText('Filter review sessions');
		await fireEvent.input(input, { target: { value: 'tree' } });
		await fireEvent.keyDown(input, { key: 'ArrowDown' });

		expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Tree Cutting 0' }));
	});

	it('switches directly to an archived definition and closes the catalogue', async () => {
		const model = await mount([
			definition('1', 'Default Tracking', { isProtected: true }),
			definition('3', 'Easter Mayhem 2026', { isActive: false, instanceCount: 35 }),
		]);
		await fireEvent.click(
			screen.getByLabelText('Review another session (currently Default Tracking)'),
		);

		await fireEvent.click(screen.getByRole('menuitem', { name: 'Easter Mayhem 2026 35' }));

		expect(model.definitionId).toBe('3');
		expect(screen.queryByRole('menu')).toBeNull();
	});
});
