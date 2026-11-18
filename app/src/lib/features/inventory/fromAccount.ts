/**
 * What the account readings recorded on their own, in words: the window a
 * record was seen in, and the conversion or sale a manual form may be about
 * to enter a second time. Pure.
 */
import type { RecordedFromAccount, StockConversionFromAccount } from '$lib/api/commands.gen';
import { formatQuantity, formatSeenAt, formatTime, formatTt, isZeroTt } from '$lib/utils/format';

/** One record the account readings made of an item within the last hour:
 * how much it took, that amount's TT, and when its step ended. A
 * conversion and a Trade Terminal sale answer in the same shape. */
export type AccountRecordRead = Pick<
	StockConversionFromAccount,
	'id' | 'quantity' | 'ttValue' | 'recordedAt'
>;

/** The two readings a record was seen between. */
export type SeenBetween = Pick<RecordedFromAccount, 'from' | 'to'>;

function sameDay(a: Date, b: Date): boolean {
	return (
		a.getDate() === b.getDate() &&
		a.getMonth() === b.getMonth() &&
		a.getFullYear() === b.getFullYear()
	);
}

/** When a record from the account happened: between the two readings that
 * saw it ("today, 3:02 PM and 3:03 PM"). The day is said once when both
 * readings fall on it. */
export function fromAccountWindow(step: SeenBetween, now: Date = new Date()): string {
	const from = formatSeenAt(step.from, now);
	const to = sameDay(new Date(step.from), new Date(step.to))
		? formatTime(step.to)
		: formatSeenAt(step.to, now);
	return `between ${from} and ${to}`;
}

/** The hover text of a history row recorded from the account. */
export function fromAccountSentence(step: SeenBetween, now: Date = new Date()): string {
	return `Recorded from your account readings: it happened ${fromAccountWindow(step, now)}.`;
}

/** What an Undo of a record from the account means for the readings. */
export const FROM_ACCOUNT_UNDO = 'Your account readings will not record this again.';

/** How long a record from the account is worth a reminder in a manual
 * form: the backend answers for the last hour, and an open form judges it
 * again against the clock. */
export const RECENT_FROM_ACCOUNT_MS = 60 * 60 * 1000;

/** Whether a record whose step ended at `recordedAt` is still recent. */
export function isRecentFromAccount(recordedAt: string, now: Date = new Date()): boolean {
	const at = Date.parse(recordedAt);
	return Number.isFinite(at) && now.getTime() - at <= RECENT_FROM_ACCOUNT_MS;
}

/** What a manual form may be about to enter a second time: a conversion,
 * or a sale to the Trade Terminal. */
export type AccountRecordKind = 'conversion' | 'sale';

const RECORD_NOUN: Record<AccountRecordKind, string> = {
	conversion: 'a conversion',
	sale: 'a Trade Terminal sale',
};

/** The line beside a manual form's amount when the readings recorded the
 * same kind of thing for the same item within the last hour. It does not
 * stop the form: a second one is often real. */
export function accountRecordSentence(
	kind: AccountRecordKind,
	itemName: string,
	record: AccountRecordRead,
): string {
	const tt = isZeroTt(record.ttValue) ? '' : ` (${formatTt(record.ttValue)} PED TT)`;
	return (
		`Your account recorded ${RECORD_NOUN[kind]} of ${formatQuantity(record.quantity)} ${itemName}${tt} ` +
		`at ${formatTime(record.recordedAt)}; record this only if it is another one.`
	);
}
