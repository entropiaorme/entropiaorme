/**
 * The account connection family: the connection's state, disconnecting,
 * and what the player's own account readings measure of tracked sessions.
 * Thin wrappers over the generated typed
 * commands. Signing in opens a window on the account site itself (see
 * `signInToAccount` in `./shell`); the session it yields never reaches
 * the frontend.
 *
 * Where the readings cover a session's cost, the measured figure is the
 * session's cost; the review of measured sessions compares it with the
 * tracked estimate it replaced. What the readings propose (a strongbox opening, a listing
 * sold or returned, a purchase at auction, something arriving in a session
 * that no loot line explains) is a guess until the player
 * records it through the strongbox, listing or asset purchase commands. An
 * opening's or a purchase's proposals are claimed before it is recorded,
 * then linked to it (or released if recording failed); a listing's are
 * linked once its outcome is recorded.
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
/** The connection at a glance: the one account read every trusted window
 * may make. */
export const getAccountSignal = commands.accountSignal;
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
/** A measured session minute by minute: what was looted beside what
 * arrived, and the measured spend beside the tracked. Reading it offers
 * each arrival nothing explains as a proposal, so each carries its handle. */
export const getSessionMinutes = commands.accountSessionMinutes;
/** Record an arrival as stock of unknown origin: held, funding no
 * activity. Answered with the record's id, which taking it back names. */
export const recordArrivalUnknownOrigin = commands.accountArrivalUnknownOrigin;
/** Take back stock of unknown origin recorded from an arrival: the
 * arrival is open to decide again. */
export const undoArrivalUnknownOrigin = commands.accountArrivalUnknownOriginUndo;
/** Offer an ignored arrival again. */
export const unignoreArrival = commands.accountArrivalUnignore;
/** A strongbox run begins: answered with a reading from before the boxes. */
export const beginAccountRun = commands.accountRunBegin;
/** A strongbox run is done: answered once one more reading is in, which can
 * take up to a minute. */
export const finishAccountRun = commands.accountRunDone;
/** Close a strongbox run without another reading. */
export const cancelAccountRun = commands.accountRunCancel;
/** Read a strongbox run whole, from the reading before its boxes to its
 * latest; an opening that balances stands among the proposals in place of
 * the steps it spans. */
export const readRunWhole = commands.accountRunWhole;
/** The items the measured sessions could not place, and the player's
 * placements. */
export const getCostPlacements = commands.accountCostPlacements;
/** Place an item for every session; answered once the measured costs it
 * moves have been rebuilt. */
export const placeCostItem = commands.accountCostPlace;
/** Clear an item's placement; answered once the measured costs it moves
 * have been rebuilt. */
export const clearCostPlacement = commands.accountCostPlacementClear;
/** The limited armour and plates held, each with the markup it is billed
 * at, and the items that may be declared one. */
export const getArmourPieces = commands.accountArmourPieces;
/** Set a limited piece's markup from now on (`null`: billed at TT); the
 * first one set covers its wear from the start. */
export const setArmourPieceMarkup = commands.accountArmourPieceMarkup;

/** Plan a stock reconciliation from the latest account reading: the app's
 * quantity of each loose stackable beside the account's, kept as the draft
 * being reviewed. A conflict when no reading has been stored; refused while
 * a session is tracked. */
export const planStockReconciliation = commands.accountReconciliationPlan;
/** Plan a stock reconciliation from an inventory pasted from the account
 * site's Copy CSV, which needs no connection. A paste with a row that does
 * not read is refused whole, the reason naming its line. */
export const planStockReconciliationFromPaste = commands.accountReconciliationPaste;
/** The reconciliation being reviewed, or `null` when there is none. */
export const getStockReconciliationDraft = commands.accountReconciliationDraft;
/** Change the draft: validated whole and kept only when it fits, the reason
 * refused otherwise with the draft unchanged. A filter reaches it as the
 * items the shared search matched. */
export const reviseStockReconciliation = commands.accountReconciliationRevise;
/** Apply the draft against the stock as it stands; applying it again
 * changes nothing. Lines whose stock moved since are refused and left for
 * the next check. */
export const applyStockReconciliation = commands.accountReconciliationApply;
/** Take back a reconciliation as a whole; a conflict, with nothing changed,
 * when something it produced has been used since. */
export const undoStockReconciliation = commands.accountReconciliationUndo;
