/** Format a PED value to 2 decimal places */
export function formatPed(value: number): string {
	return value.toFixed(2);
}

/** Format a ratio as a percentage (0.917 → "91.7%") */
export function formatPercent(ratio: number): string {
	return `${(ratio * 100).toFixed(1)}%`;
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

/** Format ledger date (today: "HH:MM AM/PM", older: "Mar 24") */
export function formatLedgerDate(iso: string): string {
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
