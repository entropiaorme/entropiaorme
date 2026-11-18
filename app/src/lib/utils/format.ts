/** The placeholder a numeric readout shows when it has no value to report:
 *  the same dash the app's other empty states render, written as an escape
 *  rather than the literal glyph so the authoring lint stays honest. */
export const NO_DATA = '\u2014';

/** Format a PED value to 2 decimal places */
export function formatPed(value: number): string {
	return value.toFixed(2);
}

/** Format a PED value to 2 decimal places with half-to-even rounding,
 *  matching the game's own display (15.625 → "15.62", where toFixed
 *  would give "15.63"). */
export function formatPedHalfEven(value: number): string {
	const scaled = value * 100;
	const floor = Math.floor(scaled);
	const diff = scaled - floor;
	let cents: number;
	if (diff > 0.5) cents = floor + 1;
	else if (diff < 0.5) cents = floor;
	else cents = floor % 2 === 0 ? floor : floor + 1;
	return (cents / 100).toFixed(2);
}

/** Format a PEC amount (a cost per use). Two decimals reads best at
 *  ordinary magnitudes, but very slow decays (below 0.1 PEC per use) would
 *  collapse to "0.01" or "0.00"; those get four decimals so the figure stays
 *  honest. */
export function formatPec(pec: number): string {
	return pec.toFixed(pec !== 0 && Math.abs(pec) < 0.1 ? 4 : 2);
}

/** A quantity as a person would write it: whole numbers without decimals. */
export function formatQuantity(quantity: number): string {
	const rounded = Math.round(quantity * 100) / 100;
	return Number.isInteger(rounded) ? rounded.toString() : rounded.toFixed(2);
}

/**
 * Whether a TT figure is nothing at all.
 *
 * Some items have no TT (a strongbox, a token, a voucher) and are worth their
 * markup alone. They are first-class holdings, not a degenerate case: wherever
 * their TT would stand, the app shows how many there are instead
 * (`formatTtOrCount`), and their markup is written in PED, since a percentage
 * of no TT says nothing.
 *
 * Only a TT of exactly nothing qualifies. An item whose TT is merely small
 * (Shrapnel, at 0.0001 PED a unit) still has one, is still held and read in
 * PED, and still has a percentage markup; the tolerance only absorbs
 * floating-point residue, matching the backend's stock epsilon.
 */
export function isZeroTt(ttValue: number): boolean {
	return Math.abs(ttValue) < 1e-9;
}

/**
 * A TT figure in PED. Two decimals at ordinary sizes, but a small holding of
 * a sub-PEC item (a few hundred Shrapnel) would read as 0.00, the very figure
 * reserved for an item with no TT, so a non-zero figure under a PEC keeps four.
 */
export function formatTt(ttValue: number): string {
	return ttValue.toFixed(!isZeroTt(ttValue) && Math.abs(ttValue) < 0.01 ? 4 : 2);
}

/** A count standing where a TT figure would: "\u00d712". */
export function formatCount(quantity: number): string {
	return `\u00d7${formatQuantity(quantity)}`;
}

/** What a TT slot shows: the TT, or for an item with no TT, how many. */
export function formatTtOrCount(ttValue: number, quantity: number): string {
	return isZeroTt(ttValue) ? formatCount(quantity) : formatTt(ttValue);
}

/** The hover text on a count shown in a TT slot. */
export const NO_TT_TITLE = 'No TT value: counted by quantity and worth its markup alone';

/** Format a ratio as a percentage (0.917 → "91.7%") */
export function formatPercent(ratio: number): string {
	return `${(ratio * 100).toFixed(1)}%`;
}

const ORDINAL_RULES = new Intl.PluralRules('en-GB', { type: 'ordinal' });
const ORDINAL_SUFFIX: Partial<Record<Intl.LDMLPluralRule, string>> = {
	one: 'st',
	two: 'nd',
	few: 'rd',
};

/** Format a place in a list (1 → "1st", 12 → "12th", 23 → "23rd"). */
export function formatOrdinal(place: number): string {
	return `${place}${ORDINAL_SUFFIX[ORDINAL_RULES.select(place)] ?? 'th'}`;
}

/** Format a multiplier value (1.5 → "1.50x", 25.0 → "25.0x"). */
export function formatMultiplier(value: number): string {
	return `${value >= 10 ? value.toFixed(1) : value.toFixed(2)}x`;
}

/** Format seconds as duration ("2h 15m") */
export function formatDuration(seconds: number): string {
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	if (h === 0) return `${m}m`;
	return `${h}h ${m.toString().padStart(2, '0')}m`;
}

/** Format seconds as elapsed timer ("5:32" or "1:05:32") */
export function formatElapsed(seconds: number): string {
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	if (h > 0) return `${h}:${m.toString().padStart(2, '0')}:${s.toString().padStart(2, '0')}`;
	return `${m}:${s.toString().padStart(2, '0')}`;
}

/** Format ISO date as short date ("Mar 24") */
export function formatDate(iso: string): string {
	return new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

/** Format an ISO timestamp as its local time of day ("2:41 PM"). */
export function formatTime(iso: string): string {
	return new Date(iso).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
}

/** Format ISO date as full date ("Mar 24, 2026") */
export function formatDateFull(iso: string): string {
	return new Date(iso).toLocaleDateString('en-US', {
		month: 'short',
		day: 'numeric',
		year: 'numeric',
	});
}

/** Today as `YYYY-MM-DD` in the player's own calendar, for prefilling a date
 * input.
 *
 * Built from the local parts rather than via `toISOString`, which converts to
 * UTC first: west of UTC that hands back tomorrow's date through the evening,
 * which is not the day the player is having. */
export function todayDate(): string {
	const now = new Date();
	const month = `${now.getMonth() + 1}`.padStart(2, '0');
	const day = `${now.getDate()}`.padStart(2, '0');
	return `${now.getFullYear()}-${month}-${day}`;
}

/** The local calendar day of an instant as `YYYY-MM-DD`, for prefilling a
 * date input with the day something was seen (see `todayDate`). */
export function localDateOf(iso: string): string {
	const at = new Date(iso);
	if (Number.isNaN(at.getTime())) return todayDate();
	const month = `${at.getMonth() + 1}`.padStart(2, '0');
	const day = `${at.getDate()}`.padStart(2, '0');
	return `${at.getFullYear()}-${month}-${day}`;
}

/** An instant as its day and time ("today, 2:41 PM", "Mar 24, 2:41 PM"). */
export function formatSeenAt(iso: string, now: Date = new Date()): string {
	const at = new Date(iso);
	const today =
		at.getDate() === now.getDate() &&
		at.getMonth() === now.getMonth() &&
		at.getFullYear() === now.getFullYear();
	return `${today ? 'today' : formatDate(iso)}, ${formatTime(iso)}`;
}

/** Format ledger date (today: "HH:MM AM/PM", older: "Mar 24")
 *
 * Ledger dates come in two shapes: a full timestamp, and a bare `YYYY-MM-DD`
 * from entries that only ever knew their day (a backdated entry, an auction
 * resolved on a date the user chose). A bare date must not be run through
 * `new Date`, which reads it as UTC midnight: east of UTC that renders as
 * "1:00 AM" today, and west of it as the previous day. A day with no time
 * shows as a day. */
export function formatLedgerDate(iso: string): string {
	const dateOnly = /^\d{4}-\d{2}-\d{2}$/.exec(iso);
	if (dateOnly) {
		const [year, month, day] = iso.split('-').map(Number);
		return new Date(year, month - 1, day).toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric',
		});
	}
	const d = new Date(iso);
	const today = new Date();
	const isToday =
		d.getDate() === today.getDate() &&
		d.getMonth() === today.getMonth() &&
		d.getFullYear() === today.getFullYear();

	if (isToday) {
		return formatTime(iso);
	}
	return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

/** Names in a sentence: "A", "A and B", "A, B and C". */
export function listNames(names: readonly string[]): string {
	if (names.length <= 1) return names.join('');
	return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}
