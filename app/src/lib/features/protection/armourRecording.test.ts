import { describe, expect, it } from 'vitest';
import type { ProtectionCandidateSession } from '$lib/api';
import {
	chosenSessions,
	EMPTY_SELECTION,
	groupBySessionType,
	groupState,
	lookBack,
	parseAmount,
	projectedShare,
	startAt,
	toggleEarlier,
	toggleGroup,
	toggleSession,
	totalHits,
} from './armourRecording';

function session(
	id: string,
	hitCount: number,
	definition: [string, string] | null,
	sessionName: string | null = definition?.[1] ?? null,
): ProtectionCandidateSession {
	return {
		sessionId: id,
		sessionName,
		definitionId: definition?.[0] ?? null,
		definitionName: definition?.[1] ?? null,
		startedAt: 0,
		endedAt: null,
		hitCount,
		covered: false,
	};
}

const aris: [string, string] = ['2', 'ARIS Dailies'];
const trees: [string, string] = ['3', 'Tree Cutting'];
const sessions = [
	session('a1', 30, aris),
	session('t1', 50, trees),
	session('a2', 10, aris),
	session('tag', 5, null, 'Old tag'),
	session('none', 1, null, null),
];

describe('grouping by session type', () => {
	it('groups by definition, then by a legacy name, then into other sessions, in first-seen order', () => {
		const groups = groupBySessionType(sessions);
		expect(groups.map((group) => group.label)).toEqual([
			'ARIS Dailies',
			'Tree Cutting',
			'Old tag',
			'Other sessions',
		]);
		expect(groups[0].sessions.map((s) => s.sessionId)).toEqual(['a1', 'a2']);
	});
});

describe('the selection', () => {
	it('starts with every look-back session ticked and no earlier one', () => {
		expect(chosenSessions(sessions, [session('old', 3, aris)], EMPTY_SELECTION)).toHaveLength(5);
	});

	it('unticks and reticks a whole group, reporting partial state in between', () => {
		const [arisGroup] = groupBySessionType(sessions);
		let selection = EMPTY_SELECTION;
		const chosen = () => new Set(chosenSessions(sessions, [], selection).map((s) => s.sessionId));
		expect(groupState(arisGroup, chosen())).toBe('all');
		selection = toggleSession(selection, 'a2');
		expect(groupState(arisGroup, chosen())).toBe('some');
		selection = toggleGroup(selection, arisGroup, chosen());
		expect(groupState(arisGroup, chosen())).toBe('all');
		selection = toggleGroup(selection, arisGroup, chosen());
		expect(groupState(arisGroup, chosen())).toBe('none');
	});

	it('drops sessions before a later start but remembers what was unticked after it', () => {
		let selection = toggleSession(EMPTY_SELECTION, 'a2');
		selection = startAt(selection, 1);
		expect(lookBack(sessions, selection).map((s) => s.sessionId)).toEqual([
			't1',
			'a2',
			'tag',
			'none',
		]);
		expect(chosenSessions(sessions, [], selection).map((s) => s.sessionId)).toEqual([
			't1',
			'tag',
			'none',
		]);
		selection = startAt(selection, 0);
		expect(chosenSessions(sessions, [], selection).map((s) => s.sessionId)).toEqual([
			'a1',
			't1',
			'tag',
			'none',
		]);
	});

	it('never starts past the newest session', () => {
		expect(lookBack(sessions, startAt(EMPTY_SELECTION, 99)).map((s) => s.sessionId)).toEqual([
			'none',
		]);
	});

	it('re-includes earlier sessions one by one, oldest first', () => {
		const earlier = [session('old', 3, aris)];
		const selection = toggleEarlier(EMPTY_SELECTION, 'old');
		expect(chosenSessions(sessions, earlier, selection)[0].sessionId).toBe('old');
		expect(chosenSessions(sessions, earlier, toggleEarlier(selection, 'old'))).toHaveLength(5);
	});
});

describe('weighting', () => {
	it('spreads by hits', () => {
		expect(totalHits(sessions)).toBe(96);
		expect(projectedShare(24, 40, 80)).toBe(12);
		expect(projectedShare(24, 40, 0)).toBe(0);
	});
});

describe('amount parsing', () => {
	it('accepts a decimal comma and refuses what is not a non-negative number', () => {
		expect(parseAmount('12,5')).toBe(12.5);
		expect(parseAmount(' 3 ')).toBe(3);
		expect(parseAmount('')).toBeNull();
		expect(parseAmount('-1')).toBeNull();
		expect(parseAmount('abc')).toBeNull();
	});
});
