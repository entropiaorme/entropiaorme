import { type Writable, writable } from 'svelte/store';
import { getPreference, setPreference } from './preferences';

export type Theme = 'dark' | 'light';
export const DEFAULT_THEME: Theme = 'dark';

const KEY = 'theme';

export const theme: Writable<Theme> = writable(DEFAULT_THEME);

export async function initTheme(): Promise<void> {
	const value = await getPreference<Theme>(KEY, DEFAULT_THEME);
	theme.set(value === 'light' ? 'light' : 'dark');
}

export async function setTheme(value: Theme): Promise<void> {
	theme.set(value);
	await setPreference(KEY, value);
}
