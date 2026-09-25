export const OVERLAY_ARMOUR_COST_WINDOW_LABEL = 'overlay-armour-cost';
export const OVERLAY_ARMOUR_COST_READY_EVENT = 'overlay-armour-cost:ready';
export const OVERLAY_ARMOUR_COST_SHOW_EVENT = 'overlay-armour-cost:show';
export const OVERLAY_ARMOUR_COST_UPDATE_EVENT = 'overlay-armour-cost:update';
export const OVERLAY_ARMOUR_COST_HIDE_EVENT = 'overlay-armour-cost:hide';
export const OVERLAY_ARMOUR_COST_CLOSED_EVENT = 'overlay-armour-cost:closed';
export interface OverlayArmourCostState {
	repairOcrEnabled: boolean;
	// Logical screen-px anchor: horizontal centre and top edge for the popup,
	// so it can re-centre against the cost button as it resizes.
	anchor: { centerX: number; top: number };
}
