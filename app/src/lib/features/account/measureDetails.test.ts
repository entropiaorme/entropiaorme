import { describe, expect, it } from 'vitest';
import type { MeasuredAmount, MeasuredEvent } from '$lib/api/commands.gen';
import { formatDate, formatTime } from '$lib/utils/format';
import { carried, exact, measure, SPAN, wholePec } from './__fixtures__/measure';
import {
	ammoLines,
	consumptionLines,
	eventLines,
	eventSentence,
	hasDetails,
	listsWholePec,
	measureClock,
	placeName,
	readingClock,
	repairLine,
	repairLines,
	wearGroups,
} from './measureDetails';

describe('the notable events in plain words', () => {
	it('names an opening, its ammo, and its certain and possible contents', () => {
		const opening: MeasuredEvent = {
			kind: 'strongboxOpening',
			between: SPAN,
			boxes: [{ name: 'Halloween Strongbox', count: 7 }],
			keysUsed: 7,
			keys: exact(70),
			ammoUnits: 600000,
			ammoRows: 1,
			ammoFromRows: wholePec(60),
			ammoFromKeys: wholePec(60),
			contents: [{ item: carried('Skeleton Pants Red (F)'), units: 1, amount: exact(10) }],
			possibleContents: [],
			settlement: 'settled',
		};
		expect(eventSentence(opening)).toBe(
			'Opened 7 Halloween Strongbox with 7 keys: 60.00 PED ammo, Skeleton Pants Red (F)',
		);
	});

	it('names every kind of box an unsettled opening may have used, and gives no ammo figure', () => {
		const opening: MeasuredEvent = {
			kind: 'strongboxOpening',
			between: SPAN,
			boxes: [
				{ name: 'Halloween Strongbox', count: 3 },
				{ name: 'Summer Strongbox', count: 1 },
			],
			keysUsed: 4,
			keys: exact(40),
			ammoUnits: 200000,
			ammoRows: 2,
			ammoFromRows: wholePec(20),
			ammoFromKeys: wholePec(25),
			contents: [],
			possibleContents: [
				{ item: carried('Fireworks'), units: 2, amount: exact(4) },
				{ item: carried('Mask'), units: 1, amount: exact(1) },
			],
			settlement: 'unsettled',
		};
		const sentence = eventSentence(opening);
		expect(sentence).toBe(
			'Opened 3 Halloween Strongbox and 1 Summer Strongbox with 4 keys: ammo that could not be settled; possibly 2 Fireworks and Mask',
		);
		expect(sentence).not.toContain('20.00');
	});

	it('names sales, purchases, moves and the rest', () => {
		const cases: [MeasuredEvent, string][] = [
			[
				{
					kind: 'sale',
					between: SPAN,
					items: [{ item: carried('Animal Oil Residue'), units: 120, amount: wholePec(12.4) }],
					amount: wholePec(12.4),
				},
				'Sold to the Trade Terminal: 12.40 PED; 120 Animal Oil Residue',
			],
			[
				{
					kind: 'purchase',
					between: SPAN,
					items: [{ item: carried('Repair Kit'), units: 5, amount: exact(5) }],
					amount: wholePec(5),
				},
				'Bought from the Trade Terminal: 5.00 PED; 5 Repair Kit',
			],
			[
				{
					kind: 'moved',
					between: SPAN,
					name: 'Shrapnel',
					units: 1000,
					amount: exact(0.1),
					from: { kind: 'carried' },
					to: { kind: 'storage', planet: 'Calypso' },
					moveKind: 'transfer',
				},
				'Moved to storage on Calypso: 1000 Shrapnel, 0.1000 PED',
			],
			[
				{
					kind: 'moved',
					between: SPAN,
					name: 'Oil',
					units: 10,
					amount: exact(1),
					from: { kind: 'storage', planet: 'Calypso' },
					to: { kind: 'carried' },
					moveKind: 'transfer',
				},
				'Moved from storage on Calypso to your inventory: 10 Oil, 1.0000 PED',
			],
			[
				{
					kind: 'moved',
					between: SPAN,
					name: 'Oil',
					units: 10,
					amount: exact(1),
					from: { kind: 'carried' },
					to: { kind: 'auction' },
					moveKind: 'listing',
				},
				'Listed on the auction: 10 Oil, 1.0000 PED',
			],
			[
				{ kind: 'appeared', between: SPAN, item: carried('Pants'), units: 1, value: exact(2) },
				'Arrived in your inventory: Pants, 2.0000 PED',
			],
			[
				{ kind: 'left', between: SPAN, item: carried('Pants'), units: 1, value: exact(2) },
				'Left your inventory: Pants, 2.0000 PED',
			],
			[
				{
					kind: 'shrapnelConverted',
					between: SPAN,
					shrapnelUnits: 10000,
					shrapnel: exact(1),
					ammoUnits: 10100,
					ammo: exact(1.01),
				},
				'Converted 10000 Shrapnel (1.0000 PED) into 10100 ammo (1.0100 PED)',
			],
			[
				{
					kind: 'listingSold',
					between: SPAN,
					item: { name: 'Oil', place: { kind: 'auction' } },
					units: 10,
					amount: exact(1),
				},
				'Sold on the auction: 10 Oil, 1.0000 PED',
			],
			[
				{ kind: 'ammoToppedUp', between: SPAN, name: 'Universal Ammo' },
				'Universal Ammo topped up: what was fired in that span is the tracked figure',
			],
		];
		for (const [event, sentence] of cases) expect(eventSentence(event)).toBe(sentence);
	});

	it('lists events in time order, each at its reading time', () => {
		const sale = (to: string, ped: number): MeasuredEvent => ({
			kind: 'sale',
			between: { from: to, to },
			items: [{ item: carried('Oil'), units: 10, amount: wholePec(ped) }],
			amount: wholePec(ped),
		});
		const early = '2026-11-15T10:01:00Z';
		const late = '2026-11-15T11:01:00Z';
		const events = [sale(late, 2), sale(early, 1)];
		const lines = eventLines(events, readingClock([early, late]));
		expect(lines.map((line) => line.text)).toEqual([
			'Sold to the Trade Terminal: 1.00 PED; 10 Oil',
			'Sold to the Trade Terminal: 2.00 PED; 10 Oil',
		]);
		expect(lines[0].at).toBe(formatTime(early));
	});

	it("shows the date as well as the time once a session's times span more than one day", () => {
		const first = '2026-11-15T12:00:00Z';
		const next = '2026-11-16T12:30:00Z';
		expect(readingClock([first, '2026-11-15T13:00:00Z'])(first)).toBe(formatTime(first));
		expect(readingClock([first, next])(next)).toBe(`${formatDate(next)}, ${formatTime(next)}`);
		const spanning = measureClock(
			measure({
				events: [
					{
						kind: 'sale',
						between: { from: first, to: first },
						items: [{ item: carried('Oil'), units: 1, amount: wholePec(1) }],
						amount: wholePec(1),
					},
				],
				repairs: [
					{
						between: { from: next, to: next },
						items: [{ item: carried('Harness'), amount: exact(1) }],
						restored: exact(1),
						cost: { kind: 'unknown' },
					},
				],
			}),
		);
		expect(spanning(first)).toBe(`${formatDate(first)}, ${formatTime(first)}`);
	});

	it('names every place', () => {
		expect(placeName({ kind: 'carried' })).toBe('your inventory');
		expect(placeName({ kind: 'auction' })).toBe('the auction');
		expect(placeName({ kind: 'storage', planet: 'Arkadia' })).toBe('storage on Arkadia');
		expect(placeName({ kind: 'attached', parent: 'Rifle' })).toBe('attached to Rifle');
		expect(placeName({ kind: 'other', name: 'Estate' })).toBe('Estate');
	});
});

describe('the details', () => {
	it('groups wear by role, heaviest first', () => {
		const groups = wearGroups([
			{ item: carried('Light Rifle'), role: 'weapon', decay: exact(0.5) },
			{ item: carried('Heavy Rifle'), role: 'weapon', decay: exact(2) },
			{ item: carried('Harness'), role: 'armour', decay: wholePec(0.3) },
		]);
		expect(groups.map((group) => group.label)).toEqual(['Weapons', 'Armour']);
		expect(groups[0].items.map((item) => item.name)).toEqual(['Heavy Rifle', 'Light Rifle']);
		expect(groups[0].total).toEqual(exact(2.5));
	});

	it('shows a repair as what it restored, with what the PED Card paid only as a cross-check', () => {
		const base = {
			between: SPAN,
			items: [{ item: carried('Harness'), amount: exact(1) }],
			restored: exact(1),
		};
		expect(
			repairLine({ ...base, cost: { kind: 'paid', amount: wholePec(1.02) } }, formatTime).paid,
		).toBe('1.02 PED paid');
		expect(
			repairLine({ ...base, cost: { kind: 'derived', amount: wholePec(1) } }, formatTime).paid,
		).toBe('about 1.00 PED paid');
		const unknown = repairLine({ ...base, cost: { kind: 'unknown' } }, formatTime);
		expect(unknown.paid).toBeNull();
		expect(unknown.what).toBe('Harness');
		expect(unknown.restored).toBe('1.0000 PED');
		expect(repairLines([], formatTime)).toEqual([]);
	});

	it('says when a listed amount rests on values rounded down to the PEC', () => {
		expect(listsWholePec(measure())).toBe(false);
		expect(
			listsWholePec(
				measure({ decay: [{ item: carried('Rifle'), role: 'weapon', decay: exact(0.5) }] }),
			),
		).toBe(false);
		expect(
			listsWholePec(
				measure({ decay: [{ item: carried('Harness'), role: 'armour', decay: wholePec(0.3) }] }),
			),
		).toBe(true);
		expect(
			listsWholePec(
				measure({
					otherConsumption: [
						{ item: carried('Mind Essence'), role: 'material', units: 900, amount: wholePec(0.09) },
					],
				}),
			),
		).toBe(true);
		expect(
			listsWholePec(
				measure({
					unexplainedDepartures: [
						{ item: carried('Oil'), role: 'material', units: 10, amount: wholePec(0.1) },
					],
				}),
			),
		).toBe(true);
		expect(
			listsWholePec(
				measure({
					repairs: [
						{
							between: SPAN,
							items: [{ item: carried('Harness'), amount: wholePec(1) }],
							restored: wholePec(1),
							cost: { kind: 'unknown' },
						},
					],
				}),
			),
		).toBe(true);
		const ammo = (burned: MeasuredAmount) =>
			measure({
				ammo: [
					{
						name: 'Universal Ammo',
						netUnits: -10000,
						net: burned,
						split: {
							kind: 'settled',
							burnedUnits: 10000,
							burned,
							strongboxUnits: 0,
							strongbox: exact(0),
							convertedUnits: 0,
							converted: exact(0),
						},
					},
				],
			});
		expect(listsWholePec(ammo(wholePec(1)))).toBe(true);
		expect(listsWholePec(ammo(exact(1)))).toBe(false);
		// An unsettled ammo type lists no burned figure, so nothing of it rests on rounding.
		expect(
			listsWholePec(
				measure({
					ammo: [
						{
							name: 'Universal Ammo',
							netUnits: -10000,
							net: wholePec(-1),
							split: {
								kind: 'unsettled',
								burnedUnitsByRows: 10000,
								strongboxUnitsByRows: 0,
								convertedUnits: 0,
							},
						},
					],
				}),
			),
		).toBe(false);
	});

	it('lists consumption heaviest first with its units', () => {
		const lines = consumptionLines([
			{ item: carried('Mind Essence'), role: 'material', units: 1, amount: exact(0.1) },
			{ item: carried('Teleport Chip'), role: 'material', units: 3, amount: exact(0.3) },
		]);
		expect(lines.map((line) => `${line.name} ${line.detail}`)).toEqual([
			'Teleport Chip 3 units',
			'Mind Essence 1 unit',
		]);
	});

	it('shows a settled ammo type burned, and an unsettled one by its net change alone', () => {
		const [settled, unsettled] = ammoLines([
			{
				name: 'Universal Ammo',
				netUnits: -9000,
				net: exact(-0.9),
				split: {
					kind: 'settled',
					burnedUnits: 10000,
					burned: exact(1),
					strongboxUnits: 1000,
					strongbox: exact(0.1),
					convertedUnits: 0,
					converted: exact(0),
				},
			},
			{
				name: 'Universal Ammo',
				netUnits: -9000,
				net: exact(-0.9),
				split: {
					kind: 'unsettled',
					burnedUnitsByRows: 10000,
					strongboxUnitsByRows: 1000,
					convertedUnits: 0,
				},
			},
		]);
		expect(settled.burned).toEqual(exact(1));
		expect(settled.parts).toEqual(['10000 burned', '1000 paid out by strongboxes (0.1000 PED)']);
		expect(unsettled.burned).toBeNull();
		expect(unsettled.parts[0]).toBe('net change -9000 units');
	});

	it('qualifies what burned of an ammo type topped up mid-session', () => {
		const used = {
			name: 'Universal Ammo',
			netUnits: -20000,
			net: exact(-2),
			split: {
				kind: 'settled' as const,
				burnedUnits: 25000,
				burned: exact(2.5),
				strongboxUnits: 0,
				strongbox: exact(0),
				convertedUnits: 0,
				converted: exact(0),
			},
		};
		const [topped] = ammoLines(
			[used],
			[{ kind: 'ammoToppedUp', between: SPAN, name: 'Universal Ammo' }],
		);
		expect(topped.burned).toEqual(exact(2.5));
		expect(topped.parts).toEqual([
			'25000 burned outside the span it was topped up in, which is not measured',
		]);
		const [plain] = ammoLines([used]);
		expect(plain.parts).toEqual(['25000 burned']);
	});

	it('has details only when the measure holds something to show', () => {
		expect(hasDetails(measure())).toBe(false);
		expect(
			hasDetails(
				measure({
					events: [
						{
							kind: 'sale',
							between: SPAN,
							items: [{ item: carried('Oil'), units: 10, amount: wholePec(1) }],
							amount: wholePec(1),
						},
					],
				}),
			),
		).toBe(true);
	});
});
