/**
 * Backend API client: typed fetch wrappers for the Python backend.
 *
 * All backend communication goes through this module.
 */

const API_BASE = `http://127.0.0.1:${import.meta.env.ENTROPIAORME_BACKEND_PORT}/api`;

import type { NotableEventCategory, NotableEventType } from '$lib/types/common';
import { guideState } from '$lib/guide/state.svelte';

/**
 * Guide-mode route swap for analytics-flavoured endpoints.
 *
 * When the interactive user guide is active on an analytics-backed surface
 * (analytics or dashboard), reads of analytics / tracking / ledger / inventory
 * are transparently retargeted onto the parallel `/api/demo/*` namespace
 * served by the curated demo DB. Surface components stay unchanged. Only the
 * read endpoints below are wrapped; everything else (live tracking, mutating
 * verbs, etc.) goes to the real backend regardless of guide state.
 */
function demoPath(path: string): string {
	return guideState.isActive ? `/demo${path}` : path;
}

export class ApiError extends Error {
	constructor(
		public status: number,
		message: string
	) {
		super(message);
		this.name = 'ApiError';
	}
}

export async function request<T>(path: string, options?: RequestInit): Promise<T> {
	const url = `${API_BASE}${path}`;
	const resp = await fetch(url, {
		headers: { 'Content-Type': 'application/json' },
		...options
	});

	if (!resp.ok) {
		const text = await resp.text().catch(() => resp.statusText);
		let message = text || resp.statusText;
		try {
			const parsed = JSON.parse(text);
			if (typeof parsed?.detail === 'string' && parsed.detail.trim()) {
				message = parsed.detail;
			}
		} catch {
			// Plain-text or non-JSON error body
		}
		throw new ApiError(resp.status, message);
	}

	return resp.json();
}

// --- Character stats ---

import type {
	CalibrationStatus,
	ComputedCharacterStats,
	SkillLevel,
	ProfessionLevel,
	ProfessionOptimizerResult,
	CodexSpecies,
	CodexRankBreakdown,
	CodexClaimResult,
	CodexSkillOption,
	CodexMetaAttribute,
	CodexMetaClaimResult,
	HpOptimizerResult,
	PathOptimizerResult,
	CharacterProspectOptions,
	ProspectResult,
} from '$lib/types/analytics';

export async function getCalibrationStatus(): Promise<CalibrationStatus> {
	return request('/character/calibration');
}

export async function getCharacterStats(): Promise<ComputedCharacterStats> {
	return request('/character/stats');
}

export async function getCharacterSkills(): Promise<SkillLevel[]> {
	return request('/character/skills');
}

export async function getCharacterProfessions(): Promise<ProfessionLevel[]> {
	return request('/character/professions');
}

export async function getProfessionOptimizer(profession: string): Promise<ProfessionOptimizerResult> {
	return request(`/character/profession-optimizer?profession=${encodeURIComponent(profession)}`);
}

export async function getProfessionPathOptimizer(
	profession: string,
	params: { targetLevel: number } | { pedBudget: number },
): Promise<PathOptimizerResult> {
	const base = `/character/profession-path-optimizer?profession=${encodeURIComponent(profession)}`;
	const suffix = 'targetLevel' in params
		? `&target_level=${params.targetLevel}`
		: `&ped_budget=${params.pedBudget}`;
	return request(base + suffix);
}

export async function getHpOptimizer(): Promise<HpOptimizerResult> {
	return request('/character/hp-optimizer');
}

export async function getCharacterProspectOptions(): Promise<CharacterProspectOptions> {
	return request('/character/prospect-options');
}

export async function getCharacterProspect(params: {
	profession: string;
	targetLevel: number;
	sliceType: 'global' | 'tag' | 'mob' | 'weapon';
	sliceValue?: string | null;
	markupUplift?: number;
}): Promise<ProspectResult> {
	const search = new URLSearchParams({
		profession: params.profession,
		target_level: String(params.targetLevel),
		slice_type: params.sliceType,
	});
	if (params.sliceType !== 'global' && params.sliceValue) {
		search.set('slice_value', params.sliceValue);
	}
	if ((params.markupUplift ?? 0) > 0) {
		search.set('markup_uplift', String(params.markupUplift));
	}
	return request(`/character/prospect?${search.toString()}`);
}

// --- Manual scan flow (public, user-driven page-by-page capture) ---

export type ScanPhase = 'idle' | 'capturing' | 'processing' | 'awaiting_review';

export interface ScanManualStatus {
	active: boolean;
	processing: boolean;
	captured_pages: number;
	expected_pages: number;
	last_scan_time: number | null;
	skills_count?: number;
	configured: boolean;
	game_window_present: boolean;
	phase: ScanPhase;
	processing_progress: { done: number; total: number };
	has_pending_result: boolean;
	error: string | null;
}

export interface SkillScanPending {
	skills: Record<string, number>;
}

export function manualSkillScanCapturePngUrl(page: number): string {
	return `${API_BASE}/scan/skills/capture/${page}`;
}

export async function getManualSkillScanStatus(): Promise<ScanManualStatus> {
	return request('/scan/skills/status');
}

export async function startManualSkillScan(pageCount?: number): Promise<ScanManualStatus & { error?: string }> {
	const suffix = pageCount !== undefined ? `?page_count=${pageCount}` : '';
	return request(`/scan/skills/start${suffix}`, { method: 'POST' });
}

export async function captureManualSkillPage(): Promise<ScanManualStatus & { page?: number; captured?: boolean; error?: string }> {
	return request('/scan/skills/capture', { method: 'POST' });
}

export async function cancelManualSkillScan(): Promise<ScanManualStatus & { error?: string }> {
	return request('/scan/skills/cancel', { method: 'POST' });
}

export async function undoManualSkillCapture(): Promise<ScanManualStatus & { undone_page?: number; error?: string }> {
	return request('/scan/skills/undo', { method: 'POST' });
}

export async function processManualSkillScan(): Promise<ScanManualStatus & { error?: string }> {
	return request('/scan/skills/process', { method: 'POST' });
}

export async function acceptManualSkillScan(): Promise<{ ok?: boolean; skills_persisted?: number; error?: string }> {
	return request('/scan/skills/accept', { method: 'POST' });
}

export async function rejectManualSkillScan(): Promise<{ ok?: boolean; error?: string }> {
	return request('/scan/skills/reject', { method: 'POST' });
}

export async function getManualSkillScanPending(): Promise<SkillScanPending | null> {
	try {
		return await request<SkillScanPending>('/scan/skills/pending');
	} catch (err) {
		if (err instanceof ApiError && err.status === 404) return null;
		throw err;
	}
}

export async function setSpacebarCapture(enabled: boolean): Promise<{ ok?: boolean; enabled?: boolean; error?: string }> {
	return request(`/scan/spacebar-capture?enabled=${enabled}`, { method: 'POST' });
}

// --- Codex ---

export async function getCodexSpecies(): Promise<CodexSpecies[]> {
	return request('/codex/species');
}

export async function getCodexSpeciesRanks(name: string): Promise<CodexRankBreakdown> {
	return request(`/codex/species/${encodeURIComponent(name)}/ranks`);
}

export async function claimCodexRank(
	speciesName: string,
	rank: number,
	skillName: string
): Promise<CodexClaimResult> {
	return request('/codex/claim', {
		method: 'POST',
		body: JSON.stringify({ species_name: speciesName, rank, skill_name: skillName })
	});
}

export async function calibrateCodex(speciesName: string, rank: number): Promise<{ speciesName: string; rank: number }> {
	return request('/codex/calibrate', {
		method: 'POST',
		body: JSON.stringify({ species_name: speciesName, rank })
	});
}

export async function getCodexRecommendation(
	speciesName: string,
	rank: number,
	options?: { target?: 'profession' | 'hp'; profession?: string }
): Promise<CodexSkillOption[]> {
	let url = `/codex/recommend?species_name=${encodeURIComponent(speciesName)}&rank=${rank}`;
	if (options?.target) url += `&target=${encodeURIComponent(options.target)}`;
	if (options?.profession) url += `&profession=${encodeURIComponent(options.profession)}`;
	return request(url);
}

// --- Codex Meta ---

export async function getCodexMetaAttributes(): Promise<CodexMetaAttribute[]> {
	return request('/codex/meta/attributes');
}

export async function claimCodexMeta(attributeName: string): Promise<CodexMetaClaimResult> {
	return request('/codex/meta/claim', {
		method: 'POST',
		body: JSON.stringify({ attribute_name: attributeName })
	});
}

// --- Equipment ---

import type { Equipment, EquipmentDetail } from '$lib/types/equipment';

/** Search result from GET /api/equipment/search */
export interface EquipmentSearchResult {
	catalogId: string | null;
	name: string;
	decay: number; // PEC
	ammoBurn: number; // PEC (ammo units / 100)
	markupPercent?: number;
	isLimited: boolean;
	damageEnhancers?: number;
}

interface AddLibraryRequest {
	type: 'weapon' | 'healing' | 'consumable';
	catalog_id?: string | null;
	name?: string | null;
	amp_catalog_id?: string | null;
	scope_catalog_id?: string | null;
	absorber_catalog_id?: string | null;
	weapon_markup?: number;
	amp_markup?: number;
	scope_markup?: number;
	absorber_markup?: number;
	damage_enhancers?: number;
}

export async function searchEquipmentItems(
	q: string,
	type: 'weapon' | 'amp' | 'healer' | 'scope' | 'absorber' | 'consumable'
): Promise<EquipmentSearchResult[]> {
	if (q.length < 2) return [];
	return request(`/equipment/search?q=${encodeURIComponent(q)}&type=${type}`);
}

export async function getEquipmentLibrary(): Promise<Equipment[]> {
	return request('/equipment/library');
}

export async function addToLibrary(req: AddLibraryRequest): Promise<Equipment> {
	return request('/equipment/library', {
		method: 'POST',
		body: JSON.stringify(req)
	});
}

export async function removeFromLibrary(id: string): Promise<void> {
	await request(`/equipment/library/${id}`, { method: 'DELETE' });
}

export async function updateLibrary(id: string, req: AddLibraryRequest): Promise<Equipment> {
	return request(`/equipment/library/${id}`, {
		method: 'PUT',
		body: JSON.stringify(req)
	});
}

export async function getEquipmentDetail(id: string): Promise<EquipmentDetail> {
	return request(`/equipment/library/${id}/detail`);
}

// --- Tracking ---

import type { TrackingSession, SessionDetail } from '$lib/types/tracking';

export interface TrackingStatus {
	status: 'unavailable' | 'idle' | 'active';
	session_id?: string;
	started_at?: string;
	kill_count?: number;
	cost?: number;
	returns?: number;
	pes?: number;
	returnRate?: number;
	damageDealtTotal?: number;
	weaponDamageDealt?: number;
	weaponCost?: number;
	shotsFiredTotal?: number;
	criticalHitsTotal?: number;
	maxDamage?: number;
	globalsCount?: number;
	hofsCount?: number;
	latestKillLoot?: number | null;
	multiplierLast?: number | null;
	multiplierAvg?: number | null;
	multiplierMax?: number | null;
	multiplierHistory?: number[];
	cumulativeNetHistory?: number[];
	hotbarListenerActive?: boolean;
	weaponAttribution?: 'hotbar' | 'trifecta';
	repairOcrEnabled?: boolean;
	endOfSessionArmourReminderEnabled?: boolean;
	mobEntryMode?: 'mob' | 'tag';
	currentMob?: string | null;
	mobSource?: 'manual' | 'tag' | null;
}

export interface RecentEvent {
	id: string;
	type: NotableEventCategory;
	eventType: NotableEventType;
	description: string;
	value: number | null;
	timestamp: string;
}

export async function getTrackingStatus(): Promise<TrackingStatus> {
	return request(demoPath('/tracking/status'));
}

export async function startTracking(): Promise<{ session_id: string; started_at: string; status: string }> {
	return request('/tracking/start', { method: 'POST' });
}

export async function stopTracking(): Promise<{ session_id: string; kill_count: number }> {
	return request('/tracking/stop', { method: 'POST' });
}

export async function getTrackingSessions(): Promise<TrackingSession[]> {
	return request(demoPath('/tracking/sessions'));
}

export async function getSessionDetail(sessionId: string): Promise<SessionDetail> {
	return request(demoPath(`/tracking/session/${encodeURIComponent(sessionId)}`));
}

export async function deleteSession(sessionId: string): Promise<void> {
	await request(`/tracking/session/${encodeURIComponent(sessionId)}`, { method: 'DELETE' });
}

/** Response shape from the loot deactivate / activate endpoints. */
export interface LootEditResponse {
	sessionId: string;
	killId: string;
	lootItemId: number;
	deactivatedAt: string | null;
	killLootTotalPed: number;
	sessionTotalReturns: number;
}

export async function deactivateLootItem(
	sessionId: string,
	lootItemId: number,
): Promise<LootEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/loot/${lootItemId}/deactivate`,
		{ method: 'POST' },
	);
}

export async function activateLootItem(
	sessionId: string,
	lootItemId: number,
): Promise<LootEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/loot/${lootItemId}/activate`,
		{ method: 'POST' },
	);
}

/** Response shape from the bulk loot-item deactivate / activate
 * endpoints. Operates on every kill_loot_items row matching
 * `(sessionId, itemName)` in one atomic transaction. */
export interface BulkLootItemEditResponse {
	sessionId: string;
	itemName: string;
	affectedRows: number;
	totalValueDelta: number;
	sessionTotalReturns: number;
}

export async function bulkDeactivateLootItem(
	sessionId: string,
	itemName: string,
): Promise<BulkLootItemEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/loot-item/${encodeURIComponent(itemName)}/deactivate`,
		{ method: 'POST' },
	);
}

export async function bulkActivateLootItem(
	sessionId: string,
	itemName: string,
): Promise<BulkLootItemEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/loot-item/${encodeURIComponent(itemName)}/activate`,
		{ method: 'POST' },
	);
}

/** Response shape from the rename-mob / restore-mob endpoints. */
export interface MobEditResponse {
	sessionId: string;
	mobName: string;
	killCount: number;
}

export async function renameSessionMob(
	sessionId: string,
	fromMobName: string,
	toMobName: string,
): Promise<MobEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/rename-mob`,
		{
			method: 'POST',
			body: JSON.stringify({ fromMobName, toMobName }),
		},
	);
}

export async function restoreSessionMob(
	sessionId: string,
	currentMobName: string,
): Promise<MobEditResponse> {
	return request(
		`/tracking/session/${encodeURIComponent(sessionId)}/restore-mob`,
		{
			method: 'POST',
			body: JSON.stringify({ currentMobName }),
		},
	);
}

export async function getRecentEvents(): Promise<RecentEvent[]> {
	return request(demoPath('/tracking/recent-events'));
}

export interface TrackingLive {
	status: 'unavailable' | 'idle' | 'active';
	sessionId?: string;
	elapsed?: number;
	killCount?: number;
	kills?: number;
	cost?: number;
	returns?: number;
	pes?: number;
	net?: number;
	returnRate?: number;
	weaponAttribution?: 'hotbar' | 'trifecta';
	repairOcrEnabled?: boolean;
	endOfSessionArmourReminderEnabled?: boolean;
	mobEntryMode?: 'mob' | 'tag';
	currentMob?: string | null;
	mobSource?: 'manual' | 'tag' | null;
	currentTool?: string | null;
	trifectaAttribution?: {
		activePresetId: string | null;
		presetName: string | null;
		presets: {
			id: string;
			name: string;
		}[];
		smallWeapon: string | null;
		bigWeapon: string | null;
		healTool: string | null;
	} | null;
	recentEvents?: {
		type: NotableEventCategory | 'warning';
		eventType?: NotableEventType;
		description: string;
		value: number;
		timestamp?: string | number;
	}[];
}

export async function getTrackingLive(): Promise<TrackingLive> {
	return request(demoPath('/tracking/live'));
}

export async function releaseMob(): Promise<{ released: string | null }> {
	return request('/tracking/release-mob', { method: 'POST' });
}

export interface ManualMobSuggestion {
	display: string;
	species: string;
	maturity: string;
}

export async function getTrackingTagSuggestions(query: string): Promise<string[]> {
	if (!query.trim()) return [];
	return request(`/tracking/tag-suggestions?q=${encodeURIComponent(query.trim())}`);
}

export async function lockTrackingTag(tag: string): Promise<{ tag: string }> {
	return request('/tracking/tag-lock', {
		method: 'POST',
		body: JSON.stringify({ tag })
	});
}

export async function getManualMobSuggestions(query: string): Promise<ManualMobSuggestion[]> {
	if (!query.trim()) return [];
	return request(`/tracking/manual-mob-suggestions?q=${encodeURIComponent(query.trim())}`);
}

export async function lockManualMob(species: string, maturity = ''): Promise<{
	mobName: string;
	species: string;
	maturity: string;
}> {
	return request('/tracking/manual-mob-lock', {
		method: 'POST',
		body: JSON.stringify({ species, maturity })
	});
}

export async function scanRepairCost(sessionId: string): Promise<{ cost_ped: number; raw_text: string; confidence: number; error?: string }> {
	return request(`/tracking/session/${encodeURIComponent(sessionId)}/repair-scan`, { method: 'POST' });
}

export async function saveArmourCost(sessionId: string, cost: number): Promise<{ sessionId: string; armourCost: number }> {
	return request(`/tracking/session/${encodeURIComponent(sessionId)}/armour-cost`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ cost }),
	});
}

export interface SessionQuestLinkSuggestion {
	sessionId: string;
	suggestionType: 'quest' | 'playlist' | 'none';
	reason: 'single_quest' | 'exact_playlist' | 'no_completions' | 'unclean' | 'ambiguous_playlist' | 'declined' | 'already_linked';
	questId: string | null;
	questName: string | null;
	playlistId: string | null;
	playlistName: string | null;
}

export interface SessionQuestLinkDecision {
	sessionId: string;
	status: 'linked' | 'declined';
	linkType?: 'quest' | 'playlist';
	questId?: string | null;
	questName?: string | null;
	playlistId?: string | null;
	playlistName?: string | null;
}

export async function getSessionQuestLinkSuggestion(sessionId: string): Promise<SessionQuestLinkSuggestion> {
	return request(`/tracking/session/${encodeURIComponent(sessionId)}/quest-link-suggestion`);
}

export async function decideSessionQuestLink(
	sessionId: string,
	action: 'accept' | 'decline'
): Promise<SessionQuestLinkDecision> {
	return request(`/tracking/session/${encodeURIComponent(sessionId)}/quest-link`, {
		method: 'POST',
		body: JSON.stringify({ action }),
	});
}

// --- Analytics ---

import type {
	OverviewStats,
	MobComparison,
	TagComparison,
	WeaponComparison,
	LedgerEntry,
	LedgerPreset,
	InventoryItem,
	InventorySellResult
} from '$lib/types/analytics';

export interface ActivityData {
	mobComparisons: MobComparison[];
	tagComparisons: TagComparison[];
	weaponComparisons: WeaponComparison[];
}

export async function getAnalyticsOverview(period: string = 'all'): Promise<OverviewStats> {
	return request(demoPath(`/analytics/overview?period=${encodeURIComponent(period)}`));
}

export async function getAnalyticsActivity(): Promise<ActivityData> {
	return request(demoPath('/analytics/activity'));
}

export async function getLedgerEntries(): Promise<LedgerEntry[]> {
	return request(demoPath('/analytics/ledger'));
}

export async function addLedgerEntry(entry: Omit<LedgerEntry, 'id'>): Promise<LedgerEntry> {
	return request('/analytics/ledger', {
		method: 'POST',
		body: JSON.stringify(entry)
	});
}

export async function deleteLedgerEntry(id: string): Promise<void> {
	await request(`/analytics/ledger/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export async function getLedgerPresets(): Promise<LedgerPreset[]> {
	return request(demoPath('/analytics/ledger/presets'));
}

export async function addLedgerPreset(preset: Omit<LedgerPreset, 'id'>): Promise<LedgerPreset> {
	return request('/analytics/ledger/presets', {
		method: 'POST',
		body: JSON.stringify(preset)
	});
}

export async function deleteLedgerPreset(id: string): Promise<void> {
	await request(`/analytics/ledger/presets/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

// --- Inventory Ledger ---

export interface InventoryItemPayload {
	name: string;
	tt_value: number;
	markup_paid: number;
	notes?: string | null;
	acquired_at?: string;
}

export interface InventoryItemPatchPayload {
	name?: string;
	tt_value?: number;
	markup_paid?: number;
	notes?: string | null;
}

export interface InventorySellPayload {
	sale_price: number;
	description?: string;
	sold_at?: string;
}

export async function getInventoryItems(): Promise<InventoryItem[]> {
	return request(demoPath('/analytics/inventory'));
}

export async function addInventoryItem(payload: InventoryItemPayload): Promise<InventoryItem> {
	return request('/analytics/inventory', {
		method: 'POST',
		body: JSON.stringify(payload),
	});
}

export async function updateInventoryItem(
	id: string,
	patch: InventoryItemPatchPayload,
): Promise<InventoryItem> {
	return request(`/analytics/inventory/${encodeURIComponent(id)}`, {
		method: 'PATCH',
		body: JSON.stringify(patch),
	});
}

export async function deleteInventoryItem(id: string): Promise<void> {
	await request(`/analytics/inventory/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export async function sellInventoryItem(
	id: string,
	payload: InventorySellPayload,
): Promise<InventorySellResult> {
	return request(`/analytics/inventory/${encodeURIComponent(id)}/sell`, {
		method: 'POST',
		body: JSON.stringify(payload),
	});
}

// --- Quests ---

import type {
	Quest,
	QuestPlaylist,
	QuestCreateData,
	QuestUpdateData,
	PlaylistCreateData,
	PlaylistUpdateData,
	QuestAnalyticsRow,
	PlaylistAnalyticsRow
} from '$lib/types/quests';

export async function getQuests(): Promise<Quest[]> {
	return request('/quests');
}

export async function getQuest(id: string): Promise<Quest> {
	return request(`/quests/${id}`);
}

export async function createQuest(data: QuestCreateData): Promise<Quest> {
	return request('/quests', { method: 'POST', body: JSON.stringify(data) });
}

export async function updateQuest(id: string, data: QuestUpdateData): Promise<Quest> {
	return request(`/quests/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}

export async function deleteQuest(id: string): Promise<void> {
	await request(`/quests/${id}`, { method: 'DELETE' });
}

export async function startQuest(id: string): Promise<Quest> {
	return request(`/quests/${id}/start`, { method: 'POST' });
}

export async function completeQuest(id: string): Promise<Quest> {
	return request(`/quests/${id}/complete`, { method: 'POST' });
}

export async function cancelQuest(id: string, undoReward = false): Promise<Quest> {
	return request(`/quests/${id}/cancel`, {
		method: 'POST',
		body: JSON.stringify({ undo_reward: undoReward })
	});
}

export async function getQuestAnalytics(): Promise<QuestAnalyticsRow[]> {
	return request('/quests/analytics');
}

export async function getPlaylistAnalytics(): Promise<PlaylistAnalyticsRow[]> {
	return request('/quests/playlists/analytics');
}

export async function getPlaylists(): Promise<QuestPlaylist[]> {
	return request('/quests/playlists');
}

export async function createPlaylist(data: PlaylistCreateData): Promise<QuestPlaylist> {
	return request('/quests/playlists', { method: 'POST', body: JSON.stringify(data) });
}

export async function updatePlaylist(id: string, data: PlaylistUpdateData): Promise<QuestPlaylist> {
	return request(`/quests/playlists/${id}`, { method: 'PUT', body: JSON.stringify(data) });
}

export async function deletePlaylist(id: string): Promise<void> {
	await request(`/quests/playlists/${id}`, { method: 'DELETE' });
}

// --- Settings ---

import type { AppSettings } from '$lib/types/settings';

export interface SettingsUpdate {
	chatlog_path?: string;
	player_name?: string;
	hotbar_hooks_enabled?: boolean;
	repair_ocr_enabled?: boolean;
	end_of_session_armour_reminder_enabled?: boolean;
	mob_tracking_mode?: 'mob' | 'tag';
	mob_tracking_tag?: string;
	hotbar?: Record<string, number | null>;
	active_trifecta_preset_id?: string | null;
	trifecta_presets?: {
		id: string;
		name: string;
		small_weapon_id: number | null;
		big_weapon_id: number | null;
		heal_id: number | null;
	}[];
	loot_filter_blacklist?: string[];
}

export async function getSettings(): Promise<AppSettings> {
	return request('/settings');
}

export async function updateSettings(updates: SettingsUpdate): Promise<AppSettings> {
	return request('/settings', { method: 'PATCH', body: JSON.stringify(updates) });
}

// --- Overlay ---

export async function getOverlayPosition(): Promise<{ x: number | null; y: number | null }> {
	return request('/settings/overlay-position');
}

export async function saveOverlayPosition(x: number, y: number): Promise<void> {
	await request('/settings/overlay-position', {
		method: 'PUT',
		body: JSON.stringify({ x, y }),
	});
}
