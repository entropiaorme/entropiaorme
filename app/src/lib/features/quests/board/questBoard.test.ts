import { describe, expect, it } from 'vitest';
import { CATALOGUE, curatedRows, family, HOUR, NOW, quest, unresolved } from './boardFixtures';
import type { BoardInput, BoardPlanet, BoardRow } from './questBoard';
import { buildBoard, formatBackAt, unplacedRewards, variantOf, waypointLabel } from './questBoard';

function input(overrides: Partial<BoardInput> = {}): BoardInput {
	const rows = curatedRows();
	return {
		catalogue: CATALOGUE,
		quests: rows.quests,
		families: rows.families,
		unresolved: [],
		recording: new Set(),
		waiting: new Set(),
		now: NOW,
		...overrides,
	};
}

function planet(board: BoardPlanet[], name: string): BoardPlanet {
	const found = board.find((entry) => entry.name === name);
	if (!found) throw new Error(`no ${name} on the board`);
	return found;
}

function row(board: BoardPlanet[], key: string): BoardRow {
	for (const entry of board) {
		for (const section of entry.sections) {
			const found = section.rows.find((candidate) => candidate.key === key);
			if (found) return found;
		}
	}
	throw new Error(`no row ${key}`);
}

const iso = (ms: number) => new Date(ms).toISOString();

describe('the board layout', () => {
	it('follows the catalogue: planets, categories and entries in authored order', () => {
		const board = buildBoard(input());
		expect(board.map((entry) => entry.name)).toEqual(['ARIS', 'Cyrene']);
		const aris = planet(board, 'ARIS');
		expect(aris.sections.map((section) => section.name)).toEqual(['Daily missions', 'Bosses']);
		expect(aris.sections[0].rows.map((entry) => entry.label)).toEqual(['Hunting 1', 'Samples']);
	});

	it('states a reward once when every row of a section shares it', () => {
		const board = buildBoard(input());
		const missions = planet(board, 'ARIS').sections[0];
		expect(missions.rewardNote).toBe('Hyperion voucher');
		expect(missions.rows.every((entry) => entry.rewardLabel === null)).toBe(true);
		const tokens = planet(board, 'Cyrene').sections[0];
		expect(tokens.rewardNote).toBeNull();
		expect(tokens.rows.map((entry) => entry.rewardLabel)).toEqual(['2 tokens', '1 token']);
	});

	it("puts the player's own quests after the curated ones, and gives a custom-only planet its own tab", () => {
		const rows = curatedRows();
		const board = buildBoard(
			input({
				quests: [
					...rows.quests,
					quest({ id: '50', name: 'My Cyrene run', planet: 'Cyrene', category: 'Mine' }),
					quest({
						id: '60',
						name: 'AI Daily: Boris',
						planet: 'Calypso',
						category: 'Caly AI Dailies',
					}),
					quest({ id: '61', name: 'test', planet: 'Calypso' }),
				],
			}),
		);
		expect(board.map((entry) => entry.name)).toEqual(['ARIS', 'Cyrene', 'Calypso']);
		const cyrene = planet(board, 'Cyrene');
		expect(cyrene.sections.map((section) => [section.name, section.custom])).toEqual([
			['Mission Tokens', false],
			['Mine', true],
		]);
		const calypso = planet(board, 'Calypso');
		expect(calypso.curated).toBe(false);
		expect(calypso.sections.map((section) => section.name)).toEqual([
			'Caly AI Dailies',
			'Your quests',
		]);
		expect(row(board, 'quest:60').custom).toBe(true);
	});

	it('skips a catalogue entry whose row has not synced', () => {
		const rows = curatedRows();
		const board = buildBoard(
			input({ quests: rows.quests.filter((entry) => entry.catalogueKey !== 'aris/verdathos') }),
		);
		expect(planet(board, 'ARIS').sections.map((section) => section.name)).toEqual([
			'Daily missions',
		]);
	});
});

describe('a slot', () => {
	it('is ready to pick up when nothing gates it, naming no variant', () => {
		const slot = row(buildBoard(input()), 'aris/hunting-1');
		expect(slot.kind).toBe('slot');
		expect(slot.state).toBe('ready');
		expect(slot.variant).toBeNull();
		expect(slot.waypoint).toBeNull();
	});

	it("names the day's variant, its waypoint, and whether the overlay records it", () => {
		const rows = curatedRows();
		rows.quests[0] = { ...rows.quests[0], startedAt: NOW / 1000 - 600 };
		const running = row(buildBoard(input({ quests: rows.quests })), 'aris/hunting-1');
		expect(running.state).toBe('running');
		expect(running.variant).toBe('Weak Mortirex');
		expect(running.waypoint).toContain('Weak Mortirex Meekling');
		expect(running.quest?.id).toBe('10');
		const recording = row(
			buildBoard(input({ quests: rows.quests, recording: new Set(['10']) })),
			'aris/hunting-1',
		);
		expect(recording.state).toBe('recording');
	});

	it('cools as one slot, naming the variant last done and when it is back', () => {
		const rows = curatedRows();
		rows.families[0] = { ...rows.families[0], cooldownExpiresAt: iso(NOW + 5 * HOUR) };
		rows.quests[1] = {
			...rows.quests[1],
			lastStartedAt: NOW / 1000 - 3600,
			lastCompletedAt: NOW / 1000 - 1800,
		};
		const board = buildBoard(input({ quests: rows.quests, families: rows.families }));
		const slot = row(board, 'aris/hunting-1');
		expect(slot.state).toBe('cooling');
		expect(slot.backAt).toBe(NOW + 5 * HOUR);
		expect(slot.variant).toBe('Defective Drones');
		expect(slot.abandoned).toBe(false);
		expect(planet(board, 'ARIS').sections[0].done).toBe(1);
	});

	it('says when the day was given up rather than finished', () => {
		const rows = curatedRows();
		rows.families[0] = { ...rows.families[0], cooldownExpiresAt: iso(NOW + HOUR) };
		rows.quests[0] = {
			...rows.quests[0],
			lastStartedAt: NOW / 1000 - 900,
			lastCompletedAt: NOW / 1000 - 86_400,
			lastAbandonedAt: NOW / 1000 - 600,
		};
		const slot = row(
			buildBoard(input({ quests: rows.quests, families: rows.families })),
			'aris/hunting-1',
		);
		expect(slot.abandoned).toBe(true);
		expect(slot.variant).toBe('Weak Mortirex');
	});

	it('includes a variant the chat log created before it was curated', () => {
		const rows = curatedRows();
		rows.quests.push(
			quest({
				id: '12',
				name: 'ARIS - Daily Hunting 1: New Beasts',
				planet: 'ARIS',
				familyId: '1',
				startedAt: NOW / 1000,
			}),
		);
		const board = buildBoard(input({ quests: rows.quests }));
		expect(row(board, 'aris/hunting-1').variant).toBe('New Beasts');
		expect(board.flatMap((entry) => entry.sections).some((section) => section.custom)).toBe(false);
	});

	it("carries every member's rewards still to settle, newest first", () => {
		const board = buildBoard(
			input({
				unresolved: [
					unresolved({ completionId: 1, questId: '10', completedAt: 100 }),
					unresolved({ completionId: 2, questId: '11', completedAt: 200 }),
					unresolved({ completionId: 3, questId: '30' }),
				],
			}),
		);
		expect(row(board, 'aris/hunting-1').unresolved.map((reward) => reward.completionId)).toEqual([
			2, 1,
		]);
		expect(row(board, 'cyrene/getting-slimed').unresolved).toHaveLength(1);
	});
});

describe('a quest', () => {
	it('names who hands a mission out, from its waypoint', () => {
		const board = buildBoard(input());
		expect(row(board, 'cyrene/getting-slimed').giver).toBe('Captain Wrigley');
		expect(row(board, 'cyrene/lord-pyscidon').giver).toBeNull();
		expect(row(board, 'aris/verdathos').giver).toBeNull();
	});

	it('cools on the later of its own and its family window, and says when it was abandoned', () => {
		const rows = curatedRows();
		rows.quests[4] = {
			...rows.quests[4],
			cooldownExpiresAt: iso(NOW + 2 * HOUR),
			lastAbandonedAt: NOW / 1000 - 60,
		};
		const pyscidon = row(buildBoard(input({ quests: rows.quests })), 'cyrene/lord-pyscidon');
		expect(pyscidon.state).toBe('cooling');
		expect(pyscidon.backAt).toBe(NOW + 2 * HOUR);
		expect(pyscidon.abandoned).toBe(true);
	});

	it('is ready again once its window has passed', () => {
		const rows = curatedRows();
		rows.quests[4] = { ...rows.quests[4], cooldownExpiresAt: iso(NOW - 1) };
		expect(row(buildBoard(input({ quests: rows.quests })), 'cyrene/lord-pyscidon').state).toBe(
			'ready',
		);
	});

	it('waits for its reward clump when a hand-in is armed', () => {
		const rows = curatedRows();
		rows.quests[2] = { ...rows.quests[2], startedAt: NOW / 1000 };
		const boss = row(
			buildBoard(input({ quests: rows.quests, waiting: new Set(['20']) })),
			'aris/verdathos',
		);
		expect(boss.state).toBe('waiting');
	});

	it('keeps a custom family as one slot row', () => {
		const board = buildBoard(
			input({
				families: [
					...curatedRows().families,
					family({
						id: '9',
						name: 'My Daily',
						planet: 'Calypso',
						curated: false,
						catalogueKey: null,
					}),
				],
				quests: [
					...curatedRows().quests,
					quest({ id: '70', name: 'My Daily: One', planet: 'Calypso', familyId: '9' }),
					quest({ id: '71', name: 'My Daily: Two', planet: 'Calypso', familyId: '9' }),
				],
			}),
		);
		const calypso = planet(board, 'Calypso');
		expect(calypso.sections[0].rows.map((entry) => [entry.kind, entry.label])).toEqual([
			['slot', 'My Daily'],
		]);
	});
});

describe('rewards no row carries', () => {
	it("lists a deleted quest's rewards apart, newest first", () => {
		const rewards = [
			unresolved({ completionId: 1, questId: '10' }),
			unresolved({ completionId: 2, questId: '35', completedAt: 100 }),
			unresolved({ completionId: 3, questId: '35', completedAt: 200 }),
		];
		const board = buildBoard(input({ unresolved: rewards }));
		expect(unplacedRewards(board, rewards).map((reward) => reward.completionId)).toEqual([3, 2]);
	});
});

describe('formatting', () => {
	it('reads the variant and the waypoint label', () => {
		expect(variantOf('ARIS - Daily Hunting 2: Derelict Warriors')).toBe('Derelict Warriors');
		expect(variantOf('Standalone')).toBe('Standalone');
		expect(waypointLabel("/wp [Planet Cyrene, 1, 2, 3, Zyn'Kimbro Farm Hand]")).toBe(
			"Zyn'Kimbro Farm Hand",
		);
		expect(waypointLabel('/wp [Aris, 1, 2, 3, Mortirex Young - Adult]')).toBe(
			'Mortirex Young - Adult',
		);
		expect(waypointLabel('/wp 1,2')).toBeNull();
		expect(waypointLabel(null)).toBeNull();
	});

	it('says when a gate lifts the way a player plans around it', () => {
		const now = new Date(2026, 9, 17, 12, 0).getTime();
		const today = formatBackAt(new Date(2026, 9, 17, 18, 30).getTime(), now);
		expect(today).not.toContain('tomorrow');
		expect(formatBackAt(new Date(2026, 9, 18, 9, 0).getTime(), now)).toMatch(/^tomorrow /);
		expect(formatBackAt(new Date(2026, 9, 20, 9, 0).getTime(), now)).not.toMatch(/^tomorrow/);
	});
});
