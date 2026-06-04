import { beforeEach, describe, expect, it, vi } from 'vitest';

// The facade's behaviour is its mapping: which client verb, which path, which
// params/body shape, and (for the analytics-flavoured reads) the per-call
// guide-mode demo dispatch. The generated client is mocked out wholesale, so
// these tests pin the facade layer alone; client.ts has its own suite.
// vi.hoisted: the module under test is imported statically, so the vi.mock
// factories run before ordinary top-level consts initialise; these seams must
// be hoisted alongside them.
const { clientGet, clientPost, clientPut, clientPatch, clientDelete, FakeApiError, guideState } =
	vi.hoisted(() => {
		class FakeApiError extends Error {
			constructor(
				public status: number,
				message: string,
			) {
				super(message);
				this.name = 'ApiError';
			}
		}
		return {
			clientGet: vi.fn(),
			clientPost: vi.fn(),
			clientPut: vi.fn(),
			clientPatch: vi.fn(),
			clientDelete: vi.fn(),
			FakeApiError,
			// Mutable guide-state seam: tests flip isActive to drive demo dispatch.
			guideState: { isActive: false },
		};
	});

vi.mock('./client', () => ({
	ApiError: FakeApiError,
	EVENTS_STREAM_URL: 'http://127.0.0.1:8421/api/events',
	manualSkillScanCapturePngUrl: (page: number) =>
		`http://127.0.0.1:8421/api/scan/skills/capture/${page}`,
	request: vi.fn(),
	unwrap: async (call: Promise<{ data?: unknown }>) => (await call).data,
	client: {
		GET: (...args: unknown[]) => clientGet(...args),
		POST: (...args: unknown[]) => clientPost(...args),
		PUT: (...args: unknown[]) => clientPut(...args),
		PATCH: (...args: unknown[]) => clientPatch(...args),
		DELETE: (...args: unknown[]) => clientDelete(...args),
	},
}));

vi.mock('$lib/guide/state.svelte', () => ({ guideState }));

import * as api from './index';

const DATA = { marker: 'payload' } as const;

beforeEach(() => {
	guideState.isActive = false;
	for (const mock of [clientGet, clientPost, clientPut, clientPatch, clientDelete]) {
		mock.mockReset();
		mock.mockResolvedValue({ data: DATA });
	}
});

type Verb = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
const verbMock: Record<Verb, ReturnType<typeof vi.fn>> = {
	GET: clientGet,
	POST: clientPost,
	PUT: clientPut,
	PATCH: clientPatch,
	DELETE: clientDelete,
};

describe('plain delegating wrappers map to the expected verb, path, and shape', () => {
	const rows: [string, () => Promise<unknown>, Verb, string, unknown?][] = [
		['getCalibrationStatus', () => api.getCalibrationStatus(), 'GET', '/api/character/calibration'],
		['getCharacterStats', () => api.getCharacterStats(), 'GET', '/api/character/stats'],
		['getCharacterSkills', () => api.getCharacterSkills(), 'GET', '/api/character/skills'],
		[
			'getCharacterProfessions',
			() => api.getCharacterProfessions(),
			'GET',
			'/api/character/professions',
		],
		[
			'getProfessionOptimizer',
			() => api.getProfessionOptimizer('Sniper (Hit)'),
			'GET',
			'/api/character/profession-optimizer',
			{ params: { query: { profession: 'Sniper (Hit)' } } },
		],
		['getHpOptimizer', () => api.getHpOptimizer(), 'GET', '/api/character/hp-optimizer'],
		[
			'getCharacterProspectOptions',
			() => api.getCharacterProspectOptions(),
			'GET',
			'/api/character/prospect-options',
		],
		[
			'getManualSkillScanStatus',
			() => api.getManualSkillScanStatus(),
			'GET',
			'/api/scan/skills/status',
		],
		[
			'startManualSkillScan',
			() => api.startManualSkillScan(5),
			'POST',
			'/api/scan/skills/start',
			{ params: { query: { page_count: 5 } } },
		],
		[
			'captureManualSkillPage',
			() => api.captureManualSkillPage(),
			'POST',
			'/api/scan/skills/capture',
		],
		['cancelManualSkillScan', () => api.cancelManualSkillScan(), 'POST', '/api/scan/skills/cancel'],
		['undoManualSkillCapture', () => api.undoManualSkillCapture(), 'POST', '/api/scan/skills/undo'],
		[
			'processManualSkillScan',
			() => api.processManualSkillScan(),
			'POST',
			'/api/scan/skills/process',
		],
		['acceptManualSkillScan', () => api.acceptManualSkillScan(), 'POST', '/api/scan/skills/accept'],
		['rejectManualSkillScan', () => api.rejectManualSkillScan(), 'POST', '/api/scan/skills/reject'],
		[
			'setSpacebarCapture',
			() => api.setSpacebarCapture(true),
			'POST',
			'/api/scan/spacebar-capture',
			{ params: { query: { enabled: true } } },
		],
		['getCodexSpecies', () => api.getCodexSpecies(), 'GET', '/api/codex/species'],
		[
			'getCodexSpeciesRanks',
			() => api.getCodexSpeciesRanks('Atrox'),
			'GET',
			'/api/codex/species/{name}/ranks',
			{ params: { path: { name: 'Atrox' } } },
		],
		[
			'claimCodexRank',
			() => api.claimCodexRank('Atrox', 3, 'Laser Sniper'),
			'POST',
			'/api/codex/claim',
			{ body: { species_name: 'Atrox', rank: 3, skill_name: 'Laser Sniper' } },
		],
		[
			'calibrateCodex',
			() => api.calibrateCodex('Atrox', 3),
			'POST',
			'/api/codex/calibrate',
			{ body: { species_name: 'Atrox', rank: 3 } },
		],
		[
			'getCodexRecommendation',
			() => api.getCodexRecommendation('Atrox', 3, { target: 'hp' }),
			'GET',
			'/api/codex/recommend',
			{
				params: {
					query: { species_name: 'Atrox', rank: 3, target: 'hp', profession: undefined },
				},
			},
		],
		[
			'getCodexMetaAttributes',
			() => api.getCodexMetaAttributes(),
			'GET',
			'/api/codex/meta/attributes',
		],
		[
			'claimCodexMeta',
			() => api.claimCodexMeta('Strength'),
			'POST',
			'/api/codex/meta/claim',
			{ body: { attribute_name: 'Strength' } },
		],
		['getEquipmentLibrary', () => api.getEquipmentLibrary(), 'GET', '/api/equipment/library'],
		[
			'addToLibrary',
			() => api.addToLibrary({ type: 'weapon', catalog_id: 'w1' }),
			'POST',
			'/api/equipment/library',
			{ body: { type: 'weapon', catalog_id: 'w1' } },
		],
		[
			'updateLibrary',
			() => api.updateLibrary('7', { type: 'weapon' }),
			'PUT',
			'/api/equipment/library/{item_id}',
			{ params: { path: { item_id: 7 } }, body: { type: 'weapon' } },
		],
		[
			'getEquipmentDetail',
			() => api.getEquipmentDetail('7'),
			'GET',
			'/api/equipment/library/{item_id}/detail',
			{ params: { path: { item_id: 7 } } },
		],
		['startTracking', () => api.startTracking(), 'POST', '/api/tracking/start'],
		['stopTracking', () => api.stopTracking(), 'POST', '/api/tracking/stop'],
		[
			'deactivateLootItem',
			() => api.deactivateLootItem('s1', 'Shrapnel'),
			'POST',
			'/api/tracking/session/{session_id}/loot-item/{item_name}/deactivate',
			{ params: { path: { session_id: 's1', item_name: 'Shrapnel' } } },
		],
		[
			'activateLootItem',
			() => api.activateLootItem('s1', 'Shrapnel'),
			'POST',
			'/api/tracking/session/{session_id}/loot-item/{item_name}/activate',
			{ params: { path: { session_id: 's1', item_name: 'Shrapnel' } } },
		],
		[
			'renameSessionMob',
			() => api.renameSessionMob('s1', 'Atrox Young', 'Atrox Mature'),
			'POST',
			'/api/tracking/session/{session_id}/rename-mob',
			{
				params: { path: { session_id: 's1' } },
				body: { fromMobName: 'Atrox Young', toMobName: 'Atrox Mature' },
			},
		],
		[
			'restoreSessionMob',
			() => api.restoreSessionMob('s1', 'Atrox Mature'),
			'POST',
			'/api/tracking/session/{session_id}/restore-mob',
			{ params: { path: { session_id: 's1' } }, body: { currentMobName: 'Atrox Mature' } },
		],
		['releaseMob', () => api.releaseMob(), 'POST', '/api/tracking/release-mob'],
		[
			'lockTrackingTag',
			() => api.lockTrackingTag('team hunt'),
			'POST',
			'/api/tracking/tag-lock',
			{ body: { tag: 'team hunt' } },
		],
		[
			'lockManualMob defaults maturity to an empty string',
			() => api.lockManualMob('Atrox'),
			'POST',
			'/api/tracking/manual-mob-lock',
			{ body: { species: 'Atrox', maturity: '' } },
		],
		[
			'scanRepairCost',
			() => api.scanRepairCost('s1'),
			'POST',
			'/api/tracking/session/{session_id}/repair-scan',
			{ params: { path: { session_id: 's1' } } },
		],
		[
			'saveArmourCost',
			() => api.saveArmourCost('s1', 1.25),
			'POST',
			'/api/tracking/session/{session_id}/armour-cost',
			{ params: { path: { session_id: 's1' } }, body: { cost: 1.25 } },
		],
		[
			'getSessionQuestLinkSuggestion',
			() => api.getSessionQuestLinkSuggestion('s1'),
			'GET',
			'/api/tracking/session/{session_id}/quest-link-suggestion',
			{ params: { path: { session_id: 's1' } } },
		],
		[
			'decideSessionQuestLink',
			() => api.decideSessionQuestLink('s1', 'accept'),
			'POST',
			'/api/tracking/session/{session_id}/quest-link',
			{ params: { path: { session_id: 's1' } }, body: { action: 'accept' } },
		],
		[
			'addLedgerEntry',
			() => api.addLedgerEntry({ description: 'ammo' } as never),
			'POST',
			'/api/analytics/ledger',
			{ body: { description: 'ammo' } },
		],
		[
			'addLedgerPreset',
			() => api.addLedgerPreset({ name: 'resupply' } as never),
			'POST',
			'/api/analytics/ledger/presets',
			{ body: { name: 'resupply' } },
		],
		[
			'addInventoryItem',
			() => api.addInventoryItem({ name: 'ESI', tt_value: 10, markup_paid: 2 }),
			'POST',
			'/api/analytics/inventory',
			{ body: { name: 'ESI', tt_value: 10, markup_paid: 2 } },
		],
		[
			'updateInventoryItem',
			() => api.updateInventoryItem('i1', { tt_value: 12 }),
			'PATCH',
			'/api/analytics/inventory/{item_id}',
			{ params: { path: { item_id: 'i1' } }, body: { tt_value: 12 } },
		],
		[
			'sellInventoryItem',
			() => api.sellInventoryItem('i1', { sale_price: 15 }),
			'POST',
			'/api/analytics/inventory/{item_id}/sell',
			{ params: { path: { item_id: 'i1' } }, body: { sale_price: 15 } },
		],
		['getQuests', () => api.getQuests(), 'GET', '/api/quests'],
		[
			'getQuest coerces the string id to a number',
			() => api.getQuest('5'),
			'GET',
			'/api/quests/{quest_id}',
			{ params: { path: { quest_id: 5 } } },
		],
		[
			'createQuest',
			() => api.createQuest({ name: 'Iron' } as never),
			'POST',
			'/api/quests',
			{ body: { name: 'Iron' } },
		],
		[
			'updateQuest',
			() => api.updateQuest('5', { name: 'Iron II' } as never),
			'PUT',
			'/api/quests/{quest_id}',
			{ params: { path: { quest_id: 5 } }, body: { name: 'Iron II' } },
		],
		[
			'startQuest',
			() => api.startQuest('5'),
			'POST',
			'/api/quests/{quest_id}/start',
			{ params: { path: { quest_id: 5 } } },
		],
		[
			'completeQuest',
			() => api.completeQuest('5'),
			'POST',
			'/api/quests/{quest_id}/complete',
			{ params: { path: { quest_id: 5 } } },
		],
		[
			'cancelQuest defaults undo_reward to false',
			() => api.cancelQuest('5'),
			'POST',
			'/api/quests/{quest_id}/cancel',
			{ params: { path: { quest_id: 5 } }, body: { undo_reward: false } },
		],
		['getQuestAnalytics', () => api.getQuestAnalytics(), 'GET', '/api/quests/analytics'],
		[
			'getPlaylistAnalytics',
			() => api.getPlaylistAnalytics(),
			'GET',
			'/api/quests/playlists/analytics',
		],
		['getPlaylists', () => api.getPlaylists(), 'GET', '/api/quests/playlists'],
		[
			'createPlaylist',
			() => api.createPlaylist({ name: 'dailies' } as never),
			'POST',
			'/api/quests/playlists',
			{ body: { name: 'dailies' } },
		],
		[
			'updatePlaylist',
			() => api.updatePlaylist('9', { name: 'weeklies' } as never),
			'PUT',
			'/api/quests/playlists/{playlist_id}',
			{ params: { path: { playlist_id: 9 } }, body: { name: 'weeklies' } },
		],
		['getSettings', () => api.getSettings(), 'GET', '/api/settings'],
		[
			'updateSettings',
			() => api.updateSettings({ player_name: 'Mikel' }),
			'PATCH',
			'/api/settings',
			{ body: { player_name: 'Mikel' } },
		],
		['startRecording', () => api.startRecording(), 'POST', '/api/recording/start'],
		['getRecordingStatus', () => api.getRecordingStatus(), 'GET', '/api/recording/status'],
		[
			'stopRecording',
			() => api.stopRecording({ scenario_name: 'baseline' }),
			'POST',
			'/api/recording/stop',
			{ body: { scenario_name: 'baseline' } },
		],
		['abortRecording', () => api.abortRecording(), 'POST', '/api/recording/abort'],
		[
			'getOverlayPosition',
			() => api.getOverlayPosition(),
			'GET',
			'/api/settings/overlay-position',
		],
	];

	it.each(rows)('%s', async (_name, call, verb, path, options) => {
		const result = await call();
		const mock = verbMock[verb];
		expect(mock).toHaveBeenCalledTimes(1);
		if (options === undefined) {
			expect(mock).toHaveBeenCalledWith(path);
		} else {
			expect(mock).toHaveBeenCalledWith(path, options);
		}
		expect(result).toEqual(DATA);
	});
});

describe('void-returning wrappers delegate without unwrapping', () => {
	const rows: [string, () => Promise<void>, Verb, string, unknown][] = [
		[
			'removeFromLibrary coerces the string id to a number',
			() => api.removeFromLibrary('7'),
			'DELETE',
			'/api/equipment/library/{item_id}',
			{ params: { path: { item_id: 7 } } },
		],
		[
			'deleteSession',
			() => api.deleteSession('s1'),
			'DELETE',
			'/api/tracking/session/{session_id}',
			{ params: { path: { session_id: 's1' } } },
		],
		[
			'deleteLedgerEntry',
			() => api.deleteLedgerEntry('e1'),
			'DELETE',
			'/api/analytics/ledger/{entry_id}',
			{ params: { path: { entry_id: 'e1' } } },
		],
		[
			'deleteLedgerPreset',
			() => api.deleteLedgerPreset('p1'),
			'DELETE',
			'/api/analytics/ledger/presets/{preset_id}',
			{ params: { path: { preset_id: 'p1' } } },
		],
		[
			'deleteInventoryItem',
			() => api.deleteInventoryItem('i1'),
			'DELETE',
			'/api/analytics/inventory/{item_id}',
			{ params: { path: { item_id: 'i1' } } },
		],
		[
			'deleteQuest',
			() => api.deleteQuest('5'),
			'DELETE',
			'/api/quests/{quest_id}',
			{ params: { path: { quest_id: 5 } } },
		],
		[
			'deletePlaylist',
			() => api.deletePlaylist('9'),
			'DELETE',
			'/api/quests/playlists/{playlist_id}',
			{ params: { path: { playlist_id: 9 } } },
		],
		[
			'saveOverlayPosition',
			() => api.saveOverlayPosition(120, 48),
			'PUT',
			'/api/settings/overlay-position',
			{ body: { x: 120, y: 48 } },
		],
	];

	it.each(rows)('%s', async (_name, call, verb, path, options) => {
		await expect(call()).resolves.toBeUndefined();
		expect(verbMock[verb]).toHaveBeenCalledWith(path, options);
	});
});

describe('guide-mode demo dispatch', () => {
	const rows: [string, () => Promise<unknown>, string, string, unknown?][] = [
		[
			'getTrackingSessions',
			() => api.getTrackingSessions(),
			'/api/tracking/sessions',
			'/api/demo/tracking/sessions',
		],
		[
			'getSessionDetail',
			() => api.getSessionDetail('s1'),
			'/api/tracking/session/{session_id}',
			'/api/demo/tracking/session/{session_id}',
			{ params: { path: { session_id: 's1' } } },
		],
		[
			'getTrackingSnapshot',
			() => api.getTrackingSnapshot(),
			'/api/tracking/snapshot',
			'/api/demo/tracking/snapshot',
		],
		[
			'getAnalyticsOverview',
			() => api.getAnalyticsOverview('30d'),
			'/api/analytics/overview',
			'/api/demo/analytics/overview',
			{ params: { query: { period: '30d' } } },
		],
		[
			'getAnalyticsActivity',
			() => api.getAnalyticsActivity(),
			'/api/analytics/activity',
			'/api/demo/analytics/activity',
		],
		[
			'getLedgerEntries',
			() => api.getLedgerEntries(),
			'/api/analytics/ledger',
			'/api/demo/analytics/ledger',
		],
		[
			'getLedgerPresets',
			() => api.getLedgerPresets(),
			'/api/analytics/ledger/presets',
			'/api/demo/analytics/ledger/presets',
		],
		[
			'getInventoryItems',
			() => api.getInventoryItems(),
			'/api/analytics/inventory',
			'/api/demo/analytics/inventory',
		],
	];

	it.each(rows)(
		'%s reads the real route normally and the demo route in guide mode',
		async (_name, call, realPath, demoPath, options) => {
			guideState.isActive = false;
			await call();
			expect(clientGet).toHaveBeenCalledTimes(1);
			expect(clientGet.mock.calls[0][0]).toBe(realPath);

			clientGet.mockClear();
			clientGet.mockResolvedValue({ data: DATA });
			guideState.isActive = true;
			await call();
			expect(clientGet).toHaveBeenCalledTimes(1);
			expect(clientGet.mock.calls[0][0]).toBe(demoPath);
			if (options !== undefined) {
				expect(clientGet.mock.calls[0][1]).toEqual(options);
			}
		},
	);

	it('getAnalyticsOverview defaults the period to "all" in both modes', async () => {
		await api.getAnalyticsOverview();
		expect(clientGet).toHaveBeenCalledWith('/api/analytics/overview', {
			params: { query: { period: 'all' } },
		});

		clientGet.mockClear();
		clientGet.mockResolvedValue({ data: DATA });
		guideState.isActive = true;
		await api.getAnalyticsOverview();
		expect(clientGet).toHaveBeenCalledWith('/api/demo/analytics/overview', {
			params: { query: { period: 'all' } },
		});
	});

	it('mutating verbs ignore guide mode entirely', async () => {
		guideState.isActive = true;
		await api.addLedgerEntry({ description: 'ammo' } as never);
		expect(clientPost).toHaveBeenCalledWith('/api/analytics/ledger', {
			body: { description: 'ammo' },
		});

		await api.startTracking();
		expect(clientPost).toHaveBeenCalledWith('/api/tracking/start');
	});
});

describe('searchEquipmentItems', () => {
	it('short-circuits to [] without a network call below two characters', async () => {
		await expect(api.searchEquipmentItems('a', 'weapon')).resolves.toEqual([]);
		await expect(api.searchEquipmentItems('', 'weapon')).resolves.toEqual([]);
		expect(clientGet).not.toHaveBeenCalled();
	});

	it('queries with q and type from two characters', async () => {
		await api.searchEquipmentItems('op', 'amp');
		expect(clientGet).toHaveBeenCalledWith('/api/equipment/search', {
			params: { query: { q: 'op', type: 'amp' } },
		});
	});
});

describe('suggestion lookups', () => {
	it('getTrackingTagSuggestions short-circuits on blank input and trims the query', async () => {
		await expect(api.getTrackingTagSuggestions('   ')).resolves.toEqual([]);
		expect(clientGet).not.toHaveBeenCalled();

		await api.getTrackingTagSuggestions('  team ');
		expect(clientGet).toHaveBeenCalledWith('/api/tracking/tag-suggestions', {
			params: { query: { q: 'team' } },
		});
	});

	it('getManualMobSuggestions short-circuits on blank input and trims the query', async () => {
		await expect(api.getManualMobSuggestions('')).resolves.toEqual([]);
		expect(clientGet).not.toHaveBeenCalled();

		await api.getManualMobSuggestions(' atrox ');
		expect(clientGet).toHaveBeenCalledWith('/api/tracking/manual-mob-suggestions', {
			params: { query: { q: 'atrox' } },
		});
	});
});

describe('getManualSkillScanPending', () => {
	it('returns the pending payload when present', async () => {
		clientGet.mockResolvedValue({ data: { skills: { Anatomy: 12 } } });
		await expect(api.getManualSkillScanPending()).resolves.toEqual({
			skills: { Anatomy: 12 },
		});
	});

	it('maps a 404 to null (no pending result is an expected state)', async () => {
		clientGet.mockRejectedValue(new FakeApiError(404, 'no pending scan'));
		await expect(api.getManualSkillScanPending()).resolves.toBeNull();
	});

	it('rethrows any other ApiError status', async () => {
		clientGet.mockRejectedValue(new FakeApiError(500, 'broken'));
		await expect(api.getManualSkillScanPending()).rejects.toMatchObject({ status: 500 });
	});

	it('rethrows non-ApiError failures', async () => {
		clientGet.mockRejectedValue(new TypeError('network down'));
		await expect(api.getManualSkillScanPending()).rejects.toBeInstanceOf(TypeError);
	});
});

describe('getProfessionPathOptimizer', () => {
	it('maps a targetLevel goal onto the target_level query', async () => {
		await api.getProfessionPathOptimizer('Sniper (Hit)', { targetLevel: 40 });
		expect(clientGet).toHaveBeenCalledWith('/api/character/profession-path-optimizer', {
			params: { query: { profession: 'Sniper (Hit)', target_level: 40 } },
		});
	});

	it('maps a pedBudget goal onto the ped_budget query', async () => {
		await api.getProfessionPathOptimizer('Sniper (Hit)', { pedBudget: 250 });
		expect(clientGet).toHaveBeenCalledWith('/api/character/profession-path-optimizer', {
			params: { query: { profession: 'Sniper (Hit)', ped_budget: 250 } },
		});
	});
});

describe('getCharacterProspect', () => {
	it('omits slice_value for the global slice even when one is supplied', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			sliceValue: 'ignored',
		});
		expect(clientGet).toHaveBeenCalledWith('/api/character/prospect', {
			params: {
				query: { profession: 'Sniper (Hit)', target_level: 40, slice_type: 'global' },
			},
		});
	});

	it('omits slice_value when it is absent on a non-global slice', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'mob',
			sliceValue: null,
		});
		expect(clientGet).toHaveBeenCalledWith('/api/character/prospect', {
			params: {
				query: { profession: 'Sniper (Hit)', target_level: 40, slice_type: 'mob' },
			},
		});
	});

	it('passes slice_value for a non-global slice', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'mob',
			sliceValue: 'Atrox',
		});
		expect(clientGet).toHaveBeenCalledWith('/api/character/prospect', {
			params: {
				query: {
					profession: 'Sniper (Hit)',
					target_level: 40,
					slice_type: 'mob',
					slice_value: 'Atrox',
				},
			},
		});
	});

	it('includes markup_uplift only when strictly positive', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			markupUplift: 0,
		});
		expect(clientGet.mock.calls[0][1]).toEqual({
			params: {
				query: { profession: 'Sniper (Hit)', target_level: 40, slice_type: 'global' },
			},
		});

		clientGet.mockClear();
		clientGet.mockResolvedValue({ data: DATA });
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			markupUplift: 1.05,
		});
		expect(clientGet.mock.calls[0][1]).toEqual({
			params: {
				query: {
					profession: 'Sniper (Hit)',
					target_level: 40,
					slice_type: 'global',
					markup_uplift: 1.05,
				},
			},
		});
	});
});

describe('re-exported client surface', () => {
	it('forwards ApiError, request, and the URL helpers from ./client', () => {
		expect(api.ApiError).toBe(FakeApiError);
		expect(api.EVENTS_STREAM_URL).toBe('http://127.0.0.1:8421/api/events');
		expect(api.manualSkillScanCapturePngUrl(2)).toBe(
			'http://127.0.0.1:8421/api/scan/skills/capture/2',
		);
		expect(typeof api.request).toBe('function');
	});
});
