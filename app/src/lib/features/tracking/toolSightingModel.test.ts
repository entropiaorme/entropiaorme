import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ToolSighting, ToolSightingReview } from '$lib/api';
import {
	canAdd,
	describeChange,
	describeKept,
	describeRefusal,
	describeSettled,
	describeSighted,
	entryToTurnOn,
	sightingName,
} from './toolSighting';
import { createToolSightingModel } from './toolSightingModel.svelte';

vi.mock('$lib/api', () => ({
	getToolSightings: vi.fn(),
	settleToolSighting: vi.fn(),
	undoToolSighting: vi.fn(),
	setLibraryEnabled: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function weapon(overrides: Partial<ToolSighting> = {}): ToolSighting {
	return {
		id: 't1',
		sighted: { kind: 'bundled', name: 'Sollomate Opalo', tool: 'weapon', shots: 212 },
		firstSeen: 1000,
		lastSeen: 2000,
		settled: null,
		...overrides,
	};
}

function unknown(overrides: Partial<ToolSighting> = {}): ToolSighting {
	return {
		id: 't2',
		sighted: { kind: 'unknown', text: 'Teleport Chip' },
		firstSeen: 1500,
		lastSeen: 1500,
		settled: null,
		...overrides,
	};
}

function entry(label: string, enabled = true) {
	return { id: '9', label, enabled };
}

function review(sessionId: string, sightings: ToolSighting[], ended = true): ToolSightingReview {
	return { sessionId, ended, sightings };
}

function harness(session = 's1') {
	let current = session;
	const model = createToolSightingModel({ sessionId: () => current });
	return {
		model,
		switchTo(next: string) {
			current = next;
		},
	};
}

describe('what the review says', () => {
	it('names a tool by its name and a name read by the text, quoted', () => {
		expect(sightingName(weapon())).toBe('Sollomate Opalo');
		expect(sightingName(unknown())).toBe('“Teleport Chip”');
	});

	it('says what happened while it was in hand, by kind', () => {
		expect(describeSighted(weapon().sighted)).toBe(
			'Held for 212 shots, costed without attachments',
		);
		expect(describeSighted({ kind: 'bundled', name: 'A', tool: 'weapon', shots: 1 })).toBe(
			'Held for 1 shot, costed without attachments',
		);
		expect(describeSighted({ kind: 'bundled', name: 'A', tool: 'weapon', shots: 0 })).toBe(
			'Held, no shots recorded',
		);
		expect(describeSighted({ kind: 'bundled', name: 'A', tool: 'weapon', shots: null })).toBe(
			'Held, costed without attachments; shots not counted yet',
		);
		expect(describeSighted({ kind: 'bundled', name: 'A', tool: 'healing', shots: null })).toBe(
			'Held; its heals were not costed',
		);
		expect(describeSighted({ kind: 'bundled', name: 'A', tool: 'harvesting', shots: null })).toBe(
			'Held, costed at its catalogue cost',
		);
		expect(describeSighted(unknown().sighted)).toBe('Seen on the tool bar');
	});

	it('offers adding only for a tool still to be settled, once the session has ended', () => {
		expect(canAdd(weapon(), true)).toBe(true);
		expect(canAdd(weapon(), false)).toBe(false);
		expect(canAdd(unknown(), true)).toBe(false);
		expect(canAdd(weapon({ settled: { kind: 'left' } }), true)).toBe(false);
	});

	it('collapses a settled one to a quiet line', () => {
		expect(describeSettled({ kind: 'added', created: true })).toBe('Added to equipment');
		expect(describeSettled({ kind: 'added', created: false })).toBe('Already in equipment');
		expect(describeSettled({ kind: 'dismissed' })).toBe('Not a tool you track');
		expect(describeSettled({ kind: 'left' })).toBe('Left');
	});

	it('never claims an added tool made the session exact', () => {
		const weaponAdded = describeChange(
			{ kind: 'added', created: true, uses: 'withoutAttachments', entry: entry('Sollomate Opalo') },
			'Sollomate Opalo',
		);
		expect(weaponAdded).toContain('Sollomate Opalo added to equipment.');
		expect(weaponAdded).toContain('stays costed without attachments');
		const healerAdded = describeChange(
			{ kind: 'added', created: false, uses: 'notCosted', entry: entry('Vivo T1 + Implant') },
			'Vivo T1',
		);
		expect(healerAdded).toContain('Vivo T1 + Implant is already in equipment.');
		expect(healerAdded).toContain('heals this session were not costed');
		const harvesterAdded = describeChange(
			{ kind: 'added', created: true, uses: 'asBooked', entry: entry('Ziplex Z1') },
			'Ziplex Z1',
		);
		expect(harvesterAdded).toBe(
			'Ziplex Z1 added to equipment. This session stays costed as it was.',
		);
		expect(harvesterAdded).not.toContain('attachments');
		const off = {
			kind: 'added',
			created: false,
			uses: 'withoutAttachments',
			entry: entry('Opalo', false),
		} as const;
		expect(describeChange(off, 'Opalo')).toContain('turned off, so it reads as unclear');
		expect(entryToTurnOn(off)).toEqual({ id: '9', label: 'Opalo' });
		expect(
			entryToTurnOn({ kind: 'added', created: true, uses: 'asBooked', entry: entry('Z') }),
		).toBeNull();
		expect(describeChange({ kind: 'left' }, 'X')).toBeNull();
		expect(describeChange({ kind: 'dismissed' }, 'X')).toBe('X will not be asked about again.');
		expect(
			describeChange({ kind: 'reopened', was: 'added', entry: { kind: 'removed' } }, 'X'),
		).toBe('X removed from equipment.');
		expect(
			describeChange(
				{ kind: 'reopened', was: 'added', entry: { kind: 'kept', because: 'inSession' } },
				'X',
			),
		).toBe('X stays in equipment: a session may have used it since.');
		expect(
			describeChange({ kind: 'reopened', was: 'left', entry: { kind: 'none' } }, 'X'),
		).toBeNull();
	});

	it('gives each reason an entry stays its own line', () => {
		const reasons = [
			'changed',
			'notInGameData',
			'otherSighting',
			'inSession',
			'referenced',
			'inSettings',
		] as const;
		const said = reasons.map((because) => describeKept(because));
		expect(new Set(said).size).toBe(reasons.length);
		expect(describeKept('inSettings')).toBe('it is on the hotbar or among the carried tools');
	});

	it('gives every refusal in the player’s words', () => {
		const reasons = [
			{ kind: 'notFound' },
			{ kind: 'otherSession' },
			{ kind: 'alreadySettled', settled: 'added' },
			{ kind: 'alreadySettled', settled: 'dismissed' },
			{ kind: 'alreadySettled', settled: 'left' },
			{ kind: 'notSettled' },
			{ kind: 'sessionRunning' },
			{ kind: 'notATool' },
			{ kind: 'notInGameData' },
			{ kind: 'needsSetUp' },
		] as const;
		const said = reasons.map((reason) => describeRefusal(reason));
		expect(new Set(said).size).toBe(reasons.length);
		expect(said.every((line) => line.length > 0 && !line.includes('_'))).toBe(true);
		expect(describeRefusal({ kind: 'sessionRunning' })).toBe(
			'Tools can be added once the session has ended.',
		);
	});
});

describe('the detected tools model', () => {
	beforeEach(() => vi.clearAllMocks());

	it('shows the session’s sightings once read, and drops a read for a session left', async () => {
		const h = harness();
		let resolve!: (r: ToolSightingReview) => void;
		mocked.getToolSightings.mockReturnValueOnce(
			new Promise((r) => {
				resolve = r;
			}),
		);
		const stale = h.model.refresh();
		h.switchTo('s2');
		resolve(review('s1', [weapon()]));
		await stale;
		expect(h.model.sightings).toEqual([]);

		mocked.getToolSightings.mockResolvedValueOnce(review('s2', [unknown()], false));
		await h.model.refresh();
		expect(h.model.sightings).toEqual([unknown()]);
		expect(h.model.ended).toBe(false);
	});

	it('applies a settlement’s refreshed review and says what it did', async () => {
		const h = harness();
		const shown = weapon();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		const added = weapon({ settled: { kind: 'added', created: true } });
		mocked.settleToolSighting.mockResolvedValueOnce({
			kind: 'settled',
			change: {
				kind: 'added',
				created: true,
				uses: 'withoutAttachments',
				entry: entry('Sollomate Opalo'),
			},
			review: review('s1', [added]),
		});
		await h.model.add(shown);
		expect(mocked.settleToolSighting).toHaveBeenCalledWith('s1', 't1', 'add');
		expect(h.model.sightings).toEqual([added]);
		expect(h.model.notice).toContain('Sollomate Opalo added to equipment.');
		expect(h.model.busy).toBeNull();

		mocked.undoToolSighting.mockResolvedValueOnce({
			kind: 'settled',
			change: { kind: 'reopened', was: 'added', entry: { kind: 'removed' } },
			review: review('s1', [shown]),
		});
		await h.model.undo(added);
		expect(mocked.undoToolSighting).toHaveBeenCalledWith('s1', 't1');
		expect(h.model.sightings).toEqual([shown]);
		expect(h.model.notice).toBe('Sollomate Opalo removed from equipment.');
	});

	it('says a refusal and shows the review as it now stands', async () => {
		const h = harness();
		const shown = weapon();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		mocked.settleToolSighting.mockResolvedValueOnce({
			kind: 'refused',
			reason: { kind: 'alreadySettled', settled: 'left' },
		});
		// Another window left it meanwhile.
		const left = weapon({ settled: { kind: 'left' } });
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [left]));
		await h.model.leave(shown);
		expect(h.model.error).toBe('Already left.');
		expect(h.model.sightings).toEqual([left]);
		h.model.dismissError();
		expect(h.model.error).toBeNull();
	});

	it('reads a running session again as the tracker moves on, until it has ended', async () => {
		const h = harness();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [weapon()], false));
		await h.model.refresh();
		const ended = weapon({
			sighted: { kind: 'bundled', name: 'Sollomate Opalo', tool: 'weapon', shots: 230 },
		});
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [ended], true));
		await h.model.followRunning();
		expect(h.model.ended).toBe(true);
		expect(h.model.sightings).toEqual([ended]);
		await h.model.followRunning();
		expect(mocked.getToolSightings).toHaveBeenCalledTimes(2);
	});

	it('offers to turn on a turned-off entry an addition linked to', async () => {
		const h = harness();
		const shown = weapon();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		mocked.settleToolSighting.mockResolvedValueOnce({
			kind: 'settled',
			change: {
				kind: 'added',
				created: false,
				uses: 'withoutAttachments',
				entry: entry('Sollomate Opalo + A101', false),
			},
			review: review('s1', [weapon({ settled: { kind: 'added', created: false } })]),
		});
		await h.model.add(shown);
		expect(h.model.offer).toEqual({ id: '9', label: 'Sollomate Opalo + A101' });
		mocked.setLibraryEnabled.mockResolvedValueOnce(
			{} as Awaited<ReturnType<typeof api.setLibraryEnabled>>,
		);
		await h.model.turnOn();
		expect(mocked.setLibraryEnabled).toHaveBeenCalledWith('9', true);
		expect(h.model.offer).toBeNull();
		expect(h.model.notice).toBe('Sollomate Opalo + A101 is on.');
	});

	it('refuses a second action while one is being saved', async () => {
		const h = harness();
		const shown = unknown();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		let finish!: () => void;
		mocked.settleToolSighting.mockReturnValueOnce(
			new Promise((r) => {
				finish = () =>
					r({
						kind: 'settled',
						change: { kind: 'dismissed' },
						review: review('s1', [unknown({ settled: { kind: 'dismissed' } })]),
					});
			}),
		);
		const first = h.model.dismiss(shown);
		expect(h.model.busy).toBe('t2');
		await h.model.leave(shown);
		expect(mocked.settleToolSighting).toHaveBeenCalledTimes(1);
		finish();
		await first;
		expect(h.model.notice).toBe('“Teleport Chip” will not be asked about again.');
	});

	it('drops a settlement answered after the player left the session', async () => {
		const h = harness();
		const shown = unknown();
		mocked.getToolSightings.mockResolvedValueOnce(review('s1', [shown]));
		await h.model.refresh();
		let finish!: () => void;
		mocked.settleToolSighting.mockReturnValueOnce(
			new Promise((r) => {
				finish = () =>
					r({
						kind: 'settled',
						change: { kind: 'left' },
						review: review('s1', [unknown({ settled: { kind: 'left' } })]),
					});
			}),
		);
		const pending = h.model.leave(shown);
		h.switchTo('s2');
		finish();
		await pending;
		h.switchTo('s1');
		expect(h.model.sightings).toEqual([shown]);
	});
});
