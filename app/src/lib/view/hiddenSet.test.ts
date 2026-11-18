import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createHiddenSet, orderByVisibility } from './hiddenSet.svelte';

vi.mock('$lib/preferences', () => ({
	getPreference: vi.fn(),
	setPreference: vi.fn(),
}));

import * as preferences from '$lib/preferences';

const prefs = vi.mocked(preferences);

beforeEach(() => {
	vi.clearAllMocks();
	prefs.getPreference.mockResolvedValue([]);
	prefs.setPreference.mockResolvedValue(undefined);
});

describe('orderByVisibility', () => {
	it('moves hidden entries last, each part keeping its order', () => {
		const ranked = ['A', 'B', 'C', 'D'];
		expect(orderByVisibility(ranked, (name) => name === 'A' || name === 'C')).toEqual([
			'B',
			'D',
			'A',
			'C',
		]);
		expect(orderByVisibility(ranked, () => false)).toEqual(ranked);
	});
});

describe('createHiddenSet', () => {
	it('restores the saved set once, however often it is loaded', async () => {
		prefs.getPreference.mockResolvedValue(['Atrox']);
		const set = createHiddenSet('some_hidden');
		await Promise.all([set.load(), set.load()]);
		await set.load();
		expect(prefs.getPreference).toHaveBeenCalledTimes(1);
		expect(prefs.getPreference).toHaveBeenCalledWith('some_hidden', []);
		expect(set.has('Atrox')).toBe(true);
		expect(set.names).toEqual(['Atrox']);
	});

	it('reads a malformed saved value as empty and drops non-string entries', async () => {
		prefs.getPreference.mockResolvedValue('nope');
		const malformed = createHiddenSet('some_hidden');
		await malformed.load();
		expect(malformed.names).toEqual([]);

		prefs.getPreference.mockResolvedValue(['Atrox', 3, null]);
		const mixed = createHiddenSet('other_hidden');
		await mixed.load();
		expect(mixed.names).toEqual(['Atrox']);
	});

	it('saves every change and ignores repeats', () => {
		const set = createHiddenSet('some_hidden');
		set.hide('Atrox');
		set.hide('Atrox');
		set.hide('Daikiba');
		expect(set.names).toEqual(['Atrox', 'Daikiba']);
		expect(prefs.setPreference).toHaveBeenCalledTimes(2);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('some_hidden', ['Atrox', 'Daikiba']);

		set.restore('Atrox');
		set.restore('Atrox');
		expect(set.names).toEqual(['Daikiba']);
		expect(prefs.setPreference).toHaveBeenCalledTimes(3);
		expect(prefs.setPreference).toHaveBeenLastCalledWith('some_hidden', ['Daikiba']);
	});
});
