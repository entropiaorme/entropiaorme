import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionCostWindow, ProtectionOverview, ProtectionSet } from '$lib/api';
import { createProtectionModel } from './protectionModel.svelte';

vi.mock('$lib/api', () => ({
	PROTECTION_TOPIC: 'protection:updated',
	archiveProtectionSet: vi.fn(),
	createProtectionSet: vi.fn(),
	getProtectionOverview: vi.fn(),
	restoreProtectionSet: vi.fn(),
	undoProtectionRecording: vi.fn(),
	updateProtectionSet: vi.fn(),
}));

const protectionListeners: Array<() => void> = [];
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (_topic: string, handler: () => void) => {
		protectionListeners.push(handler);
		return () => {};
	}),
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
	backlog: { lastRecordedAt: null, sessions: 0, hits: 0 },
};

const repair: ProtectionCostWindow = {
	id: '12',
	kind: 'repair',
	setId: null,
	setName: null,
	consumedTtPed: null,
	markupPercent: null,
	costPed: 8,
	costKnown: true,
	status: 'booked',
	reason: null,
	createdAt: 1_756_400_000,
	supersededAt: null,
	undoable: true,
	allocations: [
		{
			sessionId: 'a',
			sessionName: null,
			definitionName: 'ARIS Dailies',
			startedAt: 1_756_300_000,
			hitCount: 30,
			allocationShare: 0.75,
			costPed: 6,
			contexts: [],
		},
		{
			sessionId: 'b',
			sessionName: null,
			definitionName: 'ARIS Dailies',
			startedAt: 1_756_310_000,
			hitCount: 10,
			allocationShare: 0.25,
			costPed: 2,
			contexts: [],
		},
	],
};

const overview: ProtectionOverview = {
	sets: [set],
	removedSets: [],
	unlimited: { lastRecordedAt: null, sessions: 3, hits: 120 },
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

	it('undoes a recording through its confirmation, naming what it gives back', async () => {
		mocked.undoProtectionRecording.mockResolvedValue({ ...overview, recentCostWindows: [] });
		const model = createProtectionModel();
		await model.load();
		model.askUndoWindow(repair);
		expect(model.undoModalOpen).toBe(true);
		expect(model.pendingUndo?.title).toBe('Undo this unlimited repair?');
		expect(model.pendingUndo?.detail).toContain('8.00 PED comes off 2 sessions');
		await model.confirmUndo();
		expect(mocked.undoProtectionRecording).toHaveBeenCalledWith({
			kind: 'recording',
			windowId: 12,
		});
		expect(model.undoModalOpen).toBe(false);
	});

	it('keeps the undo open with the reason when the backend refuses it', async () => {
		mocked.undoProtectionRecording.mockRejectedValue(
			new Error('Only the latest recording of each armour stream can be undone'),
		);
		const model = createProtectionModel();
		await model.load();
		model.askUndoWindow(repair);
		await model.confirmUndo();
		expect(model.undoModalOpen).toBe(true);
		expect(model.error).toContain('latest recording');
	});

	it('offers to undo only a reading that booked nothing', async () => {
		const model = createProtectionModel();
		const reading = {
			id: '5',
			setId: '7',
			ttValuePed: 41.2,
			source: 'manual' as const,
			rawText: null,
			observedAt: 1_756_400_000,
			resetReason: null,
			measured: true,
		};
		model.askUndoReading({ ...set, latestObservation: reading });
		expect(model.pendingUndo).toBeNull();
		model.askUndoReading({ ...set, latestObservation: { ...reading, measured: false } });
		expect(model.pendingUndo?.target).toEqual({ kind: 'reading', observationId: 5 });
	});

	it('restores a removed set', async () => {
		mocked.restoreProtectionSet.mockResolvedValue(overview);
		const model = createProtectionModel();
		await model.restoreSet(set);
		expect(mocked.restoreProtectionSet).toHaveBeenCalledWith('7');
		expect(model.overview.sets).toEqual([set]);
	});

	it('re-reads in place when any window records or undoes a cost', async () => {
		const model = createProtectionModel();
		await model.load();
		await model.subscribe();
		mocked.getProtectionOverview.mockResolvedValue({ ...overview, recentCostWindows: [repair] });
		protectionListeners.at(-1)?.();
		await vi.waitFor(() => expect(model.overview.recentCostWindows).toEqual([repair]));
		expect(model.loading).toBe(false);
	});

	it('carries what is still unrecorded', async () => {
		const model = createProtectionModel();
		await model.load();
		expect(model.overview.unrecorded).toEqual({ sessions: 3, hits: 120 });
	});
});
