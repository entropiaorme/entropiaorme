import { describe, expect, it } from 'vitest';
import type { ToolEntry, ToolHeld, ToolSourceView } from '$lib/api';
import { NO_DATA } from '$lib/utils/format';
import {
	cueCopy,
	type HeldToolDisplay,
	heldToolDisplay,
	SCREEN_NOT_READ_MARKER,
	SCREEN_SHOWS_TOOL,
	screenNotReadLine,
	screenShowsTool,
	toolInHand,
} from './heldTool';

function reading(held: ToolHeld, manual = false): ToolSourceView {
	return { readingScreen: true, notReading: null, manual, held };
}

const keys: ToolSourceView = {
	readingScreen: false,
	notReading: null,
	manual: false,
	held: { kind: 'notRead' },
};

function entry(id: number, label: string, name = label): ToolEntry {
	return { id, name, label };
}

const CHOOSE = { kind: 'choose', label: 'Choose a tool' };

/** Words that belong to the implementation, never to what the player reads. */
const TELEMETRY =
	/pulse|regime|retention|catalogue|contended|provisional|generation|bundled|twins|notread|uncertain|hotbar intent|screen source/i;

/** Every state the feature can show, with an entry in each list. */
const EVERY_STATE: ToolSourceView[] = [
	reading({ kind: 'notRead' }),
	reading({ kind: 'tool', name: 'Pistol', provisional: true }),
	reading({ kind: 'tool', name: 'Pistol', provisional: false }),
	reading({ kind: 'tool', name: 'Pistol', provisional: false }, true),
	reading({ kind: 'nothing' }),
	reading({ kind: 'notTracked', name: 'Chip' }),
	reading({ kind: 'notVisible', last: 'Pistol', provisional: false }),
	reading({ kind: 'notVisible', last: null, provisional: false }),
	reading({ kind: 'uncertain', why: { kind: 'unreadable' } }),
	reading({ kind: 'uncertain', why: { kind: 'unknown', text: 'x' } }),
	reading({
		kind: 'uncertain',
		why: { kind: 'contended', candidates: [{ name: 'A', libraryId: 1 }], more: 1 },
	}),
	reading({ kind: 'uncertain', why: { kind: 'twins', entries: [entry(1, 'A'), entry(2, 'B')] } }),
	reading({ kind: 'uncertain', why: { kind: 'disabled', entries: [entry(1, 'A')] } }),
];

function allText(display: HeldToolDisplay | null): string[] {
	if (!display) return [];
	return [
		display.label,
		display.marker ?? '',
		...display.lines,
		...display.actions.flatMap((action) =>
			action.kind === 'declare' ? [action.label, action.accessibleName] : [action.label],
		),
	];
}

describe('heldToolDisplay', () => {
	it('says nothing while the keys are the source', () => {
		expect(heldToolDisplay(keys)).toBeNull();
		expect(heldToolDisplay(null)).toBeNull();
		expect(heldToolDisplay(undefined)).toBeNull();
		expect(heldToolDisplay({ ...keys, notReading: 'stopped' })).toBeNull();
	});

	it('shows a tool read off the screen by name, in the ordinary tone, with nothing to settle', () => {
		expect(heldToolDisplay(reading({ kind: 'tool', name: 'Pistol', provisional: false }))).toEqual({
			label: 'Pistol',
			tone: 'normal',
			marker: null,
			lines: [],
			actions: [],
		});
	});

	it('shows a tool known only from the game data in the provisional tone, saying how it is costed', () => {
		const display = heldToolDisplay(
			reading({ kind: 'tool', name: 'Rusty Rifle', provisional: true }),
		);
		expect(display?.label).toBe('Rusty Rifle');
		expect(display?.tone).toBe('provisional');
		expect(display?.lines).toEqual(['Not in your equipment: costed without attachments.']);
	});

	it('shows nothing held as a quiet "No tool", and a name not tracked, muted', () => {
		expect(heldToolDisplay(reading({ kind: 'nothing' }))).toMatchObject({
			label: 'No tool',
			tone: 'quiet',
			marker: null,
		});
		expect(heldToolDisplay(reading({ kind: 'notTracked', name: 'Chip' }))).toMatchObject({
			label: 'Chip',
			tone: 'muted',
			lines: ['Not a tracked tool.'],
		});
	});

	it('shows a quiet placeholder before the first look', () => {
		expect(heldToolDisplay(reading({ kind: 'notRead' }))).toMatchObject({
			label: NO_DATA,
			tone: 'quiet',
			lines: ['Not read yet.'],
		});
	});

	it('keeps the tool before a cover, marked, in its own tone, and offers the picker', () => {
		expect(
			heldToolDisplay(reading({ kind: 'notVisible', last: 'Pistol', provisional: false })),
		).toEqual({
			label: 'Pistol',
			tone: 'muted',
			marker: 'Covered',
			lines: ['The tool bar is covered.'],
			actions: [CHOOSE],
		});
		expect(
			heldToolDisplay(reading({ kind: 'notVisible', last: 'Rusty Rifle', provisional: true }))
				?.tone,
		).toBe('provisional');
		expect(
			heldToolDisplay(reading({ kind: 'notVisible', last: null, provisional: false }))?.label,
		).toBe(NO_DATA);
	});

	it('marks a tool the player chose, standing until the screen shows what is held', () => {
		expect(
			heldToolDisplay(reading({ kind: 'tool', name: 'Pistol', provisional: false }, true)),
		).toEqual({
			label: 'Pistol',
			tone: 'normal',
			marker: 'Chosen',
			lines: ['Chosen by you until the screen shows what is held.'],
			actions: [],
		});
	});

	describe('an unclear bar', () => {
		it('says at once that the tool is unclear, in the attention tone, and offers the picker', () => {
			expect(heldToolDisplay(reading({ kind: 'uncertain', why: { kind: 'unreadable' } }))).toEqual({
				label: 'Tool unclear',
				tone: 'attention',
				marker: null,
				lines: ['The tool bar cannot be read.'],
				actions: [CHOOSE],
			});
		});

		it('tells twins apart and names each button by the entry it takes', () => {
			const display = heldToolDisplay(
				reading({
					kind: 'uncertain',
					why: {
						kind: 'twins',
						entries: [entry(1, 'Pistol + A101', 'Pistol'), entry(2, 'Pistol')],
					},
				}),
			);
			expect(display?.lines).toEqual([
				'Two entries look the same on the bar: Pistol + A101 and Pistol.',
			]);
			expect(display?.actions).toEqual([
				{
					kind: 'declare',
					equipmentId: 1,
					name: 'Pistol + A101',
					label: 'Use this one',
					accessibleName: 'Use Pistol + A101',
				},
				{
					kind: 'declare',
					equipmentId: 2,
					name: 'Pistol',
					label: 'Use this one',
					accessibleName: 'Use Pistol',
				},
				CHOOSE,
			]);
		});

		it('says more than two twins, and one left, in plain words', () => {
			expect(
				heldToolDisplay(
					reading({
						kind: 'uncertain',
						why: { kind: 'twins', entries: [entry(1, 'A'), entry(2, 'B'), entry(3, 'C')] },
					}),
				)?.lines,
			).toEqual(['These entries look the same on the bar: A, B and C.']);
			const one = heldToolDisplay(
				reading({ kind: 'uncertain', why: { kind: 'twins', entries: [entry(1, 'A')] } }),
			);
			expect(one?.lines).toEqual(['Could be: A.']);
			expect(one?.actions[0]).toMatchObject({ label: 'Use it', accessibleName: 'Use A' });
		});

		it('says an entry is turned off and offers to use it', () => {
			const display = heldToolDisplay(
				reading({ kind: 'uncertain', why: { kind: 'disabled', entries: [entry(4, 'Rifle')] } }),
			);
			expect(display?.lines).toEqual(['Rifle is turned off in your equipment.']);
			expect(display?.actions[0]).toMatchObject({ equipmentId: 4, label: 'Use it' });
			const several = heldToolDisplay(
				reading({
					kind: 'uncertain',
					why: { kind: 'disabled', entries: [entry(4, 'Rifle (1)'), entry(5, 'Rifle (2)')] },
				}),
			);
			expect(several?.lines).toEqual(['Rifle (1) and Rifle (2) are turned off in your equipment.']);
			expect(several?.actions.map((action) => action.label)).toEqual([
				'Use this one',
				'Use this one',
				'Choose a tool',
			]);
		});

		it('falls back to the unreadable line when the entries named are gone', () => {
			for (const kind of ['twins', 'disabled'] as const) {
				const display = heldToolDisplay(reading({ kind: 'uncertain', why: { kind, entries: [] } }));
				expect(display?.lines).toEqual(['The tool bar cannot be read.']);
				expect(display?.actions).toEqual([CHOOSE]);
			}
			expect(
				heldToolDisplay(
					reading({ kind: 'uncertain', why: { kind: 'contended', candidates: [], more: 0 } }),
				)?.lines,
			).toEqual(['The tool bar cannot be read.']);
		});

		it('lists what it could be, offering only library entries to use', () => {
			const display = heldToolDisplay(
				reading({
					kind: 'uncertain',
					why: {
						kind: 'contended',
						candidates: [
							{ name: 'Pistol', libraryId: 1 },
							{ name: 'Pistol Mk2', libraryId: null },
						],
						more: 3,
					},
				}),
			);
			expect(display?.lines).toEqual(['Could be: Pistol, Pistol Mk2 (and 3 more).']);
			expect(display?.actions).toEqual([
				{
					kind: 'declare',
					equipmentId: 1,
					name: 'Pistol',
					label: 'Use this one',
					accessibleName: 'Use Pistol',
				},
				CHOOSE,
			]);
		});

		it('quotes what it read', () => {
			expect(
				heldToolDisplay(reading({ kind: 'uncertain', why: { kind: 'unknown', text: 'Pstl' } }))
					?.lines,
			).toEqual(['Read as "Pstl".']);
		});
	});

	it('never says an implementation word to the player, in any string the feature shows', () => {
		const shown = [
			...EVERY_STATE.flatMap((state) => allText(heldToolDisplay(state))),
			screenNotReadLine({ ...keys, notReading: 'stopped' }) ?? '',
			screenNotReadLine({ ...keys, notReading: 'notStarted' }) ?? '',
			SCREEN_NOT_READ_MARKER,
			SCREEN_SHOWS_TOOL,
			cueCopy(true).weaponMismatch('A', 'B'),
			cueCopy(true).confirm('A'),
			cueCopy(true).harvestMismatch('A', null),
			cueCopy(true).hit('boosted'),
			cueCopy(true).hit('reduced'),
		];
		expect(shown.length).toBeGreaterThan(40);
		for (const text of shown) expect(text).not.toMatch(TELEMETRY);
	});
});

describe('the screen not being read', () => {
	it('says why, in the player terms, only while reading is on and the keys are the source', () => {
		expect(screenNotReadLine({ ...keys, notReading: 'stopped' })).toBe(
			'The screen stopped being read: hotbar keys are in use.',
		);
		expect(screenNotReadLine({ ...keys, notReading: 'notStarted' })).toBe(
			'Screen capture is not running: turn the setting off and on to start it. Hotbar keys are in use.',
		);
		expect(screenNotReadLine(keys)).toBeNull();
		expect(screenNotReadLine(null)).toBeNull();
	});
});

describe('screenShowsTool', () => {
	it('is true only while the screen shows outright what is held', () => {
		expect(screenShowsTool(reading({ kind: 'tool', name: 'P', provisional: false }))).toBe(true);
		expect(screenShowsTool(reading({ kind: 'nothing' }))).toBe(true);
		expect(screenShowsTool(reading({ kind: 'notTracked', name: 'C' }))).toBe(true);
		expect(screenShowsTool(reading({ kind: 'tool', name: 'P', provisional: false }, true))).toBe(
			false,
		);
		expect(screenShowsTool(reading({ kind: 'uncertain', why: { kind: 'unreadable' } }))).toBe(
			false,
		);
		expect(screenShowsTool(reading({ kind: 'notVisible', last: 'P', provisional: false }))).toBe(
			false,
		);
		expect(screenShowsTool(reading({ kind: 'notRead' }))).toBe(false);
		expect(screenShowsTool(keys)).toBe(false);
	});
});

describe('cueCopy', () => {
	it('keeps the hotbar words with the keys the source', () => {
		const copy = cueCopy(false);
		expect(copy.weaponMismatch('Rifle', 'Pistol')).toBe(
			'Damage fits Rifle; the hotbar shows Pistol',
		);
		expect(copy.confirm('Rifle')).toBe(
			'Confirm Rifle: record it from now and reprice the shots since your last hotbar press',
		);
		expect(copy.harvestMismatch('Axe', null)).toBe('Board output says Axe; hotbar shows no tool');
		expect(copy.hit('boosted')).toBe(
			"Recent hits are above this weapon's listed damage, so they are priced as boosted. If you have no damage buff running, press the slot of the weapon you are firing.",
		);
		expect(copy.hit('reduced')).toBe(
			"Recent hits are below this weapon's listed damage, so they are priced as reduced. If that looks wrong, press the slot of the weapon you are firing.",
		);
	});

	it('names the screen, and the set-up to check, while the screen is read', () => {
		const copy = cueCopy(true);
		expect(copy.weaponMismatch('Rifle', 'Pistol')).toBe(
			'Damage fits Rifle; the screen shows Pistol',
		);
		expect(copy.harvestMismatch('Axe', 'Finder')).toBe(
			'Board output says Axe; the screen shows Finder',
		);
		expect(copy.hit('boosted')).toMatch(/check which set-up is on in Equipment\.$/);
		for (const text of [copy.confirm('R'), copy.hit('reduced')])
			expect(text).not.toMatch(/hotbar|slot/);
	});
});

describe('toolInHand', () => {
	it('is what the screen shows while it is the source, never a weapon guessed from damage', () => {
		expect(toolInHand(reading({ kind: 'tool', name: 'Rifle', provisional: false }), 'Pistol')).toBe(
			'Rifle',
		);
		expect(toolInHand(reading({ kind: 'nothing' }), 'Pistol')).toBeNull();
		expect(
			toolInHand(reading({ kind: 'uncertain', why: { kind: 'unreadable' } }), 'Pistol'),
		).toBeNull();
		expect(
			toolInHand(reading({ kind: 'notVisible', last: 'Rifle', provisional: false }), 'Pistol'),
		).toBe('Rifle');
	});

	it('is the readout tool while the keys are the source', () => {
		expect(toolInHand(keys, 'Pistol')).toBe('Pistol');
		expect(toolInHand(null, undefined)).toBeNull();
	});
});
