/**
 * The overlay's equipment picker model: loads what can be declared in hand
 * and declares a pick, as its hotbar press would. The picker is how a
 * machine that cannot read keys says which weapon, healer, or harvesting
 * tool is in use; with keys it corrects the hand without a press.
 */

import { declareEquipment, getEquipmentLibrary, getSettings } from '$lib/api';
import { declarableOptions } from '$lib/features/equipment/declareOptions';
import { buildEquipmentMenuState, type OverlayEquipmentMenuState } from '$lib/windows/overlayMenu';

export function createEquipmentDeclare(options: {
	/** Re-read the readout once a declaration lands. */
	onDeclared: () => Promise<void> | void;
	onError: (message: string) => void;
}) {
	let declaring = $state(false);

	/** The picker's rows over the current library and hotbar, with the tool
	 * in hand and the healer held marked. */
	async function menuState(
		anchorWidth: number,
		inHand: string | null,
		healer: string | null,
	): Promise<OverlayEquipmentMenuState> {
		const [library, settings] = await Promise.all([getEquipmentLibrary(), getSettings()]);
		return buildEquipmentMenuState(
			anchorWidth,
			declarableOptions(library, settings),
			inHand,
			healer,
		);
	}

	async function declare(equipmentId: number) {
		if (declaring) return;
		declaring = true;
		try {
			await declareEquipment(equipmentId);
			await options.onDeclared();
		} catch (error) {
			options.onError(
				error instanceof Error ? error.message : 'The equipment could not be declared',
			);
		} finally {
			declaring = false;
		}
	}

	return {
		get declaring() {
			return declaring;
		},
		menuState,
		declare,
	};
}
