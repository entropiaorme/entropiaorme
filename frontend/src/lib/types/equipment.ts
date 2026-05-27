import type { Pec } from './common';

/** Enrichment level: 0 = unresolved, 1 = weapon only, 2 = +amp, 3 = full setup */
export type EnrichmentLevel = 0 | 1 | 2 | 3;

/** Equipment library entry (shown in list) */
export interface Equipment {
	id: string;
	name: string;
	type: 'weapon' | 'healing' | 'consumable';
	amplifierName: string | null;
	costPerUse: Pec;
	damageMin: number | null;
	damageMax: number | null;
	reloadSeconds: number | null;
	isLimited: boolean;
	enrichmentLevel: EnrichmentLevel;
}

/** Expanded equipment detail (inline expand) */
export interface EquipmentDetail {
	id: string;
	weapon: {
		catalogId: string | null;
		name: string;
		decay: Pec;
		ammoBurn: number;
		markupPercent: number;
		isLimited: boolean;
		damageEnhancers: number;
	};
	amplifier: {
		catalogId: string | null;
		name: string;
		decay: Pec;
		ammoBurn: number;
		markupPercent: number;
		isLimited: boolean;
	} | null;
	scope: {
		catalogId: string | null;
		name: string;
		decay: Pec;
		ammoBurn: number;
		markupPercent: number;
		isLimited: boolean;
		damageEnhancers: number;
	} | null;
	absorber: {
		catalogId: string | null;
		name: string;
		decay: Pec;
		ammoBurn: number;
		absorptionPercent: number;
		markupPercent: number;
		isLimited: boolean;
	} | null;
	costBreakdown: CostBreakdownLine[];
	totalCostPerUse: Pec;
}

export interface CostBreakdownLine {
	component: string;
	costPec: Pec;
	markupMultiplier: number;
	effectiveCostPec: Pec;
}

export interface HealingTool {
	id: string;
	name: string;
	costPerHeal: Pec;
	isLimited: boolean;
}
