/**
 * The account connection family: the connection's state, disconnecting,
 * and what the player's own account readings measure of tracked sessions,
 * beside the tracked figures. Thin wrappers over the generated typed
 * commands. Signing in opens a window on the account site itself (see
 * `signInToAccount` in `./shell`); the session it yields never reaches
 * the frontend.
 *
 * Measured figures stand beside the tracked ones and never replace or
 * adjust them. What the readings propose (a strongbox opening, a listing
 * sold or returned) is a guess until the player records it through the
 * strongbox or listing commands. An opening's proposals are claimed before
 * it is recorded, then linked to it (or released if recording failed); a
 * listing's are linked once its outcome is recorded.
 */

import * as commands from './commands.gen';

/**
 * The Tauri-bus topic the shell's event bridge emits when the account
 * connection or its readings change (the colon form of the
 * `account.updated` wire topic): a connect, a disconnect, the session
 * expiring, a reading stored, a session's measure sealed, or a proposal
 * found or decided. A pure
 * trigger for every surface showing the connection to re-read.
 */
export const ACCOUNT_TOPIC = 'account:updated';

export const getAccountStatus = commands.accountStatus;
export const disconnectAccount = commands.accountDisconnect;
export const getSessionMeasure = commands.accountSessionMeasure;
export const getMeasuredSessions = commands.accountMeasuredSessions;
export const getMeasuredSummary = commands.accountMeasuredSummary;
export const getProposals = commands.accountProposals;
/** Claim proposals before the opening they show is recorded. */
export const claimProposals = commands.accountProposalsClaim;
/** Release proposals claimed for a recording that did not happen. */
export const releaseProposals = commands.accountProposalsRelease;
/** Link proposals, as one, to the record they became. */
export const linkProposals = commands.accountProposalsLink;
export const dismissProposal = commands.accountProposalDismiss;
/** A strongbox run begins: answered with a reading from before the boxes. */
export const beginAccountRun = commands.accountRunBegin;
/** A strongbox run is done: answered once one more reading is in, which can
 * take up to a minute. */
export const finishAccountRun = commands.accountRunDone;
/** Close a strongbox run without another reading. */
export const cancelAccountRun = commands.accountRunCancel;
