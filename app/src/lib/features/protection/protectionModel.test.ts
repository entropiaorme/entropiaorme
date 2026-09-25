import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionOverview, ProtectionSet } from '$lib/api';
import { createProtectionModel } from './protectionModel.svelte';

vi.mock('$lib/api', () => ({
	archiveProtectionSet: vi.fn(),
	createProtectionSet: vi.fn(),
	getProtectionOverview: vi.fn(),
	updateProtectionSet: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

const set: ProtectionSet = {
	id: '7',
	kind: 'armour',
	name: 'Hyperion',
	markupPercent: 120,
	latestObservation: null,
	basisLocked: false,
};

const overview: ProtectionOverview = {
	sets: [set],
	recentCostWindows: [],
	unrecorded: { sessions: 3, hits: 120 },
};

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getProtectionOverview.mockResolvedValue(overview);
});

describe('the limited-set catalogue', () => {
	it('adds a limited set with its markup', async () => {
		mocked.createProtectionSet.mockResolvedValue(overview);
		const model = createProtectionModel();
		model.openSet('plates');
		model.setName = ' 5B plates ';
		model.setMarkup = '135.5';
		await model.saveSet();
		expect(mocked.createProtectionSet).toHaveBeenCalledWith({
			kind: 'plates',
			name: '5B plates',
			markupPercent: 135.5,
		});
		expect(model.setModalOpen).toBe(false);
	});

	it('refuses a markup below 100 percent before asking the backend', async () => {
		const model = createProtectionModel();
		model.openSet('armour');
		model.setName = 'Hyperion';
		model.setMarkup = '95';
		expect(model.setSaveDisabled).toBe(true);
		await model.saveSet();
		expect(mocked.createProtectionSet).not.toHaveBeenCalled();
	});

	it('edits a set by id and keeps the modal open on a refusal', async () => {
		mocked.updateProtectionSet.mockRejectedValue(
			new Error("A set's markup cannot change after its first reading"),
		);
		const model = createProtectionModel();
		await model.load();
		model.editSet(set);
		model.setMarkup = '130';
		await model.saveSet();
		expect(mocked.updateProtectionSet).toHaveBeenCalledWith('7', {
			name: 'Hyperion',
			markupPercent: 130,
		});
		expect(model.setModalOpen).toBe(true);
		expect(model.error).toContain('cannot change');
	});

	it('removes a set through the confirmation', async () => {
		mocked.archiveProtectionSet.mockResolvedValue({ ...overview, sets: [] });
		const model = createProtectionModel();
		await model.load();
		model.askRemoveSet(set);
		expect(model.removalModalOpen).toBe(true);
		await model.confirmRemoval();
		expect(mocked.archiveProtectionSet).toHaveBeenCalledWith('7');
		expect(model.removalModalOpen).toBe(false);
		expect(model.overview.sets).toEqual([]);
	});

	it('carries what is still unrecorded', async () => {
		const model = createProtectionModel();
		await model.load();
		expect(model.overview.unrecorded).toEqual({ sessions: 3, hits: 120 });
	});
});
