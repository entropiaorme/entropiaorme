/**
 * The loadout comparison: setups built from the catalogue, compared per
 * kill across mob HP. A thin alias over the generated
 * typed command.
 */

import * as commands from './commands.gen';

export type {
	ComparedSetup,
	ComparisonAssumptionsView,
	ComponentPick,
	KillFiguresView,
	LoadoutComparison,
	SetupComponentRole,
	SetupPick,
	UnmodelledReason,
} from './commands.gen';

export const compareLoadouts = commands.loadoutCompare;
