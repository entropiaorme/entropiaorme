import { getPreference, setPreference } from './preferences';

export const CURRENT_TOS_VERSION = '1.1';

const VERSION_KEY = 'tos_accepted_version';
const TIMESTAMP_KEY = 'tos_accepted_at';

export const getAcceptedTosVersion = (): Promise<string | null> =>
	getPreference<string | null>(VERSION_KEY, null);

export async function setAcceptedTosVersion(version: string): Promise<void> {
	await setPreference(VERSION_KEY, version);
	await setPreference(TIMESTAMP_KEY, new Date().toISOString());
}

export async function isTosAccepted(): Promise<boolean> {
	const accepted = await getAcceptedTosVersion();
	return accepted === CURRENT_TOS_VERSION;
}
