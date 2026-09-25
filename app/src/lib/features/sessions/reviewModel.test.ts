import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import { createInstancesModel } from './instancesModel.svelte';
import { createReviewModel } from './reviewModel.svelte';

vi.mock('$lib/api', () => ({
	getTrackingSessions: vi.fn(),
	getSessionDetail: vi.fn(),
	deleteSession: vi.fn(),
	reassignSession: vi.fn(),
	getAllSessionDefinitions: vi.fn(),
	restoreSessionDefinition: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function definition(overrides: Partial<SessionDefinition> = {}): SessionDefinition {
	return {
		id: '1',
		name: 'Default Tracking',
		adHocSegments: false,
		trackProtectionCosts: true,
		isProtected: true,
		isActive: true,
		instanceCount: 0,
		createdAt: 0,
		updatedAt: null,
		roster: [],
		...overrides,
	};
}

/** The review model over the real instances model, with the API mocked:
 * the composition under test is the surface plus its scoped list. */
function reviewModel(
	definitions: SessionDefinition[],
	refreshPlayableDefinitions = vi.fn(async () => {}),
) {
	mocked.getAllSessionDefinitions.mockResolvedValue(definitions);
	return createReviewModel({
		listAllDefinitions: () => api.getAllSessionDefinitions(),
		restoreDefinition: (id) => api.restoreSessionDefinition(id),
		refreshPlayableDefinitions,
		createInstances: (definitionId) => createInstancesModel({ definitionId }),
	});
}

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getTrackingSessions.mockResolvedValue({ sessions: [], nextCursor: null, total: 0 });
});

describe('openReview', () => {
	it('opens on the given definition and reads its instances, not the whole history', async () => {
		const model = reviewModel([definition(), definition({ id: '2', name: 'ARIS Dailies' })]);
		await model.openReview('2');

		expect(model.open).toBe(true);
		expect(model.definitionId).toBe('2');
		expect(model.definition?.name).toBe('ARIS Dailies');
		expect(mocked.getTrackingSessions).toHaveBeenCalledWith(undefined, undefined, '2');
	});

	it('switching definition re-reads under the new scope', async () => {
		const model = reviewModel([definition(), definition({ id: '2', name: 'ARIS Dailies' })]);
		await model.openReview('1');
		await model.reviewDefinition('2');

		expect(model.definitionId).toBe('2');
		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith(undefined, undefined, '2');
	});

	it('reads nothing at all when no definition is selected yet', async () => {
		const model = reviewModel([definition()]);
		await model.openReview(null);

		expect(model.open).toBe(true);
		expect(model.definitionId).toBeNull();
		// Not "reads everything": an unscoped page would be the whole
		// recorded history shown under one definition's heading, pager,
		// and empty state.
		expect(mocked.getTrackingSessions).not.toHaveBeenCalled();
		// The switcher is still populated, so choosing one is the way out.
		expect(model.activeDefinitions).toHaveLength(1);
	});

	it('re-selecting the definition already under review does not re-read', async () => {
		const model = reviewModel([definition()]);
		await model.openReview('1');
		mocked.getTrackingSessions.mockClear();

		await model.reviewDefinition('1');
		expect(mocked.getTrackingSessions).not.toHaveBeenCalled();
	});
});

describe('the definitions it offers', () => {
	it('lists every archived definition apart so even an empty one can be restored', async () => {
		const model = reviewModel([
			definition(),
			definition({ id: '2', name: 'Archived With History', isActive: false, instanceCount: 4 }),
			definition({ id: '3', name: 'Archived Empty', isActive: false, instanceCount: 0 }),
		]);
		await model.openReview('1');

		expect(model.activeDefinitions.map((d) => d.id)).toEqual(['1']);
		expect(model.archivedDefinitions.map((d) => d.id)).toEqual(['3', '2']);
	});

	it('offers only active definitions as move targets, never the one under review', async () => {
		const model = reviewModel([
			definition(),
			definition({ id: '2', name: 'ARIS Dailies' }),
			definition({ id: '3', name: 'Archived', isActive: false, instanceCount: 2 }),
		]);
		await model.openReview('1');

		expect(model.moveTargets.map((d) => d.id)).toEqual(['2']);
	});

	it('offers no move target while reviewing an archived definition with nothing else on offer', async () => {
		const model = reviewModel([
			definition({ id: '3', name: 'Archived', isActive: false, instanceCount: 2 }),
		]);
		await model.openReview('3');

		expect(model.moveTargets).toEqual([]);
	});
});

describe('the writes', () => {
	it('re-filing refreshes the definition list, so both instance counts read true', async () => {
		const model = reviewModel([definition(), definition({ id: '2', name: 'ARIS Dailies' })]);
		mocked.getTrackingSessions.mockResolvedValue({
			sessions: [{ id: 's1' }],
			nextCursor: null,
			total: 1,
		} as never);
		mocked.reassignSession.mockResolvedValue({
			sessionId: 's1',
			definitionId: '2',
			sessionName: 'ARIS Dailies',
		});
		await model.openReview('1');
		mocked.getAllSessionDefinitions.mockClear();

		expect(await model.reassign('s1', '2')).toBe(true);
		expect(mocked.getAllSessionDefinitions).toHaveBeenCalledTimes(1);
	});

	it('a refused re-file does not refresh the definition list', async () => {
		const model = reviewModel([definition(), definition({ id: '2', name: 'ARIS Dailies' })]);
		mocked.reassignSession.mockRejectedValueOnce(new Error('Session definition not found'));
		await model.openReview('1');
		mocked.getAllSessionDefinitions.mockClear();

		expect(await model.reassign('s1', '2')).toBe(false);
		expect(mocked.getAllSessionDefinitions).not.toHaveBeenCalled();
	});

	it('deleting an instance refreshes the definition list too', async () => {
		const model = reviewModel([definition()]);
		mocked.deleteSession.mockResolvedValue(undefined);
		await model.openReview('1');
		mocked.getAllSessionDefinitions.mockClear();

		await model.remove('s1');
		expect(mocked.deleteSession).toHaveBeenCalledWith('s1');
		expect(mocked.getAllSessionDefinitions).toHaveBeenCalledTimes(1);
	});

	it('restores the archived definition under review without selecting it', async () => {
		const archived = definition({ id: '3', name: 'Easter Mayhem 2026', isActive: false });
		const restored = { ...archived, isActive: true };
		const refreshPlayableDefinitions = vi.fn(async () => {});
		const model = reviewModel([definition(), archived], refreshPlayableDefinitions);
		mocked.restoreSessionDefinition.mockResolvedValue(restored);
		await model.openReview('3');
		mocked.getAllSessionDefinitions.mockResolvedValue([definition(), restored]);

		expect(await model.restoreCurrent()).toBe(true);
		expect(mocked.restoreSessionDefinition).toHaveBeenCalledWith('3');
		expect(model.definition?.isActive).toBe(true);
		expect(refreshPlayableDefinitions).toHaveBeenCalledTimes(1);
		expect(model.restoring).toBe(false);
	});

	it('keeps a refused restore visible with an actionable error', async () => {
		const archived = definition({ id: '3', name: 'Seasonal', isActive: false });
		const refreshPlayableDefinitions = vi.fn(async () => {});
		const model = reviewModel([archived], refreshPlayableDefinitions);
		mocked.restoreSessionDefinition.mockRejectedValue(
			new Error("A session named 'Seasonal' already exists"),
		);
		await model.openReview('3');

		expect(await model.restoreCurrent()).toBe(false);
		expect(model.error).toBe("A session named 'Seasonal' already exists");
		expect(model.definition?.isActive).toBe(false);
		expect(refreshPlayableDefinitions).not.toHaveBeenCalled();
	});
});

describe('close', () => {
	it('clears the surface and every armed inner control', async () => {
		const model = reviewModel([definition()]);
		await model.openReview('1');
		model.instances.confirmDeleteId = 's1';

		model.close();
		expect(model.open).toBe(false);
		expect(model.instances.confirmDeleteId).toBeNull();
		expect(model.instances.expandedSessionId).toBeNull();
	});
});
