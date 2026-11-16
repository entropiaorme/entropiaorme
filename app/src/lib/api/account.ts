/**
 * The account connection family: the connection's state, disconnecting,
 * and what the player's own account readings measure of tracked sessions,
 * beside the tracked figures. Thin wrappers over the generated typed
 * commands. Signing in opens a window on the account site itself (see
 * `signInToAccount` in `./shell`); the session it yields never reaches
 * the frontend.
 *
 * Measured figures stand beside the tracked ones and never replace or
 * adjust them.
 */

import * as commands from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits when the account
 * connection or its readings change (the colon form of the
 * `account.updated` wire topic): a connect, a disconnect, the session
 * expiring, a reading stored, or a session's measure sealed. A pure
 * trigger for every surface showing the connection to re-read.
 */
export const ACCOUNT_TOPIC = 'account:updated';

export const getAccountStatus = commands.accountStatus;
export const disconnectAccount = commands.accountDisconnect;
export const getSessionMeasure = commands.accountSessionMeasure;
export const getMeasuredSessions = commands.accountMeasuredSessions;
export const getMeasuredSummary = commands.accountMeasuredSummary;
