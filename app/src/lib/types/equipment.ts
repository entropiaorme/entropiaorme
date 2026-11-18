/**
 * Equipment-surface types. The wire shapes re-export from the generated
 * bindings (the authoritative contract); `Equipment` is the historical
 * consumer-facing name of the generated `EquipmentSummary`. Only
 * genuinely frontend-owned view types live here.
 */

import type { Pec } from './common';

export type {
	CostBreakdownLine,
	EquipmentDetail,
	EquipmentKind,
	EquipmentLibrary,
	EquipmentSummary as Equipment,
	HealingMode,
	HealingProfileDto as HealingProfile,
	WeaponAttackRate,
} from '$lib/api/commands.gen';

/** The healing-tool view row the equipment page derives from the
 * library list (a projection, not a wire shape). */
export interface HealingTool {
	id: string;
	name: string;
	/** Whether the player has the entry in use (see `Equipment.enabled`). */
	enabled: boolean;
	costPerHeal: Pec;
	isLimited: boolean;
	reloadSeconds: number | null;
	profile: import('$lib/api/commands.gen').HealingProfileDto | null;
}
