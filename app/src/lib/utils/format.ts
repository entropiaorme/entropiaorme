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
		return d.toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
	}
	return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}
