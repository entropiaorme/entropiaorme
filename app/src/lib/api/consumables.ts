/** Consumable doses: the running session's doses and the reload speed they put in effect, a manual start (timed, or untimed for a dose taken before the session), ending an untimed dose, removal and restore, and a session's doses. */

import * as commands from './commands.gen';

export type {
	ConsumableDose,
	ConsumableDoseRemoval,
	ConsumableDoseRequest,
	ConsumableDoseSource,
	ConsumableDoses,
	ConsumableEffect,
	ConsumableOption,
	ConsumableSettings,
	ReloadSpeedNow,
} from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits whenever a dose starts,
 * ends, or is removed or restored (the colon form of the
 * `consumables.updated` wire topic): a pure trigger for every surface showing
 * doses, the reload speed in effect, or session costs to re-read them.
 */
export const CONSUMABLES_TOPIC = 'consumables:updated';

/** The running and just-ended doses, the reload speed in effect, and what a dose can start from. */
export const getConsumableDoses = commands.consumableDoses;
/** Start a dose of a configured consumable by hand in the running session; `untimed` declares a dose taken before the session as still in force, with no expiry and no cost. Answers with the refreshed readout. */
export const startConsumableDose = commands.consumableDoseStart;
/** End an untimed dose's effect now (it ran out); answers with the refreshed readout. */
export const endConsumableDose = commands.consumableDoseEnd;
/** Remove a dose (a misclick): its effect and booked cost come back off; answers with the refreshed readout. */
export const removeConsumableDose = commands.consumableDoseRemove;
/** Give a removed dose back exactly; answers with the refreshed readout. */
export const restoreConsumableDose = commands.consumableDoseRestore;
/** Every dose taken in a session, removed ones included, oldest first. */
export const getSessionDoses = commands.consumableSessionDoses;
