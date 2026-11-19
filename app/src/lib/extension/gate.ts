/**
 * The gate: whether a command or a route is refused, and why. The default:
 * nothing is refused.
 */

/** Why a command is refused. */
export interface CommandRefusal {
	/** The error a refused call raises. */
	readonly message: string;
	/** A control's short title while the command is refused. */
	readonly hint: string;
}

/** Why `command` is refused, or `null` when it runs. Reactive. */
export function commandRefusal(_command: string): CommandRefusal | null {
	return null;
}

/** Why the route `route` is refused, as a short reason, or `null` when it
 * is open. Reactive. */
export function routeRefusal(_route: string): string | null {
	return null;
}
