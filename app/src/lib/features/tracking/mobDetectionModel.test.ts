import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MobDetectionGroup, MobDetectionReview } from '$lib/api';
import {
	continuityLine,
	describeCertainty,
	describeGrade,
	describeSettled,
	groupKey,
	isContinuity,
	isSuggestion,
	isUnlisted,
	killCount,
	nameAction,
} from './mobDetection';
import { createMobDetectionModel } from './mobDetectionModel.svelte';

vi.mock('$lib/api', () => ({
	getMobDetections: vi.fn(),
	settleMobDetection: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function group(overrides: Partial<MobDetectionGroup> = {}, name = 'Gloomrat'): MobDetectionGroup {
	return {
		group: {
			grade: 'provisional',
			creature: { name, species: '', maturity: '' },
			provisionalKey: `${name.toUpperCase()} L4 #0`,
		},
		killCount: 2,
		exampleText: name,
		currentName: name,
		reviewed: false,
		...overrides,
	};
}

function review(sessionId: string, groups: MobDetectionGroup[], ended = true): MobDetectionReview {
	return { sessionId, ended, groups };
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
	it('says how sure the screen was, never a backend grade name', () => {
		expect(describeGrade('named')).toBe('Read clearly');
		expect(describeGrade('guess')).toBe('Probably');
		expect(describeGrade('provisional')).toBe('Not in the creature list');
		expect(describeGrade('latest_read')).toMatch(/^Suggested/);
		expect(describeGrade('provisional_latest_read')).toMatch(/^Suggested/);
		expect(isSuggestion('latest_read')).toBe(true);
		expect(isSuggestion('guess')).toBe(false);
		expect(isUnlisted('provisional_guess')).toBe(true);
		expect(isUnlisted('named')).toBe(false);
		expect(describeGrade('continuity')).toBe('From the kills before');
		expect(isSuggestion('continuity')).toBe(false);
		expect(isUnlisted('provisional_continuity')).toBe(true);
		expect(isUnlisted('continuity')).toBe(false);
		expect(isContinuity('provisional_continuity')).toBe(true);
		expect(isContinuity('named')).toBe(false);
		expect(continuityLine(1)).toBe('1 kill assigned on continuity');
		expect(continuityLine(4)).toBe('4 kills assigned on continuity');
		expect(killCount(1)).toBe('1 kill');
		expect(killCount(3)).toBe('3 kills');
	});

	it('tells groups apart by grade and by the level an unlisted name was read at', () => {
		const a = group();
		const b = group({ group: { ...a.group, provisionalKey: 'GLOOMRAT L9 #3' } });
		const c = group({ group: { ...a.group, grade: 'provisional_guess' } });
		expect(new Set([groupKey(a.group), groupKey(b.group), groupKey(c.group)]).size).toBe(3);
	});

	it('says what a settlement did', () => {
		const creature = { name: 'Gloomrat', species: 'Gloomrat', maturity: '' };
		expect(describeSettled({ kind: 'newName', creature })).toBe(
			'Recorded as Gloomrat, and known by that name from now on.',
		);
		expect(
			describeSettled({ kind: 'listed', creature: { ...creature, name: 'Young Atrox' } }),
		).toBe('Recorded as Young Atrox.');
		expect(describeSettled({ kind: 'declared' })).toBe('Put back to the declared mob.');
	});
});

describe('the detected creatures model', () => {
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
		answer(review('s1', [group()]));
		await loading;
		expect(h.model.groups).toEqual([]);

		mocked.getMobDetections.mockResolvedValueOnce(review('s2', [group({}, 'Snarl')]));
		await h.model.refresh();
		expect(h.model.groups.map((g) => g.group.creature.name)).toEqual(['Snarl']);
	});

	it('names a group with the typed name, applies the fresh review and tells the owner', async () => {
		const h = harness();
		const shown = group();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		expect(h.model.draft(shown)).toBe('Gloomrat');
		h.model.setDraft(shown, 'Gloomrat Prime');
		const settledGroup = group({ reviewed: true, currentName: 'Gloomrat Prime' });
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: {
				kind: 'newName',
				creature: { name: 'Gloomrat Prime', species: 'Gloomrat Prime', maturity: '' },
			},
			review: review('s1', [settledGroup]),
		});
		await h.model.name(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledWith('s1', shown.group, {
			kind: 'creature',
			name: 'Gloomrat Prime',
		});
		expect(h.model.groups).toEqual([settledGroup]);
		expect(h.model.notice).toBe('Recorded as Gloomrat Prime, and known by that name from now on.');
		expect(h.model.draft(settledGroup)).toBe('Gloomrat');
		expect(h.settled).toHaveBeenCalledTimes(1);
		expect(h.model.busy).toBeNull();
	});

	it('puts back the declared mob in one action for the whole group', async () => {
		const h = harness();
		const shown = group({
			group: { ...group().group, grade: 'latest_read', provisionalKey: null },
		});
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: { kind: 'declared' },
			review: review('s1', [{ ...shown, reviewed: true }]),
		});
		await h.model.restoreDeclared(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledWith('s1', shown.group, { kind: 'declared' });
		expect(h.model.notice).toBe('Put back to the declared mob.');
	});

	it('refuses a second settlement while one is saving, and shows a refusal without changing the list', async () => {
		const h = harness();
		const shown = group();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		let fail: (reason: Error) => void = () => {};
		mocked.settleMobDetection.mockReturnValueOnce(
			new Promise((_, reject) => {
				fail = reject;
			}),
		);
		const first = h.model.name(shown);
		expect(h.model.busy).toBe(groupKey(shown.group));
		await h.model.restoreDeclared(shown);
		expect(mocked.settleMobDetection).toHaveBeenCalledTimes(1);
		fail(new Error('"42" does not read as a creature\'s name'));
		await first;
		expect(h.model.error).toMatch(/does not read as a creature/);
		expect(h.model.groups).toEqual([shown]);
		expect(h.settled).not.toHaveBeenCalled();
		h.model.dismissError();
		expect(h.model.error).toBeNull();
	});

	it('shows nothing for a session still running', async () => {
		const h = harness();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [group()], false));
		await h.model.refresh();
		expect(h.model.groups).toEqual([]);
	});

	it('offers the creature a loosely typed name may mean, and records it in one action', async () => {
		const h = harness();
		const shown = group();
		mocked.getMobDetections.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		h.model.setDraft(shown, 'Atrok Yung');
		const meant = { name: 'Young Atrox', species: 'Atrox', maturity: 'Young' };
		mocked.settleMobDetection.mockResolvedValueOnce({ kind: 'didYouMean', creature: meant });
		await h.model.name(shown);
		expect(h.model.offerFor(shown)).toEqual(meant);
		expect(h.model.groups).toEqual([shown]);
		expect(h.settled).not.toHaveBeenCalled();
		mocked.settleMobDetection.mockResolvedValueOnce({
			kind: 'settled',
			settled: { kind: 'listed', creature: meant },
			review: review('s1', [{ ...shown, reviewed: true, currentName: 'Young Atrox' }]),
		});
		await h.model.acceptOffer(shown);
		expect(mocked.settleMobDetection).toHaveBeenLastCalledWith('s1', shown.group, {
			kind: 'creature',
			name: 'Young Atrox',
		});
		expect(h.model.offerFor(shown)).toBeNull();
		expect(h.model.notice).toBe('Recorded as Young Atrox.');
	});
});

describe('the words beside a name', () => {
	it('says keeping an unlisted spelling is keeping a new creature', () => {
		expect(nameAction('provisional', 'Gloomrat', 'Gloomrat')).toBe('Keep as a new creature');
		expect(nameAction('provisional_latest_read', ' Gloomrat ', 'Gloomrat')).toBe(
			'Keep as a new creature',
		);
		expect(nameAction('named', 'Young Atrox', 'Young Atrox')).toBe('Confirm');
		expect(nameAction('provisional', 'Young Atrox', 'Gloomrat')).toBe('Save');
	});

	it('says how sure a recorded creature is', () => {
		expect(describeCertainty('recorded')).toBeNull();
		expect(describeCertainty('probable')).toBe('probably');
		expect(describeCertainty('unlisted')).toBe('not in the creature list, unconfirmed');
		expect(describeCertainty('unlisted_probable')).toMatch(/^probably; not in the creature list/);
	});
});
