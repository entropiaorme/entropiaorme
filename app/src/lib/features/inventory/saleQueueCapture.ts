/**
 * What a capture of the game's sell queue means to the player, and what an
 * accepted draft records.
 *
 * A capture answers one draft per queued item that read whole, the items in
 * view that did not, and how many the window says are queued; or why it read
 * nothing. The lines here say each of those plainly. A draft fills the same
 * form a typed listing does, and the figures it fills are marked for checking
 * where nothing the window prints could catch a misread digit.
 *
 * Accepting a draft records, figure by figure, whether it went in as read or
 * as the player changed it: a figure typed over a read is the player's, not
 * the screen's.
 *
 * Pure, so the surface and the floating capture button say the same thing.
 */

import type { OverlayCaptureReply } from '$lib/api';
import type {
	DraftName,
	ListingFieldSourceInput,
	QueuedField,
	QueuedListingDraft,
	ReadFigure,
	SaleQueueCapture,
	SaleQueueRefusal,
	UnreadQueuedItem,
	UnreadWhy,
} from '$lib/api/commands.gen';
import type { ListingDraftFields } from './listingIntake';

/** A capture that read the queue. */
export type QueueRead = Extract<SaleQueueCapture, { kind: 'read' }>;

/** What to tell the player when a capture read no queue: what to change in
 * the game for the next one, or that the listing is theirs to type. */
export function refusalLine(reason: SaleQueueRefusal): string {
	switch (reason) {
		case 'gameWindowNotFound':
			return 'The Entropia Universe window was not found';
		case 'gameWindowOffScreen':
			return 'The game window is off the screen this app can capture; move it onto that screen';
		case 'captureFailed':
			return 'The game window could not be captured; enter the listing by hand';
		case 'captureNotAllowed':
			return 'Screen capture was not allowed; allow it to read the sell queue';
		case 'sellWindowNotOpen':
			return 'Open the Auction sell window in the game, with its title in view';
		case 'sellWindowNotWhole':
			return 'The sell window is partly covered or off the screen; bring all of it into view';
		case 'queueNotVisible':
			return 'The sell queue could not be read; make sure nothing covers it';
		case 'priceModeUnread':
			return "The sell window's PED and Markup switch could not be read; make sure nothing covers it";
		case 'otherPriceMode':
			return 'Switch the sell window to PED prices';
		case 'queueEmpty':
			return 'The sell queue is empty';
		case 'readerUnavailable':
			return 'The sell queue reader is not available; enter the listing by hand';
		default: {
			const unhandled: never = reason;
			return unhandled;
		}
	}
}

function items(count: number): string {
	return count === 1 ? '1 queued item' : `${count} queued items`;
}

/** How many of the queued items were read, when not all of them were; null
 * when every one was. The count the window prints is given only when it read
 * and agrees with what is in view. */
export function countLine(read: QueueRead): string | null {
	const drafts = read.drafts.length;
	switch (read.queued.kind) {
		case 'all':
			return null;
		case 'some': {
			const rest = read.queued.of - drafts - read.unread.length;
			const line = `Read ${drafts} of ${read.queued.of} queued items.`;
			if (rest <= 0) return line;
			return `${line} ${rest === 1 ? '1 is' : `${rest} are`} out of view.`;
		}
		case 'unread':
			return `Read ${items(drafts)}. The queue's count could not be read, so some may be out of view.`;
		case 'mismatched':
			return `Read ${items(drafts)}. The count shown does not match the rows in view, so some may be out of view.`;
		default: {
			const unhandled: never = read.queued;
			return unhandled;
		}
	}
}

const FIELD_NAMES: Record<QueuedField, string> = {
	itemName: 'the item name',
	ttValue: 'the TT value',
	quantity: 'the quantity',
	startingBid: 'the opening bid',
	buyout: 'the buyout',
	auctionDays: 'the days',
	listingFee: 'the fee',
	markup: 'the markup',
};

const WHY: Record<UnreadWhy, string> = {
	notRead: 'did not read clearly',
	cutOff: 'is cut off',
	outOfBounds: 'is outside what the window allows',
	disagrees: "does not agree with the window's other figures",
};

/** Why a queued item in view gave no draft: its first figure that failed. */
export function unreadLine(item: UnreadQueuedItem): string {
	const first = item.problems[0];
	if (!first) return 'Not read';
	return `Not read: ${FIELD_NAMES[first.field]} ${WHY[first.why]}`;
}

/** What the floating capture button says of its capture, and whether that is
 * a failure the player should act on before tabbing away. */
export function overlayReplyLine(reply: OverlayCaptureReply): { message: string; failed: boolean } {
	if (reply.kind === 'refused') return { message: refusalLine(reply.reason), failed: true };
	if (reply.drafts === 0) {
		return { message: 'No queued item read whole. Check the main window.', failed: true };
	}
	const notRead = reply.unread > 0 ? `, ${reply.unread} not` : '';
	return {
		message: `Read ${items(reply.drafts)}${notRead}. Check the main window.`,
		failed: false,
	};
}

/** One line summing a draft up for the list of drafts, each figure as the
 * window printed it. */
export function draftSummary(draft: QueuedListingDraft): string {
	const buyout =
		draft.buyout.kind === 'price' ? `buyout ${draft.buyout.figure.text} PED` : 'no buyout';
	const days = draft.auctionDays.value === 1 ? '1 day' : `${draft.auctionDays.text} days`;
	return [
		draft.quantity.text,
		`opening bid ${draft.startingBid.text} PED`,
		buyout,
		days,
		`fee ${draft.listingFee.text} PED`,
	].join(' · ');
}

/** The form's fields for a draft: each figure as read. A buyout the window
 * showed switched off is no buyout, never zero. */
export function draftFields(draft: QueuedListingDraft): ListingDraftFields {
	return {
		itemName: draft.itemName.text,
		quantity: draft.quantity.value,
		ttValue: draft.ttValue.value,
		auctionFee: draft.listingFee.value,
		auctionDays: draft.auctionDays.value,
		startingBid: draft.startingBid.value,
		buyout: draft.buyout.kind === 'price' ? draft.buyout.figure.value : null,
	};
}

/** How the name in the form came to be there: as read, as the application
 * matched the read name to a holding of another name, or as the player typed
 * or picked it. */
export type NameOrigin = 'read' | 'matched' | 'player';

/** Why a figure read off the window carries the "check" mark. */
export const FIGURE_CHECK =
	'Nothing on screen fully confirms this; compare it with the sell window.';
const NAME_IN_LIST =
	"Found in the sell window's inventory list but not in the app's item data; compare it with the sell window.";
const NAME_NOWHERE =
	"Not found in the app's item data or the sell window's inventory list; compare it with the sell window.";
const NAME_MATCHED =
	'Matched to a holding recorded under another name; make sure it is the item being sold.';

/** Why a queued item's name carries the "check" mark, or null when the
 * application's item data names it. */
export function nameCheck(name: DraftName): string | null {
	switch (name.found) {
		case 'itemData':
			return null;
		case 'inventoryList':
			return NAME_IN_LIST;
		case 'nowhere':
			return NAME_NOWHERE;
		default: {
			const unhandled: never = name.found;
			return unhandled;
		}
	}
}

/** For each field of the form, why it carries the "check" mark, or null.
 * A figure is marked while it still holds what was read and nothing on screen
 * fully confirmed it; one the player has changed is theirs, and carries none.
 * A name the application matched to a holding stays marked until the player
 * edits it or picks a holding. */
export function checkMarks(
	draft: QueuedListingDraft,
	fields: ListingDraftFields,
	nameOrigin: NameOrigin,
): Record<keyof ListingDraftFields, string | null> {
	const marked = (figure: ReadFigure, value: number | null) =>
		figure.check && value === figure.value ? FIGURE_CHECK : null;
	const name =
		nameOrigin === 'matched'
			? NAME_MATCHED
			: fields.itemName.trim() === draft.itemName.text
				? nameCheck(draft.itemName)
				: null;
	return {
		itemName: name,
		quantity: marked(draft.quantity, fields.quantity),
		ttValue: marked(draft.ttValue, fields.ttValue),
		auctionFee: marked(draft.listingFee, fields.auctionFee),
		auctionDays: marked(draft.auctionDays, fields.auctionDays),
		startingBid: marked(draft.startingBid, fields.startingBid),
		buyout: draft.buyout.kind === 'price' ? marked(draft.buyout.figure, fields.buyout) : null,
	};
}

type FieldSource = ListingFieldSourceInput['source'];

/** A figure kept as read: checked or not by the window's other figures. */
function asRead(text: string | null, check: boolean): FieldSource {
	return check ? { kind: 'readUnchecked', text } : { kind: 'readChecked', text };
}

const TYPED: FieldSource = { kind: 'typed' };

/** Where each figure of an accepted draft came from: kept as read (with the
 * text the window printed, and whether the window's own figures checked it),
 * the name as the application matched it to a holding, or typed. A TT value
 * the player changed is left out: the listing keeps no TT value from the
 * form, so there is nothing typed to record. `recordedName` is the name the
 * listing is recorded under. */
export function fieldSources(
	draft: QueuedListingDraft,
	fields: ListingDraftFields,
	recordedName: string,
	nameOrigin: NameOrigin,
): ListingFieldSourceInput[] {
	const figure = (
		field: ListingFieldSourceInput['field'],
		figure: ReadFigure,
		value: number | null,
	): ListingFieldSourceInput => ({
		field,
		source: value === figure.value ? asRead(figure.text, figure.check) : TYPED,
	});
	const read = draft.itemName.text;
	const name: FieldSource =
		recordedName.trim() === read
			? asRead(read, draft.itemName.found !== 'itemData')
			: nameOrigin === 'matched'
				? { kind: 'matched', text: read }
				: TYPED;
	const buyout: FieldSource =
		draft.buyout.kind === 'off'
			? fields.buyout === null
				? asRead(null, false)
				: TYPED
			: fields.buyout === draft.buyout.figure.value
				? asRead(draft.buyout.figure.text, draft.buyout.figure.check)
				: TYPED;
	const sources: ListingFieldSourceInput[] = [{ field: 'itemName', source: name }];
	if (fields.ttValue === draft.ttValue.value) {
		sources.push(figure('ttValue', draft.ttValue, fields.ttValue));
	}
	sources.push(
		figure('quantity', draft.quantity, fields.quantity),
		figure('startingBid', draft.startingBid, fields.startingBid),
		{ field: 'buyout', source: buyout },
		figure('listingFee', draft.listingFee, fields.auctionFee),
		figure('auctionDays', draft.auctionDays, fields.auctionDays),
	);
	return sources;
}

/** What identifies a queued row from one capture to the next: its name and
 * every figure as printed. The game keeps a queued row until its own list
 * button is pressed, so a row already recorded comes back in a recapture. */
export function rowKey(draft: QueuedListingDraft): string {
	return JSON.stringify([
		draft.itemName.text,
		draft.ttValue.text,
		draft.quantity.text,
		draft.startingBid.text,
		draft.buyout.kind === 'price' ? draft.buyout.figure.text : null,
		draft.listingFee.text,
		draft.auctionDays.text,
	]);
}
