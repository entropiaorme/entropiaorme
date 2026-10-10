import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { EquipmentSearchResult } from '$lib/api';
import { comparison } from './__fixtures__/comparison';
import {
	createComparator,
	createSetupDraft,
	MAX_SETUPS,
	markupProblem,
	sessionComparator,
} from './compareModel.svelte';

vi.mock('$lib/api', () => ({
	compareLoadouts: vi.fn(),
	searchEquipmentItems: vi.fn(async () => []),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function hit(overrides: Partial<EquipmentSearchResult> = {}): EquipmentSearchResult {
	return {
		catalogId: 'bc30',
		name: 'Exceptional A&P Series Mayhem BC-30, Modified (L)',
		decay: 0.002,
		ammoBurn: 8.04,
		absorptionPercent: null,
		isLimited: true,
		healMin: null,
		healMax: null,
		reloadSeconds: null,
		lifestealPercent: null,
		usesPerMinute: 67,
		consumable: null,
		...overrides,
	};
}

const unlimited = (catalogId: string, name: string) => hit({ catalogId, name, isLimited: false });

beforeEach(() => {
	vi.clearAllMocks();
	mocked.compareLoadouts.mockResolvedValue(comparison());
});

describe('markupProblem', () => {
	it('takes a markup of at least 100%', () => {
		expect(markupProblem('Weapon', 1800)).toBeNull();
		expect(markupProblem('Weapon', 100)).toBeNull();
		expect(markupProblem('Weapon', 99)).toBe('Weapon markup is a percentage of at least 100.');
		expect(markupProblem('Weapon', null)).not.toBeNull();
	});
});

describe('createSetupDraft', () => {
	it('has no request until it has a weapon', () => {
		const draft = createSetupDraft(1, 1);
		expect(draft.pick()).toBeNull();
		expect(draft.problem).toBeNull();
	});

	it('carries a limited item at its markup and an unlimited one with none', () => {
		const draft = createSetupDraft(1, 1);
		draft.pickers.weapon.select(hit());
		expect(draft.markupFor('weapon')).toBe(100);
		draft.setMarkup('weapon', 1800);
		draft.pickers.amplifier.select(unlimited('a106', 'Omegaton A106'));
		expect(draft.pick()).toEqual({
			weapon: { catalogId: 'bc30', markupPercent: 1800 },
			amplifier: { catalogId: 'a106', markupPercent: null },
			absorber: null,
			implant: null,
		});
	});

	it('starts a newly picked item at TT again', () => {
		const draft = createSetupDraft(1, 1);
		draft.pickers.weapon.select(hit());
		draft.setMarkup('weapon', 1800);
		draft.pickers.weapon.select(hit({ catalogId: 'gauss', name: 'Gauss' }));
		expect(draft.markupFor('weapon')).toBe(100);
	});

	it('refuses a markup under TT on a limited part', () => {
		const draft = createSetupDraft(1, 1);
		draft.pickers.weapon.select(hit());
		draft.pickers.absorber.select(hit({ catalogId: 'p20', name: 'ArMatrix Extender P20 (L)' }));
		draft.setMarkup('absorber', 50);
		expect(draft.problem).toBe('Absorber markup is a percentage of at least 100.');
		expect(draft.pick()).toBeNull();
	});

	it('opens and removes optional parts', () => {
		const draft = createSetupDraft(1, 1);
		expect(draft.isShown('implant')).toBe(false);
		draft.show('implant');
		expect(draft.isShown('implant')).toBe(true);
		draft.pickers.implant.select(
			hit({ catalogId: 'neo', name: 'NeoPsion 85-B Mindforce Implant (L)' }),
		);
		draft.removePart('implant');
		expect(draft.isShown('implant')).toBe(false);
		expect(draft.pickers.implant.selected).toBeNull();
	});
});

describe('createComparator', () => {
	/** List a setup through the empty row, as picking its weapon does. */
	function list(comparator: ReturnType<typeof createComparator>, item: EquipmentSearchResult) {
		comparator.blank?.pickers.weapon.select(item);
		comparator.commitBlank();
	}

	function ready() {
		const comparator = createComparator();
		list(comparator, hit());
		list(comparator, unlimited('scimitar', 'Scimitar of the Mages, TWEN Edition'));
		return comparator;
	}

	it('starts with one empty row and lists it once its weapon is picked', () => {
		const comparator = createComparator();
		expect(comparator.drafts).toHaveLength(0);
		const first = comparator.blank;
		expect(first).not.toBeNull();
		// An empty row is not listed.
		comparator.commitBlank();
		expect(comparator.drafts).toHaveLength(0);
		list(comparator, hit());
		expect(comparator.drafts).toEqual([first]);
		expect(comparator.blank).not.toBe(first);
		expect(comparator.blank?.slot).not.toBe(first?.slot);
	});

	it('stops offering an empty row at the most setups, and reuses a freed colour', () => {
		const comparator = createComparator();
		for (let count = 0; count < MAX_SETUPS; count += 1) list(comparator, hit());
		expect(comparator.drafts).toHaveLength(MAX_SETUPS);
		expect(comparator.blank).toBeNull();
		const third = comparator.drafts[2];
		comparator.remove(third.id);
		expect(comparator.drafts).toHaveLength(MAX_SETUPS - 1);
		expect(comparator.blank?.slot).toBe(third.slot);
	});

	it('compares every listed setup once per distinct request', async () => {
		const comparator = createComparator();
		expect(comparator.requestKey).toBeNull();
		await comparator.compare();
		expect(mocked.compareLoadouts).not.toHaveBeenCalled();

		list(comparator, hit());
		list(comparator, unlimited('scimitar', 'Scimitar'));
		await comparator.compare();
		expect(mocked.compareLoadouts).toHaveBeenCalledWith([
			{
				weapon: { catalogId: 'bc30', markupPercent: 100 },
				amplifier: null,
				absorber: null,
				implant: null,
			},
			{
				weapon: { catalogId: 'scimitar', markupPercent: null },
				amplifier: null,
				absorber: null,
				implant: null,
			},
		]);
		expect(comparator.current).toBe(true);
		await comparator.compare();
		expect(mocked.compareLoadouts).toHaveBeenCalledTimes(1);

		comparator.drafts[0].setMarkup('weapon', 1600);
		expect(comparator.current).toBe(false);
		await comparator.compare();
		expect(mocked.compareLoadouts).toHaveBeenCalledTimes(2);
	});

	it('maps the computed figures back to the setups they answer', async () => {
		const comparator = ready();
		await comparator.compare();
		const [first, second] = comparator.drafts;
		expect(comparator.figuresFor(first.id)?.index).toBe(0);
		expect(comparator.figuresFor(second.id)?.index).toBe(1);
		comparator.remove(first.id);
		expect(comparator.drafts).toEqual([second]);
		expect(comparator.current).toBe(false);
	});

	it('never lets a slower earlier answer replace a later one', async () => {
		let resolveFirst: (value: ReturnType<typeof comparison>) => void = () => {};
		mocked.compareLoadouts
			.mockImplementationOnce(() => new Promise((resolve) => (resolveFirst = resolve)))
			.mockResolvedValueOnce(comparison({ hpMax: 999 }));
		const comparator = ready();
		const first = comparator.compare();
		comparator.drafts[0].setMarkup('weapon', 1600);
		await comparator.compare();
		resolveFirst(comparison({ hpMax: 1000 }));
		await first;
		expect(comparator.comparison?.hpMax).toBe(999);
		expect(comparator.loading).toBe(false);
	});

	it('reports a refusal and keeps what it had', async () => {
		const comparator = ready();
		await comparator.compare();
		mocked.compareLoadouts.mockRejectedValueOnce(
			new Error("Setup 1's weapon 'bc30' is not in the catalogue"),
		);
		comparator.drafts[0].setMarkup('weapon', 1700);
		await comparator.compare();
		expect(comparator.error).toBe("Setup 1's weapon 'bc30' is not in the catalogue");
		expect(comparator.comparison).not.toBeNull();
	});

	it('keeps one comparator for the app session', () => {
		expect(sessionComparator()).toBe(sessionComparator());
	});
});
