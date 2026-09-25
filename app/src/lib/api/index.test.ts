import { beforeEach, describe, expect, it, vi } from 'vitest';

// The facade's behaviour is its mapping: which typed command, which
// params/body shape, and (for the analytics-flavoured reads) the per-call
// guide-mode demo dispatch. The typed-command transport is mocked out
// wholesale, so these tests pin the facade layer alone; client.ts has its own
// suite. vi.hoisted: the module under test is imported statically, so the
// vi.mock factories run before ordinary top-level consts initialise; these
// seams must be hoisted alongside them.
const { FakeApiError, guideState, tauriInvoke } = vi.hoisted(() => {
	class FakeApiError extends Error {
		constructor(
			public kind: string,
			message: string,
		) {
			super(message);
			this.name = 'ApiError';
		}
	}
	return {
		tauriInvoke: vi.fn(),
		FakeApiError,
		// Mutable guide-state seam: tests flip isActive to drive demo dispatch.
		guideState: { isActive: false },
	};
});

vi.mock('./client', () => ({
	ApiError: FakeApiError,
}));

vi.mock('$lib/guide/state.svelte', () => ({ guideState }));

// Startup readiness has its own suite; here the backend is already up, so
// the transport dispatches straight through.
vi.mock('./readiness.svelte', () => ({ whenSubstrateReady: () => Promise.resolve() }));

// The typed-command transport under the generated bindings: the equipment
// family invokes Tauri commands rather than the HTTP-shaped client.
vi.mock('@tauri-apps/api/core', () => ({
	invoke: (...args: unknown[]) => tauriInvoke(...args),
}));

import * as api from './index';

const DATA = { marker: 'payload' } as const;

beforeEach(() => {
	guideState.isActive = false;
	tauriInvoke.mockReset();
	tauriInvoke.mockResolvedValue(DATA);
});

// The tracking family serves its live surface over typed IPC commands. The
// session-scoped writes, lifecycle verbs, and mob/loot edits
// decision are always commands (no demo branch); the camelCase call
// arguments map to the snake_case invoke keys the generated bindings send.
describe('tracking wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['startTracking', () => api.startTracking(), 'tracking_start', {}],
		['stopTracking', () => api.stopTracking(), 'tracking_stop', {}],
		[
			'deactivateLootItem',
			() => api.deactivateLootItem('s1', 'Shrapnel'),
			'tracking_loot_item_deactivate',
			{ session_id: 's1', item_name: 'Shrapnel' },
		],
		[
			'activateLootItem',
			() => api.activateLootItem('s1', 'Shrapnel'),
			'tracking_loot_item_activate',
			{ session_id: 's1', item_name: 'Shrapnel' },
		],
		[
			'renameSessionMob',
			() => api.renameSessionMob('s1', 'Atrox Young', 'Atrox Mature'),
			'tracking_rename_mob',
			{ session_id: 's1', from_mob_name: 'Atrox Young', to_mob_name: 'Atrox Mature' },
		],
		[
			'restoreSessionMob',
			() => api.restoreSessionMob('s1', 'Atrox Mature'),
			'tracking_restore_mob',
			{ session_id: 's1', current_mob_name: 'Atrox Mature' },
		],
		['releaseMob', () => api.releaseMob(), 'tracking_release_mob', {}],
		[
			'setSessionConfig',
			() => api.setSessionConfig('ARIS Dailies', 50),
			'tracking_session_config',
			{ session_name: 'ARIS Dailies', skill_boost_percent: 50 },
		],
		[
			'setSessionConfig clears both facets with nulls',
			() => api.setSessionConfig(null, null),
			'tracking_session_config',
			{ session_name: null, skill_boost_percent: null },
		],
		[
			'lockManualMob defaults maturity to an empty string',
			() => api.lockManualMob('Atrox'),
			'tracking_manual_mob_lock',
			{ species: 'Atrox', maturity: '' },
		],
		[
			'scanRepairCost',
			() => api.scanRepairCost('s1'),
			'tracking_repair_scan',
			{ session_id: 's1' },
		],
		[
			'saveArmourCost',
			() => api.saveArmourCost('s1', 1.25),
			'tracking_armour_cost',
			{ session_id: 's1', cost: 1.25 },
		],
		[
			'deleteSession',
			() => api.deleteSession('s1'),
			'tracking_session_delete',
			{ session_id: 's1' },
		],
	];

	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});
});

describe('session definition lifecycle wrappers dispatch typed commands', () => {
	it.each([
		[
			'archiveSessionDefinition',
			() => api.archiveSessionDefinition('7'),
			'session_definition_archive',
		],
		[
			'restoreSessionDefinition',
			() => api.restoreSessionDefinition('7'),
			'session_definition_restore',
		],
	] as const)('%s', async (_name, call, command) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledWith(command, { definition_id: 7 });
	});
});

// The three tracking reads with a guide-mode surface: the live command and the
// guide-mode demo command share their DTOs. Guide mode dispatches the `demo_*`
// command with the identical args.
describe('guide-mode demo dispatch', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>, string][] = [
		[
			'getTrackingSessions',
			() => api.getTrackingSessions(),
			'tracking_sessions',
			{ cursor: null, limit: null, definition_id: null },
			'demo_tracking_sessions',
		],
		[
			'getSessionDetail',
			() => api.getSessionDetail('s1'),
			'tracking_session_detail',
			{ session_id: 's1' },
			'demo_tracking_session_detail',
		],
		[
			'getTrackingSnapshot',
			() => api.getTrackingSnapshot(),
			'tracking_snapshot',
			{},
			'demo_tracking_snapshot',
		],
	];

	it.each(
		rows,
	)('%s invokes the live command, or the demo command in guide mode', async (_name, call, command, args, demoCommand) => {
		guideState.isActive = false;
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);

		tauriInvoke.mockClear();
		guideState.isActive = true;
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(demoCommand, args);
	});
});

// The analytics family serves its live surface over typed IPC commands and
// keeps a per-call demo branch (the guide-mode surface dispatches the parallel
// `demo_*` command sharing the DTO): the reads dispatch a live command or a
// demo command by guide state; the writes are always commands (no demo
// branch).
describe('analytics wrappers dispatch typed commands', () => {
	it('getAnalyticsOverview invokes the live command, or the demo command in guide mode', async () => {
		guideState.isActive = false;
		await api.getAnalyticsOverview('30d');
		expect(tauriInvoke).toHaveBeenCalledWith('analytics_overview', { period: '30d' });

		tauriInvoke.mockClear();
		guideState.isActive = true;
		await api.getAnalyticsOverview('30d');
		expect(tauriInvoke).toHaveBeenCalledWith('demo_analytics_overview', { period: '30d' });
	});

	it('getAnalyticsOverview defaults the period to "all"', async () => {
		await api.getAnalyticsOverview();
		expect(tauriInvoke).toHaveBeenCalledWith('analytics_overview', { period: 'all' });
	});

	it('getAnalyticsHunting invokes the command live', async () => {
		await api.getAnalyticsHunting();
		expect(tauriInvoke).toHaveBeenCalledWith('analytics_hunting', {});
	});

	it('getAnalyticsHarvest forwards the selected period and defaults to all time', async () => {
		await api.getAnalyticsHarvest('30d');
		expect(tauriInvoke).toHaveBeenCalledWith('analytics_harvest', { period: '30d' });

		await api.getAnalyticsHarvest();
		expect(tauriInvoke).toHaveBeenLastCalledWith('analytics_harvest', { period: 'all' });
	});

	it('getLedgerEntries invokes ledger_list and reshapes the page (cursor from the body)', async () => {
		tauriInvoke.mockResolvedValue({ entries: [{ id: 'e1' }], nextCursor: 'cur', total: 9 });
		const page = await api.getLedgerEntries('seek', 25);
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_list', { cursor: 'seek', limit: 25 });
		expect(page).toEqual({ items: [{ id: 'e1' }], nextCursor: 'cur', total: 9 });
	});

	it('getLedgerEntries passes nulls for an unpaged first read', async () => {
		tauriInvoke.mockResolvedValue({ entries: [], nextCursor: null, total: 0 });
		const page = await api.getLedgerEntries();
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_list', { cursor: null, limit: null });
		expect(page.nextCursor).toBeNull();
	});

	it('getLedgerSummary invokes the live command, or the demo command in guide mode', async () => {
		guideState.isActive = false;
		await api.getLedgerSummary('all');
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_summary', { period: 'all' });

		tauriInvoke.mockClear();
		guideState.isActive = true;
		await api.getLedgerSummary('30d');
		expect(tauriInvoke).toHaveBeenCalledWith('demo_ledger_summary', { period: '30d' });
	});

	it('getLedgerPresets / getInventoryItems invoke their list commands live', async () => {
		await api.getLedgerPresets();
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_presets_list', {});
		tauriInvoke.mockClear();
		await api.getInventoryItems();
		expect(tauriInvoke).toHaveBeenCalledWith('inventory_list', {});
	});

	it('the write wrappers invoke their commands (no demo branch, even in guide mode)', async () => {
		guideState.isActive = true;
		const entry = { date: '2026-05-01', type: 'expense', description: 'ammo', amount: 1, tag: 't' };
		await api.addLedgerEntry(entry as never);
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_create', { entry });

		const preset = { name: 'resupply', type: 'expense', description: 'd', amount: 1, tag: 't' };
		await api.addLedgerPreset(preset as never);
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_preset_create', { preset });

		const item = { name: 'ESI', tt_value: 10, markup_paid: 2 };
		await api.addInventoryItem(item);
		expect(tauriInvoke).toHaveBeenCalledWith('inventory_create', { item });

		await api.updateInventoryItem('i1', { tt_value: 12 });
		expect(tauriInvoke).toHaveBeenCalledWith('inventory_update', {
			item_id: 'i1',
			patch: { tt_value: 12 },
		});

		await api.sellInventoryItem('i1', { sale_price: 15 });
		expect(tauriInvoke).toHaveBeenCalledWith('inventory_sell', {
			item_id: 'i1',
			sale: { sale_price: 15 },
		});
	});

	it('the delete wrappers invoke their commands and resolve void', async () => {
		tauriInvoke.mockResolvedValue(undefined);
		await expect(api.deleteLedgerEntry('e1')).resolves.toBeUndefined();
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_delete', { entry_id: 'e1' });
		await expect(api.deleteLedgerPreset('p1')).resolves.toBeUndefined();
		expect(tauriInvoke).toHaveBeenCalledWith('ledger_preset_delete', { preset_id: 'p1' });
		await expect(api.deleteInventoryItem('i1')).resolves.toBeUndefined();
		expect(tauriInvoke).toHaveBeenCalledWith('inventory_delete', { item_id: 'i1' });
	});
});

describe('equipment wrappers dispatch typed commands', () => {
	it('searchEquipmentItems short-circuits to [] without a command below two characters', async () => {
		await expect(api.searchEquipmentItems('a', 'weapon')).resolves.toEqual([]);
		await expect(api.searchEquipmentItems('', 'weapon')).resolves.toEqual([]);
		expect(tauriInvoke).not.toHaveBeenCalled();
	});

	it('searchEquipmentItems invokes with q and kind from two characters', async () => {
		tauriInvoke.mockResolvedValue([]);
		await api.searchEquipmentItems('op', 'amp');
		expect(tauriInvoke).toHaveBeenCalledWith('equipment_search', { q: 'op', kind: 'amp' });
	});

	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getEquipmentLibrary', () => api.getEquipmentLibrary(), 'equipment_library', {}],
		[
			'addToLibrary',
			() => api.addToLibrary({ type: 'weapon', catalog_id: 'w1' }),
			'equipment_add',
			{ req: { type: 'weapon', catalog_id: 'w1' } },
		],
		[
			'updateLibrary coerces the string id to a number',
			() => api.updateLibrary('7', { type: 'weapon' }),
			'equipment_update',
			{ item_id: 7, req: { type: 'weapon' } },
		],
		[
			'removeFromLibrary coerces the string id to a number',
			() => api.removeFromLibrary('7'),
			'equipment_delete',
			{ item_id: 7 },
		],
		[
			'getEquipmentDetail coerces the string id to a number',
			() => api.getEquipmentDetail('7'),
			'equipment_detail',
			{ item_id: 7 },
		],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('maps a typed error payload onto the thrown ApiError contract', async () => {
		tauriInvoke.mockRejectedValue({ kind: 'notFound', message: 'Equipment item 9 not found' });
		const failure = api.getEquipmentDetail('9');
		await expect(failure).rejects.toBeInstanceOf(FakeApiError);
		await expect(failure).rejects.toMatchObject({
			kind: 'notFound',
			message: 'Equipment item 9 not found',
		});
	});

	it('maps a message-less kind onto its fixed message', async () => {
		tauriInvoke.mockRejectedValue({ kind: 'unavailable' });
		await expect(api.getEquipmentLibrary()).rejects.toMatchObject({
			kind: 'unavailable',
			message: 'backend substrate not ready',
		});
	});

	it('surfaces an out-of-contract rejection verbatim', async () => {
		tauriInvoke.mockRejectedValue('command equipment_detail not found');
		await expect(api.getEquipmentDetail('7')).rejects.toMatchObject({
			kind: 'unknown',
			message: 'command equipment_detail not found',
		});
	});
});

describe('settings wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getSettings', () => api.getSettings(), 'settings_get', {}],
		[
			'updateSettings passes the partial patch through',
			() => api.updateSettings({ player_name: 'Mikel' }),
			'settings_update',
			{ patch: { player_name: 'Mikel' } },
		],
		['getOverlayPosition', () => api.getOverlayPosition(), 'settings_overlay_position', {}],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('saveOverlayPosition invokes the typed command with x and y and resolves void', async () => {
		tauriInvoke.mockResolvedValue(undefined);
		await expect(api.saveOverlayPosition(120, 48)).resolves.toBeUndefined();
		expect(tauriInvoke).toHaveBeenCalledWith('settings_set_overlay_position', { x: 120, y: 48 });
	});

	it('maps a typed error payload onto the thrown ApiError contract', async () => {
		tauriInvoke.mockRejectedValue({ kind: 'badRequest', message: 'No fields to update' });
		const failure = api.updateSettings({});
		await expect(failure).rejects.toBeInstanceOf(FakeApiError);
		await expect(failure).rejects.toMatchObject({
			kind: 'badRequest',
			message: 'No fields to update',
		});
	});
});

describe('codex wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getCodexSpecies', () => api.getCodexSpecies(), 'codex_species', {}],
		[
			'getCodexSpeciesRanks',
			() => api.getCodexSpeciesRanks('Atrox'),
			'codex_species_ranks',
			{ species_name: 'Atrox' },
		],
		[
			'claimCodexRank',
			() => api.claimCodexRank('Atrox', 3, 'Laser Sniper'),
			'codex_claim',
			{ species_name: 'Atrox', rank: 3, skill_name: 'Laser Sniper' },
		],
		[
			'unclaimCodexRank',
			() => api.unclaimCodexRank('Atrox'),
			'codex_unclaim',
			{ species_name: 'Atrox' },
		],
		[
			'calibrateCodex',
			() => api.calibrateCodex('Atrox', 3),
			'codex_calibrate',
			{ species_name: 'Atrox', rank: 3 },
		],
		[
			'getCodexRecommendation passes an explicit target and professions',
			() =>
				api.getCodexRecommendation('Atrox', 3, {
					target: 'hp',
					professions: ['Sniper (Hit)', 'Ranger (Hit)'],
				}),
			'codex_recommend',
			{
				species_name: 'Atrox',
				rank: 3,
				professions: ['Sniper (Hit)', 'Ranger (Hit)'],
				target: 'hp',
			},
		],
		[
			'getCodexRecommendation defaults to the profession target and no professions',
			() => api.getCodexRecommendation('Atrox', 3),
			'codex_recommend',
			{ species_name: 'Atrox', rank: 3, professions: [], target: 'profession' },
		],
		[
			'getCodexMasteryOptions passes professions and defaults the target',
			() => api.getCodexMasteryOptions({ professions: ['Evader'] }),
			'codex_mastery_options',
			{ professions: ['Evader'], target: 'profession' },
		],
		[
			'getCodexMasteryOptions defaults to no professions',
			() => api.getCodexMasteryOptions(),
			'codex_mastery_options',
			{ professions: [], target: 'profession' },
		],
		['getCodexMetaAttributes', () => api.getCodexMetaAttributes(), 'codex_meta_attributes', {}],
		[
			'claimCodexMeta',
			() => api.claimCodexMeta('Strength'),
			'codex_meta_claim',
			{ attribute_name: 'Strength' },
		],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('maps the not-found rank lookup onto the thrown ApiError contract', async () => {
		tauriInvoke.mockRejectedValue({ kind: 'notFound', message: "Species 'No Such' not found" });
		const failure = api.getCodexSpeciesRanks('No Such');
		await expect(failure).rejects.toBeInstanceOf(FakeApiError);
		await expect(failure).rejects.toMatchObject({
			kind: 'notFound',
			message: "Species 'No Such' not found",
		});
	});
});

describe('quests wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getQuests', () => api.getQuests(), 'quests_list', {}],
		['getQuest coerces the string id', () => api.getQuest('5'), 'quest_get', { quest_id: 5 }],
		[
			'createQuest',
			() => api.createQuest({ name: 'Iron' } as never),
			'quest_create',
			{ input: { name: 'Iron' } },
		],
		[
			'updateQuest',
			() => api.updateQuest('5', { name: 'Iron II' } as never),
			'quest_update',
			{ quest_id: 5, input: { name: 'Iron II' } },
		],
		['startQuest', () => api.startQuest('5'), 'quest_start', { quest_id: 5 }],
		['completeQuest', () => api.completeQuest('5'), 'quest_complete', { quest_id: 5 }],
		[
			'cancelQuest defaults undo_reward to false',
			() => api.cancelQuest('5'),
			'quest_cancel',
			{ quest_id: 5, undo_reward: false },
		],
		[
			'cancelQuest passes an explicit undo_reward',
			() => api.cancelQuest('5', true),
			'quest_cancel',
			{ quest_id: 5, undo_reward: true },
		],
		['getQuestAnalytics', () => api.getQuestAnalytics(), 'quests_analytics', {}],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('deleteQuest invokes the void typed command and resolves void', async () => {
		await expect(api.deleteQuest('5')).resolves.toBeUndefined();
		expect(tauriInvoke).toHaveBeenCalledWith('quest_delete', { quest_id: 5 });
	});
});

describe('suggestion lookups', () => {
	it('getManualMobSuggestions short-circuits on blank input and trims the query', async () => {
		await expect(api.getManualMobSuggestions('')).resolves.toEqual([]);
		expect(tauriInvoke).not.toHaveBeenCalled();

		await api.getManualMobSuggestions(' atrox ');
		expect(tauriInvoke).toHaveBeenCalledWith('tracking_manual_mob_suggestions', {
			q: 'atrox',
			limit: null,
		});
	});
});

describe('manual-scan wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getManualSkillScanStatus', () => api.getManualSkillScanStatus(), 'scan_status', {}],
		[
			'startManualSkillScan passes the page count',
			() => api.startManualSkillScan(5),
			'scan_start',
			{ page_count: 5 },
		],
		[
			'startManualSkillScan defaults the page count to null',
			() => api.startManualSkillScan(),
			'scan_start',
			{ page_count: null },
		],
		['captureManualSkillPage', () => api.captureManualSkillPage(), 'scan_capture', {}],
		['cancelManualSkillScan', () => api.cancelManualSkillScan(), 'scan_cancel', {}],
		['undoManualSkillCapture', () => api.undoManualSkillCapture(), 'scan_undo', {}],
		['processManualSkillScan', () => api.processManualSkillScan(), 'scan_process', {}],
		['acceptManualSkillScan', () => api.acceptManualSkillScan(), 'scan_accept', {}],
		['rejectManualSkillScan', () => api.rejectManualSkillScan(), 'scan_reject', {}],
		['getManualSkillScanPending', () => api.getManualSkillScanPending(), 'scan_pending', {}],
		[
			'setSpacebarCapture passes the enabled flag',
			() => api.setSpacebarCapture(true),
			'scan_spacebar_capture',
			{ enabled: true },
		],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('returns the held pending payload when present', async () => {
		tauriInvoke.mockResolvedValue({ skills: { Anatomy: 12 } });
		await expect(api.getManualSkillScanPending()).resolves.toEqual({ skills: { Anatomy: 12 } });
	});

	it('maps an absent pending result to null (the command returns null directly)', async () => {
		tauriInvoke.mockResolvedValue(null);
		await expect(api.getManualSkillScanPending()).resolves.toBeNull();
	});
});

describe('character wrappers dispatch typed commands', () => {
	const rows: [string, () => Promise<unknown>, string, Record<string, unknown>][] = [
		['getCalibrationStatus', () => api.getCalibrationStatus(), 'character_calibration', {}],
		['getCharacterStats', () => api.getCharacterStats(), 'character_stats', {}],
		['getCharacterSkills', () => api.getCharacterSkills(), 'character_skills', {}],
		['getCharacterProfessions', () => api.getCharacterProfessions(), 'character_professions', {}],
		[
			'getProfessionOptimizer',
			() => api.getProfessionOptimizer('Sniper (Hit)'),
			'character_profession_optimizer',
			{ profession: 'Sniper (Hit)' },
		],
		['getHpOptimizer', () => api.getHpOptimizer(), 'character_hp_optimizer', {}],
		[
			'getCharacterProspectOptions',
			() => api.getCharacterProspectOptions(),
			'character_prospect_options',
			{},
		],
	];
	it.each(rows)('%s', async (_name, call, command, args) => {
		await call();
		expect(tauriInvoke).toHaveBeenCalledTimes(1);
		expect(tauriInvoke).toHaveBeenCalledWith(command, args);
	});

	it('maps a typed error payload onto the thrown ApiError contract', async () => {
		tauriInvoke.mockRejectedValue({ kind: 'internal' });
		await expect(api.getCharacterStats()).rejects.toMatchObject({
			kind: 'internal',
			message: 'Internal Server Error',
		});
	});
});

describe('getProfessionPathOptimizer dispatches the typed command', () => {
	it('maps a targetLevel goal onto the target_level argument, ped_budget null', async () => {
		await api.getProfessionPathOptimizer('Sniper (Hit)', { targetLevel: 40 });
		expect(tauriInvoke).toHaveBeenCalledWith('character_path_optimizer', {
			profession: 'Sniper (Hit)',
			target_level: 40,
			ped_budget: null,
		});
	});

	it('maps a pedBudget goal onto the ped_budget argument, target_level null', async () => {
		await api.getProfessionPathOptimizer('Sniper (Hit)', { pedBudget: 250 });
		expect(tauriInvoke).toHaveBeenCalledWith('character_path_optimizer', {
			profession: 'Sniper (Hit)',
			target_level: null,
			ped_budget: 250,
		});
	});
});

describe('getCharacterProspect dispatches the typed command', () => {
	it('omits sliceValue for the global slice even when one is supplied', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			sliceValue: 'ignored',
		});
		expect(tauriInvoke).toHaveBeenCalledWith('character_prospect', {
			query: { profession: 'Sniper (Hit)', targetLevel: 40, sliceType: 'global' },
		});
	});

	it('omits sliceValue when it is absent on a non-global slice', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'mob',
			sliceValue: null,
		});
		expect(tauriInvoke).toHaveBeenCalledWith('character_prospect', {
			query: { profession: 'Sniper (Hit)', targetLevel: 40, sliceType: 'mob' },
		});
	});

	it('passes sliceValue for a non-global slice', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'mob',
			sliceValue: 'Atrox',
		});
		expect(tauriInvoke).toHaveBeenCalledWith('character_prospect', {
			query: {
				profession: 'Sniper (Hit)',
				targetLevel: 40,
				sliceType: 'mob',
				sliceValue: 'Atrox',
			},
		});
	});

	it('includes markupUplift only when strictly positive', async () => {
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			markupUplift: 0,
		});
		expect(tauriInvoke.mock.calls[0][1]).toEqual({
			query: { profession: 'Sniper (Hit)', targetLevel: 40, sliceType: 'global' },
		});

		tauriInvoke.mockClear();
		tauriInvoke.mockResolvedValue(DATA);
		await api.getCharacterProspect({
			profession: 'Sniper (Hit)',
			targetLevel: 40,
			sliceType: 'global',
			markupUplift: 1.05,
		});
		expect(tauriInvoke.mock.calls[0][1]).toEqual({
			query: {
				profession: 'Sniper (Hit)',
				targetLevel: 40,
				sliceType: 'global',
				markupUplift: 1.05,
			},
		});
	});
});

describe('re-exported client and shell surface', () => {
	it('forwards ApiError from ./client and the shell window commands from ./shell', async () => {
		expect(api.ApiError).toBe(FakeApiError);
		tauriInvoke.mockResolvedValue('aGVsbG8=');
		expect(await api.manualSkillScanCapturePng(2)).toBe('data:image/png;base64,aGVsbG8=');
		expect(tauriInvoke).toHaveBeenCalledWith('capture_png', { page: 2 });
		await api.toggleOverlay();
		expect(tauriInvoke).toHaveBeenCalledWith('toggle_overlay');
		await api.toggleCartographyOverlay();
		expect(tauriInvoke).toHaveBeenCalledWith('toggle_cartography_overlay');
	});
});
