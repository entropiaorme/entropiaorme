import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CreatureEntry, CreatureEntryCheck } from '$lib/api';
import { createCreatureEntries, plateText } from './creatureEntries.svelte';

const zombie: CreatureEntry = {
	name: 'Survival Zombie',
	species: 'Zombie',
	maturity: '04',
	anyNumber: true,
	source: 'player',
};
const gloomrat: CreatureEntry = {
	name: 'Gloomrat',
	species: null,
	maturity: null,
	anyNumber: false,
	source: 'review',
};

const ready: CreatureEntryCheck = {
	kind: 'ready',
	name: 'Survival Zombie',
	maturity: '04',
	anyNumber: true,
};

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((r) => {
		resolve = r;
	});
	return { promise, resolve };
}

function rig(initial: CreatureEntry[] = [zombie, gloomrat]) {
	const deps = {
		list: vi.fn(async () => initial),
		check: vi.fn(async (_text: string, _replacing: string | null) => ready),
		save: vi.fn(async (_text: string, _species: string | null, _replacing: string | null) => [
			gloomrat,
		]),
		remove: vi.fn(async (_name: string) => [zombie]),
	};
	return { deps, model: createCreatureEntries(deps) };
}

beforeEach(() => {
	vi.useFakeTimers();
});
afterEach(() => {
	vi.useRealTimers();
});

describe('the plate text of a kept mob', () => {
	it('is the name then the maturity, or the name alone', () => {
		expect(plateText(zombie)).toBe('Survival Zombie 04');
		expect(plateText(gloomrat)).toBe('Gloomrat');
	});
});

describe('loading the kept mobs', () => {
	it('lists them and marks the model loaded', async () => {
		const { model } = rig();
		expect(model.loaded).toBe(false);
		await model.load();
		expect(model.entries).toEqual([zombie, gloomrat]);
		expect(model.loaded).toBe(true);
		expect(model.error).toBeNull();
	});

	it('says so when the list could not be read, and is still loaded', async () => {
		const { deps, model } = rig();
		deps.list.mockRejectedValueOnce(new Error('disk gone'));
		await model.load();
		expect(model.error).toBeTruthy();
		expect(model.entries).toEqual([]);
		expect(model.loaded).toBe(true);
	});
});

describe('checking what is typed', () => {
	it('asks once, after typing rests, not on every keystroke', async () => {
		const { deps, model } = rig();
		model.text = 'Survival';
		await vi.advanceTimersByTimeAsync(100);
		model.text = 'Survival Zombie';
		await vi.advanceTimersByTimeAsync(100);
		model.text = 'Survival Zombie 04';
		expect(deps.check).not.toHaveBeenCalled();
		expect(model.checking).toBe(true);
		await vi.advanceTimersByTimeAsync(250);
		expect(deps.check).toHaveBeenCalledTimes(1);
		expect(deps.check).toHaveBeenCalledWith('Survival Zombie 04', null);
		expect(model.check).toEqual(ready);
		expect(model.checking).toBe(false);
	});

	it('lets only the latest answer stand when an earlier one arrives late', async () => {
		const { deps, model } = rig();
		const slow = deferred<CreatureEntryCheck>();
		const fast: CreatureEntryCheck = { kind: 'refused', reason: 'Too short.' };
		deps.check.mockImplementationOnce(() => slow.promise);
		deps.check.mockImplementationOnce(async () => fast);

		model.text = 'Sur';
		await vi.advanceTimersByTimeAsync(250);
		model.text = 'Survival Zombie 04';
		await vi.advanceTimersByTimeAsync(250);
		expect(model.check).toEqual(fast);

		slow.resolve(ready);
		await vi.advanceTimersByTimeAsync(0);
		expect(model.check).toEqual(fast);
	});

	it('turns a failed check into a refusal the player can read', async () => {
		const { deps, model } = rig();
		deps.check.mockRejectedValueOnce(new Error('nope'));
		model.text = 'Survival Zombie 04';
		await vi.advanceTimersByTimeAsync(250);
		expect(model.check?.kind).toBe('refused');
		expect(model.checking).toBe(false);
	});

	it('clears the check and never asks when the entry is emptied', async () => {
		const { deps, model } = rig();
		model.text = 'Survival Zombie 04';
		await vi.advanceTimersByTimeAsync(250);
		expect(model.check).toEqual(ready);
		model.text = '   ';
		await vi.advanceTimersByTimeAsync(500);
		expect(model.check).toBeNull();
		expect(model.checking).toBe(false);
		expect(deps.check).toHaveBeenCalledTimes(1);
	});
});

describe('when the entry can be kept', () => {
	it('is ready only on a ready check with nothing checking or busy', async () => {
		const { deps, model } = rig();
		expect(model.ready).toBe(false);
		model.text = 'Survival Zombie 04';
		expect(model.ready).toBe(false); // still checking
		await vi.advanceTimersByTimeAsync(250);
		expect(model.ready).toBe(true);

		model.text = 'Survival Zombie 05';
		expect(model.ready).toBe(false); // checking again

		deps.check.mockResolvedValueOnce({ kind: 'refused', reason: 'No.' });
		await vi.advanceTimersByTimeAsync(250);
		expect(model.ready).toBe(false); // refused
	});

	it('is not ready while a write runs', async () => {
		const { deps, model } = rig();
		const pending = deferred<CreatureEntry[]>();
		deps.save.mockImplementationOnce(() => pending.promise);
		model.text = 'Survival Zombie 04';
		await vi.advanceTimersByTimeAsync(250);
		const saving = model.save();
		expect(model.busy).toBe(true);
		expect(model.ready).toBe(false);
		pending.resolve([zombie]);
		await saving;
		expect(model.busy).toBe(false);
	});
});

describe('keeping a mob', () => {
	it('sends the text and species, replaces the list and resets the entry', async () => {
		const { deps, model } = rig();
		model.text = '  Survival Zombie 04 ';
		model.species = ' Zombie ';
		await vi.advanceTimersByTimeAsync(250);
		await model.save();
		expect(deps.save).toHaveBeenCalledWith('Survival Zombie 04', 'Zombie', null);
		expect(model.entries).toEqual([gloomrat]);
		expect(model.text).toBe('');
		expect(model.species).toBe('');
		expect(model.check).toBeNull();
		expect(model.editing).toBeNull();
	});

	it('sends a blank species as none', async () => {
		const { deps, model } = rig();
		model.text = 'Survival Zombie 04';
		model.species = '   ';
		await vi.advanceTimersByTimeAsync(250);
		await model.save();
		expect(deps.save).toHaveBeenCalledWith('Survival Zombie 04', null, null);
	});

	it('does nothing unless the check is ready', async () => {
		const { deps, model } = rig();
		model.text = 'Survival Zombie 04';
		await model.save(); // not yet checked
		expect(deps.save).not.toHaveBeenCalled();
	});

	it('keeps the entry and says why when the save fails', async () => {
		const { deps, model } = rig();
		await model.load();
		deps.save.mockRejectedValueOnce(new Error('could not write'));
		model.text = 'Survival Zombie 04';
		model.species = 'Zombie';
		await vi.advanceTimersByTimeAsync(250);
		await model.save();
		expect(model.error).toBeTruthy();
		expect(model.text).toBe('Survival Zombie 04');
		expect(model.species).toBe('Zombie');
		expect(model.entries).toEqual([zombie, gloomrat]);
		expect(model.busy).toBe(false);
	});
});

describe('editing a kept mob', () => {
	it('loads it as its plate text with its species, and checks it as a replacement', async () => {
		const { deps, model } = rig();
		model.edit(zombie);
		expect(model.editing).toBe('Survival Zombie');
		expect(model.text).toBe('Survival Zombie 04');
		expect(model.species).toBe('Zombie');
		await vi.advanceTimersByTimeAsync(250);
		expect(deps.check).toHaveBeenCalledWith('Survival Zombie 04', 'Survival Zombie');
	});

	it('loads a mob without species or maturity as a bare name and blank species', () => {
		const { model } = rig();
		model.edit(gloomrat);
		expect(model.text).toBe('Gloomrat');
		expect(model.species).toBe('');
	});

	it('saves over the entry being edited', async () => {
		const { deps, model } = rig();
		model.edit(zombie);
		await vi.advanceTimersByTimeAsync(250);
		await model.save();
		expect(deps.save).toHaveBeenCalledWith('Survival Zombie 04', 'Zombie', 'Survival Zombie');
		expect(model.editing).toBeNull();
	});

	it('cancel puts the entry back to adding, and drops a pending check', async () => {
		const { deps, model } = rig();
		model.edit(zombie);
		model.cancel();
		expect(model.editing).toBeNull();
		expect(model.text).toBe('');
		expect(model.species).toBe('');
		expect(model.checking).toBe(false);
		await vi.advanceTimersByTimeAsync(500);
		expect(deps.check).not.toHaveBeenCalled();
		expect(model.check).toBeNull();
	});
});

describe('removing a kept mob', () => {
	it('removes it by name and takes the list it answers with', async () => {
		const { deps, model } = rig();
		await model.load();
		await model.remove(gloomrat);
		expect(deps.remove).toHaveBeenCalledWith('Gloomrat');
		expect(model.entries).toEqual([zombie]);
	});

	it('says so when it could not be removed, and keeps the list', async () => {
		const { deps, model } = rig();
		await model.load();
		deps.remove.mockRejectedValueOnce(new Error('locked'));
		await model.remove(gloomrat);
		expect(model.error).toBeTruthy();
		expect(model.entries).toEqual([zombie, gloomrat]);
	});
});
