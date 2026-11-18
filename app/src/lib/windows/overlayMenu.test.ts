// @vitest-environment happy-dom

import { describe, expect, it } from 'vitest';
import type { ActivityOption, ActivityOptionsResult } from '$lib/api';
import {
	buildActivitiesMenuState,
	buildConsumablesMenuState,
	buildEquipmentMenuState,
	buildHeldToolState,
	buildMobSourceState,
	buildQuestHandInMenuState,
	computeMenuHeight,
	isPanelState,
	menuRowCount,
} from './overlayMenu';

function option(overrides: Partial<ActivityOption> = {}): ActivityOption {
	return {
		key: 'quest:11',
		kind: 'quest',
		name: 'Daily: Carabok',
		questId: 11,
		active: false,
		available: true,
		resettable: false,
		unavailableReason: null,
		availableFrom: null,
		offRoster: false,
		manualHandIn: false,
		handInWaiting: false,
		rosterOrder: 0,
		...overrides,
	};
}

function offerings(overrides: Partial<ActivityOptionsResult> = {}): ActivityOptionsResult {
	return {
		definitionId: 1,
		definitionName: 'Daily Hunt',
		visible: true,
		adHocSegments: false,
		readyCount: 1,
		options: [option()],
		active: [],
		...overrides,
	};
}

describe('the Activities menu state', () => {
	it('carries the typed name, so a re-presented menu can hand it back', () => {
		// The panel's input is a view of the model's buffer. Without this
		// field a refused declaration cleared the input and stranded the
		// name where nothing rendered it.
		const state = buildActivitiesMenuState(
			180,
			offerings({ adHocSegments: true }),
			false,
			'Boss lap',
		);

		expect(state.segmentDraft).toBe('Boss lap');
	});

	it('carries an empty draft as readily, which is how a landed declaration clears the field', () => {
		const state = buildActivitiesMenuState(180, offerings({ adHocSegments: true }), false, '');

		expect(state.segmentDraft).toBe('');
	});

	it('reports whether a session is running, which is what disables every row', () => {
		expect(buildActivitiesMenuState(180, offerings(), true, '').idle).toBe(true);
		expect(buildActivitiesMenuState(180, offerings(), false, '').idle).toBe(false);
	});

	it('counts the naming field as a row, so the window is sized to hold it', () => {
		const rows = menuRowCount(buildActivitiesMenuState(180, offerings(), false, ''));
		const withEntry = menuRowCount(
			buildActivitiesMenuState(180, offerings({ adHocSegments: true }), false, ''),
		);

		expect(withEntry).toBe(rows + 1);
	});

	it('never sizes to nothing, so the empty state has a line to render on', () => {
		const state = buildActivitiesMenuState(180, offerings({ options: [] }), false, '');

		expect(menuRowCount(state)).toBe(1);
	});
});

describe('the manual hand-in satellite', () => {
	it('reserves its maximum height while waiting so the next candidate cannot be clipped', () => {
		const state = buildQuestHandInMenuState(180, {
			questId: 7,
			questName: 'AI Daily terminal',
			waiting: true,
			candidate: null,
		});

		expect(computeMenuHeight(menuRowCount(state))).toBe(220);
	});
});

describe('the consumables menu', () => {
	const pill = {
		equipmentId: 40,
		name: 'Adrenaline',
		durationSeconds: 3600,
		doseCostPed: 4.5,
		costTracked: true,
		reloadSpeedPercent: 10,
		hotbarSlot: null,
	};

	it('widens for the already-in-effect choice only when a row offers it', () => {
		const offering = buildConsumablesMenuState(0, [pill], []);
		const running = buildConsumablesMenuState(0, [pill], [40]);
		const instant = buildConsumablesMenuState(0, [{ ...pill, durationSeconds: 0 }], []);
		expect(offering.width).toBeGreaterThan(running.width);
		expect(instant.width).toBe(running.width);
	});
});

describe('buildEquipmentMenuState', () => {
	const rifle = { equipmentId: 1, name: 'Opalo', kind: 'weapon' as const, hotbarSlot: '1' };
	const fap = { equipmentId: 4, name: 'FAP', kind: 'healing' as const, hotbarSlot: null };

	it('marks the healer held apart from the tool in hand, and sizes one row per item', () => {
		const state = buildEquipmentMenuState(0, [rifle, fap], 'Opalo', 'FAP');
		expect(state.inHand).toBe('Opalo');
		expect(state.healer).toBe('FAP');
		expect(menuRowCount(state)).toBe(2);
	});

	it('does not mark the healer twice when it is the tool in hand', () => {
		expect(buildEquipmentMenuState(0, [fap], 'FAP', 'FAP').healer).toBeNull();
	});

	it('keeps one row for the empty state', () => {
		expect(menuRowCount(buildEquipmentMenuState(0, [], null, null))).toBe(1);
	});
});

describe('the panels that say more about a line', () => {
	const old = { name: 'Atrox Old', species: 'Atrox', maturity: 'Old' };
	const young = { name: 'Atrox Young', species: 'Atrox', maturity: 'Young' };

	it('says where the mob comes from as the snapshot reads it', () => {
		const state = buildMobSourceState(
			{ readingScreen: true, pin: null, typed: young, reading: old, continuity: null },
			true,
			false,
		);
		expect(state.lines.map((line) => [line.kind, line.creature?.name])).toEqual([
			['reading', 'Atrox Old'],
			['backstop', 'Atrox Young'],
		]);
		expect(state).toMatchObject({ kind: 'mobSource', canType: true, pinned: false, saving: false });
		const pinned = buildMobSourceState(
			{
				readingScreen: false,
				pin: { creature: old, by: 'typed' },
				typed: null,
				reading: null,
				continuity: null,
			},
			false,
			true,
		);
		expect(pinned).toMatchObject({ canType: false, pinned: true, saving: true });
	});

	it('says the tool in hand with the hit cue explained when one stands', () => {
		const display = {
			label: 'Pistol',
			tone: 'normal' as const,
			marker: null,
			lines: ['Read off the screen.'],
			actions: [],
			setup: null,
		};
		expect(buildHeldToolState(display, null, false).lines).toEqual(['Read off the screen.']);
		const cued = buildHeldToolState(display, 'Hits land below the set-up.', true);
		expect(cued.lines).toEqual(['Read off the screen.', 'Hits land below the set-up.']);
		expect(cued.declaring).toBe(true);
	});

	it('tells a panel from a list of rows', () => {
		expect(isPanelState({ kind: 'note', heading: 'Tool', text: 'Why' })).toBe(true);
		expect(isPanelState(buildEquipmentMenuState(0, [], null, null))).toBe(false);
	});
});
