import type { ISODate, Ped, Pes, Ratio, Trend } from './common';

// ── Overview tab ──

export interface ReturnsBreakdown {
	lootTt: Ped;
	pes: Pes;
	codexPes: Pes;
	questPes: Pes;
	ledger: Record<string, Ped>;
}

export interface CycledBreakdown {
	weapon: Ped;
	healing: Ped;
	enhancer: Ped;
	armour: Ped;
	dangling: Ped;
}

export interface LossesBreakdown {
	trackingCost: Ped;
	cycledBreakdown: CycledBreakdown;
	ledger: Record<string, Ped>;
}

export interface OverviewStats {
	totalReturnRate: Ratio;
	trend: Trend;
	returnsBreakdown: ReturnsBreakdown;
	lossesBreakdown: LossesBreakdown;
	totalGains: Ped;
	totalLosses: Ped;
	timeline: TimelineDay[];
	monthlyBreakdown: MonthlyEntry[];
}

export interface TimelineDay {
	date: ISODate;
	lootTt: Ped;
	pes: Pes;
	codexPes: Pes;
	questPes: Pes;
	ledgerGains: Record<string, Ped>;
	trackingCost: Ped;
	ledgerLosses: Record<string, Ped>;
}

export interface MonthlyEntry {
	month: string; // "2026-03"
	lootTt: Ped;
	pes: Pes;
	codexPes: Pes;
	questPes: Pes;
	ledgerGains: Record<string, Ped>;
	trackingCost: Ped;
	ledgerLosses: Record<string, Ped>;
}

// ── Ledger tab ──

export type LedgerEntryType = 'expense' | 'markup';

export interface LedgerEntry {
	id: string;
	date: ISODate;
	type: LedgerEntryType;
	description: string;
	amount: Ped;
	tag: string;
}

export interface LedgerPreset {
	id: string;
	name: string;
	type: LedgerEntryType;
	description: string;
	amount: Ped;
	tag: string;
}

export interface TagSummary {
	tag: string;
	total: Ped;
}

// ── Inventory Ledger tab ──

export interface InventoryItem {
	id: string;
	name: string;
	ttValue: Ped;
	markupPaid: Ped;
	notes: string | null;
	acquiredAt: ISODate;
}

export interface InventorySellResult {
	ledgerEntry: LedgerEntry | null;
	soldItem: InventoryItem;
}

// ── Activity tab ──

export interface MobComparison {
	mobName: string;
	sessions: number;
	kills: number;
	hours: number;
	cycled: Ped;
	pesPer100Ped: Pes;
	lootRate: Ratio;
}

export interface TagComparison {
	tagName: string;
	sessions: number;
	kills: number;
	hours: number;
	cycled: Ped;
	pesPer100Ped: Pes;
	lootRate: Ratio;
}

export interface WeaponComparison {
	weaponName: string;
	sessions: number;
	kills: number;
	hours: number;
	cycled: Ped;
	pesPer100Ped: Pes;
	lootRate: Ratio;
}

// ── Character tab ──

export interface CalibrationStatus {
	calibrated: boolean;
	lastCalibration: ISODate | null;
	stale: boolean;
}

export interface ComputedCharacterStats {
	hp: number;
	topProfessions: ProfessionLevel[];
}

export interface SkillLevel {
	name: string;
	category: string;
	level: number;
	anchorLevel: number | null;
	gainSinceAnchor: number | null;
	rankName: string;
	ttValue: Ped;
	isAttribute: boolean;
}

export interface ProfessionLevel {
	name: string;
	level: number;
	anchorLevel: number | null;
	gainSinceAnchor: number | null;
	category: string;
}

// ── Codex tab ──

export interface CodexSpecies {
	name: string;
	baseCost: number;
	codexType: string | null;
	currentRank: number;
	nextRank: number | null;
	nextCategory: string | null;
	nextCost: number | null;
}

export interface CodexRankItem {
	rank: number;
	category: string;
	cost: number;
	rewardPed: number;
	cat4Bonus: boolean;
	cat4RewardPed: number | null;
	skills: string[];
	cat4Skills: string[];
	claimed: boolean;
	claimedSkill: string | null;
	claimedPed: number | null;
	isNext: boolean;
}

export interface CodexRankBreakdown {
	speciesName: string;
	baseCost: number;
	codexType: string | null;
	currentRank: number;
	ranks: CodexRankItem[];
}

export interface CodexClaimResult {
	speciesName: string;
	rank: number;
	skillName: string;
	pedValue: number;
}

export interface CodexMetaAttribute {
	name: string;
	currentLevel: number | null;
}

export interface CodexMetaClaimResult {
	attributeName: string;
	pedValue: number;
}

export interface CodexSkillOption {
	skillName: string;
	category: string;
	rewardPed: number;
	currentLevel: number | null;
	levelsGained: number;
	professionWeight: number;
	profContribution: number;
	hpIncrease: number | null;
	hpGain: number;
	recommendRank: number | null;
}

// ── Profession optimizer ──

export interface OptimizerSkill {
	name: string;
	weight: number;
	currentLevel: number;
	levelsNeeded: number;
	pedToNextLevel: Ped;
	codexCategory: string | null;
	codexDivisor: number | null;
}

export interface OptimizerAttribute {
	name: string;
	weight: number;
	currentLevel: number;
	contributionFactor: number;
}

export interface ProfessionOptimizerResult {
	profession: string;
	currentLevel: number;
	nextLevel: number;
	gap: number;
	skills: OptimizerSkill[];
	attributes: OptimizerAttribute[];
	error?: string;
}

// ── Path optimizer ──

export interface PathAllocation {
	name: string;
	weight: number;
	currentLevel: number;
	levelsToGain: number;
	pedCost: Ped;
	newLevel: number;
	codexCategory: string | null;
	codexDivisor: number | null;
}

export interface ExcludedSkill {
	name: string;
	weight: number;
	reason: string;
}

export interface PathOptimizerResult {
	profession: string;
	mode: 'target' | 'budget';
	inputTargetLevel: number | null;
	inputPedBudget: number | null;
	currentLevel: number;
	endLevel: number;
	professionLevelsGained: number;
	totalPed: Ped;
	allocations: PathAllocation[];
	attributes: OptimizerAttribute[];
	excluded: ExcludedSkill[];
	error?: string;
}

// ── Prospect forecast ──

export type ProspectSliceType = 'global' | 'tag' | 'mob' | 'weapon';

export interface ProspectOption {
	value: string;
	label: string;
	sessions: number;
	kills: number;
	hours: number;
	cycledPed: Ped;
}

export interface CharacterProspectOptions {
	tags: ProspectOption[];
	mobs: ProspectOption[];
	weapons: ProspectOption[];
}

export interface ProspectSample {
	sessions: number;
	kills: number;
	hours: number;
	cycledPed: Ped;
	lootTt: Ped;
	pes: Pes;
	attributeLevels: number;
	cycledPerHour: Ped;
	lootPerHour: Ped;
	returnRate: Ratio;
	pesPerPed: number;
	lootTtPerPed: number;
}

export interface ProspectRow {
	name: string;
	isAttribute: boolean;
	weight: number;
	currentLevel: number;
	observedShare: number;
	observedRate: number;
	projectedGain: number;
	projectedEndLevel: number;
	professionContribution: number;
	relevant: boolean;
}

export interface ProspectResult {
	profession: string;
	sliceType: ProspectSliceType;
	sliceValue: string | null;
	markupUplift: number;
	currentLevel: number;
	targetLevel: number;
	projectedCycledPed: Ped;
	projectedHours: number;
	expectedLootTt: Ped;
	expectedNetTtBurn: Ped;
	speculativeLootTt: Ped | null;
	speculativeNetTtBurn: Ped | null;
	sample: ProspectSample;
	rows: ProspectRow[];
	warnings: string[];
	error?: string;
}

// ── HP optimizer ──

export interface HpOptimizerSkill {
	name: string;
	hpIncrease: number;
	currentLevel: number;
	levelsPerHp: number;
	pedPerHp: Ped;
	hpPerPed: number;
	codexCategory: string | null;
	codexDivisor: number | null;
}

export interface HpOptimizerAttribute {
	name: string;
	hpIncrease: number;
	currentLevel: number;
	levelsPerHp: number;
	hpContribution: number;
}

export interface HpOptimizerResult {
	currentHp: number;
	skills: HpOptimizerSkill[];
	attributes: HpOptimizerAttribute[];
}
