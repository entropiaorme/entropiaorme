import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MobDetectionReview, ReviewedMob } from '$lib/api';
import {
	describeCertainty,
	describeSettled,
	isUnnamed,
	killCount,
	lineName,
	lineNote,
	mobKey,
	nameAction,
	needsAnswer,
} from './mobDetection';
import { createMobDetectionModel } from './mobDetectionModel.svelte';

vi.mock('$lib/api', () => ({
	getMobDetections: vi.fn(),
	settleMobDetection: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function line(overrides: Partial<ReviewedMob> = {}, name = 'Gloomrat'): ReviewedMob {
	return {
		mob: {
			kind: 'mob',
			species: name,
			maturity: '',
			provisionalKey: `${name.toUpperCase()} L4 #0`,
		},
		name,
		killCount: 2,
		unsureCount: 0,
		exampleText: name,
		currentName: name,
		unlisted: true,
		reviewed: false,
		...overrides,
	};
}

/** A listed mob: nothing about it asks for the player. */
function listed(overrides: Partial<ReviewedMob> = {}): ReviewedMob {
	return line({
		mob: { kind: 'mob', species: 'Atrox', maturity: 'Young', provisionalKey: null },
		name: 'Atrox Young',
		currentName: 'Atrox Young',
		exampleText: 'Atrox Young',
		unlisted: false,
		...overrides,
	});
}

const UNNAMED: ReviewedMob = {
	mob: { kind: 'unnamed' },
	name: null,
	killCount: 7,
	unsureCount: 0,
	exampleText: null,
	currentName: null,
	unlisted: false,
	reviewed: false,
};

function review(sessionId: string, mobs: ReviewedMob[], ended = true): MobDetectionReview {
	return { sessionId, ended, mobs };
}

function harness(session = 's1') {
	let current = session;
	const settled = vi.fn();
	const model = createMobDetectionModel({
		sessionId: () => current,
		onSettled: settled,
	});
	return {
		model,
		settled,
		switchTo(next: string) {
			current = next;
		},
	};
}

beforeEach(() => {
	vi.resetAllMocks();
});

describe('the words the review uses', () => {
	it('gives each mob one line, with how sure the screen was as a quiet note', () => {
		expect(lineName(listed())).toBe('Atrox Young');
		expect(lineNote(listed())).toBeNull();
		expect(lineNote(listed({ killCount: 9, unsureCount: 4 }))).toBe('4 less certain');
		expect(lineNote(listed({ killCount: 3, unsureCount: 3 }))).toBe('less certain');
		expect(lineNote(listed({ currentName: 'Atrox Old' }))).toBe('recorded as Atrox Old');
		expect(lineNote(line())).toBe('not in the mob list');
		expect(lineNote(line({ reviewed: true }))).toBeNull();
		expect(killCount(1)).toBe('1 kill');
		expect(killCount(3)).toBe('3 kills');
	});

	it('asks only about a mob on no list that the player has not settled', () => {
		expect(needsAnswer(line())).toBe(true);
		expect(needsAnswer(line({ reviewed: true }))).toBe(false);
		expect(needsAnswer(listed())).toBe(false);
		expect(needsAnswer(UNNAMED)).toBe(false);
	});

	it('heads the kills the screen could not name as such, and says what they were named', () => {
		expect(isUnnamed(UNNAMED)).toBe(true);
		expect(isUnnamed(listed())).toBe(false);
		expect(lineName(UNNAMED)).toBe('Not named');
		expect(lineNote(UNNAMED)).toBeNull();
		expect(lineNote({ ...UNNAMED, currentName: 'Atrox Young' })).toBe('recorded as Atrox Young');
	});

	it('tells lines apart by the mob and by the level an unlisted name was read at', () => {
		const a = line();
		const b = line({ mob: { ...a.mob, provisionalKey: 'GLOOMRAT L9 #3' } as ReviewedMob['mob'] });
		const keys = [mobKey(a.mob), mobKey(b.mob), mobKey(listed().mob), mobKey(UNNAMED.mob)];
		expect(new Set(keys).size).toBe(4);
	});

	it('says what a settlement did', () => {
		const creature = { name: 'Gloomrat', species: 'Gloomrat', maturity: '' };
		expect(describeSettled({ kind: 'newName', creature })).toBe(
			'Recorded as Gloomrat, and known by that name from now on.',
		);
		expect(
			describeSettled({ kind: 'listed', creature: { ...creature, name: 'Atrox Young' } }),
		).toBe('Recorded as Atrox Young.');
		expect(describeSettled({ kind: 'declared' })).toBe('Put back to the declared mob.');
	});
});

describe('the detected mobs model', () => {
	it('lists the session review and drops a read for a session no longer shown', async () => {
		const h = harness();
		let answer: (value: MobDetectionReview) => void = () => {};
		mocked.getMobDetections.mockReturnValueOnce(
			new Promise((resolve) => {
				answer = resolve;
			}),
		);
		const loading = h.model.refresh();
		h.switchTo('s2');
		answer(review('s1', [line()]));
		await loading;
		expect(h.model.mobs).toEqual([]);

		mocked.getMobDetections.mockResolvedValueOnce(review('s2', [line({}, 'Snarl')]));
		await h.model.refresh();
		expect(h.model.mobs.map((mob) => mob.name)).toEqual(['Snarl']);
	});

	it('names a line with the typed name, applies the fresh review and tells the owner', async () => {
		const h = harness();
		const shown = line();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		expect(h.model.draft(shown)).toBe('Gloomrat');
		h.model.setDraft(shown, 'Gloomrat Prime');
		const settledGroup = line({ reviewed: true, currentName: 'Gloomrat Prime' });
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: {
				kind: 'newName',
				creature: { name: 'Gloomrat Prime', species: 'Gloomrat Prime', maturity: '' },
			},
			review: review('s1', [settledGroup]),
		});
		await h.model.name(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledWith('s1', shown.mob, {
			kind: 'creature',
			name: 'Gloomrat Prime',
		});
		expect(h.model.mobs).toEqual([settledGroup]);
		expect(h.model.notice).toBe('Recorded as Gloomrat Prime, and known by that name from now on.');
		expect(h.model.draft(settledGroup)).toBe('Gloomrat');
		expect(h.settled).toHaveBeenCalledTimes(1);
		expect(h.model.busy).toBeNull();
	});

	it('puts back the declared mob in one action for the whole line', async () => {
		const h = harness();
		const shown = listed({ unsureCount: 2 });
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: { kind: 'declared' },
			review: review('s1', [{ ...shown, reviewed: true }]),
		});
		await h.model.restoreDeclared(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledWith('s1', shown.mob, { kind: 'declared' });
		expect(h.model.notice).toBe('Put back to the declared mob.');
	});

	it('refuses a second settlement while one is saving, and shows a refusal without changing the list', async () => {
		const h = harness();
		const shown = line();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		let fail: (reason: Error) => void = () => {};
		mocked.settleMobDetection.mockReturnValueOnce(
			new Promise((_, reject) => {
				fail = reject;
			}),
		);
		const first = h.model.name(shown);
		expect(h.model.busy).toBe(mobKey(shown.mob));
		await h.model.restoreDeclared(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledTimes(1);
		fail(new Error('"42" does not read as a mob\'s name'));
		await first;
		expect(h.model.error).toMatch(/does not read as a mob/);
		expect(h.model.mobs).toEqual([shown]);
		expect(h.settled).not.toHaveBeenCalled();
		h.model.dismissError();
		expect(h.model.error).toBeNull();
	});

	it('shows nothing for a session still running', async () => {
		const h = harness();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [line()], false));
		await h.model.refresh();
		expect(h.model.mobs).toEqual([]);
	});

	it('offers the mob a loosely typed name may mean, and records it in one action', async () => {
		const h = harness();
		const shown = line();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		h.model.setDraft(shown, 'Atrok Yung');
		const meant = { name: 'Atrox Young', species: 'Atrox', maturity: 'Young' };
		mocked.settleMobDetection.mockResolvedValueOnce({ kind: 'didYouMean', creature: meant });
		await h.model.name(shown);
		expect(h.model.offerFor(shown)).toEqual(meant);
		expect(h.model.mobs).toEqual([shown]);
		expect(h.settled).not.toHaveBeenCalled();
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: { kind: 'listed', creature: meant },
			review: review('s1', [{ ...shown, reviewed: true, currentName: 'Atrox Young' }]),
		});
		await h.model.acceptOffer(shown);
		expect(mocked.settleMobDetection).toHaveBeenLastCalledWith('s1', shown.mob, {
			kind: 'creature',
			name: 'Atrox Young',
		});
		expect(h.model.offerFor(shown)).toBeNull();
		expect(h.model.notice).toBe('Recorded as Atrox Young.');
	});
});

describe('the words beside a name', () => {
	it('says keeping an unlisted spelling is keeping a new mob', () => {
		expect(nameAction(line(), 'Gloomrat')).toBe('Keep as a new mob');
		expect(nameAction(line(), ' Gloomrat ')).toBe('Keep as a new mob');
		expect(nameAction(listed(), 'Atrox Young')).toBe('Confirm');
		expect(nameAction(line(), 'Atrox Young')).toBe('Save');
		expect(nameAction(UNNAMED, 'Atrox Young')).toBe('Record');
	});

	it('says how sure a recorded mob is', () => {
		expect(describeCertainty('recorded')).toBeNull();
		expect(describeCertainty('probable')).toBe('probably');
		expect(describeCertainty('unlisted')).toBe('not in the mob list, unconfirmed');
		expect(describeCertainty('unlisted_probable')).toMatch(/^probably; not in the mob list/);
	});
});
