import { commandRefusal } from '$lib/extension/gate';

/**
 * Whether the account surfaces (the connection, the sign-in, the measures
 * and their review) are offered: only while the gate lets signing in to the
 * account run. Reactive, as the gate is.
 */
export function accountOffered(): boolean {
	return commandRefusal('account_sign_in') === null;
}
