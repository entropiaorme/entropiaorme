import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { EquipmentSearchResult } from '$lib/api';
import { comparison, component, economics, setup } from './__fixtures__/comparison';
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
vi.mock('$lib/api/character', () => ({
	getCharacterProfessions: vi.fn(),
}));

import * as api from '$lib/api';
import * as character from '$lib/api/character';

const mocked = vi.mocked(api);
const professions = vi.mocked(character.getCharacterProfessions);

function profession(name: string, level: number) {
	return { name, level, anchorLevel: null, gainSinceAnchor: null, category: 'Looting' };
}

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
	professions.mockResolvedValue([]);
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

	it('holds capital typed for an unlimited part against the item it was typed for', () => {
		const draft = createSetupDraft(1, 1);
		const scimitar = unlimited('scimitar', 'Scimitar of the Mages, TWEN Edition');
		draft.pickers.weapon.select(scimitar);
		draft.setCapitalText('weapon', '150k');
		draft.pickers.amplifier.select(unlimited('a106', 'Omegaton A106'));
		draft.setCapitalText('amplifier', '+2,500');
		expect(draft.capitalTextFor('weapon')).toBe('150k');
		expect(draft.capitalPed).toBe(152_500);

		// Another weapon starts with none; the first one's comes back with it.
		draft.pickers.weapon.select(unlimited('saif', 'CUHOF Saif'));
		expect(draft.capitalTextFor('weapon')).toBe('');
		expect(draft.capitalPed).toBe(2500);
		draft.pickers.weapon.select(scimitar);
		expect(draft.capitalPed).toBe(152_500);

		// Text that is not an amount counts for nothing.
		draft.setCapitalText('amplifier', 'lots');
		expect(draft.capitalPed).toBe(150_000);
	});

	it('counts no capital for a limited part, whose markup is paid per use', () => {
		const draft = createSetupDraft(1, 1);
		draft.pickers.weapon.select(hit());
		draft.setCapitalText('weapon', '5000');
		expect(draft.capitalPed).toBe(0);
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
		expect(mocked.compareLoadouts).toHaveBeenCalledWith(
			[
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
			],
			3000,
			null,
		);
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

	it('carries the curve reach and looter level into the request', async () => {
		const comparator = ready();
		await comparator.compare();
		comparator.hpMax = 20_000;
		expect(comparator.current).toBe(false);
		await comparator.compare();
		expect(mocked.compareLoadouts).toHaveBeenLastCalledWith(expect.any(Array), 20_000, null);
		comparator.looterLevel = 42.5;
		expect(comparator.current).toBe(false);
		await comparator.compare();
		expect(mocked.compareLoadouts).toHaveBeenLastCalledWith(expect.any(Array), 20_000, 42.5);
		comparator.looterLevel = null;
		expect(comparator.requestKey).toContain('null');
		expect(comparator.current).toBe(false);
	});

	it('keeps the sliders inside their ranges', () => {
		const comparator = createComparator();
		comparator.hpMax = 5;
		expect(comparator.hpMax).toBe(10);
		comparator.hpMax = 250_000;
		expect(comparator.hpMax).toBe(100_000);
		comparator.hpMax = Number.NaN;
		expect(comparator.hpMax).toBe(100_000);

		// The pinned HP stays on the curve, and a shorter curve pulls it in.
		comparator.pinnedHp = 0;
		expect(comparator.pinnedHp).toBe(1);
		comparator.pinnedHp = 4321.4;
		expect(comparator.pinnedHp).toBe(4321);
		comparator.hpMax = 500;
		expect(comparator.pinnedHp).toBe(500);
		comparator.pinnedHp = 900;
		expect(comparator.pinnedHp).toBe(500);

		comparator.killCount = 12.6;
		expect(comparator.killCount).toBe(13);
		comparator.killCount = -5;
		expect(comparator.killCount).toBe(0);
		comparator.killCount = 5_000_000;
		expect(comparator.killCount).toBe(1_000_000);
		comparator.killMax = 0;
		expect(comparator.killMax).toBe(1_000_000);
		comparator.killMax = 4000.4;
		expect(comparator.killMax).toBe(4000);
		expect(comparator.killCount).toBe(4000);

		comparator.looterLevel = 150;
		expect(comparator.looterLevel).toBe(100);
		comparator.looterLevel = -3;
		expect(comparator.looterLevel).toBe(0);
		comparator.looterLevel = Number.NaN;
		expect(comparator.looterLevel).toBeNull();

		comparator.capitalRate = 0;
		expect(comparator.capitalRate).toBe(0.1);
		comparator.capitalRate = 0.05;
		expect(comparator.capitalRate).toBe(0.05);
	});

	it('weighs capital against the picked limited setup, else the cheapest per kill', async () => {
		// Two limited setups, the second cheaper per kill, and an unlimited one.
		mocked.compareLoadouts.mockResolvedValue(
			comparison({
				setups: [
					setup(0, 'BC-30'),
					setup(1, 'Gauss', {
						economics: economics({ lossPerUsePec: 0.3, houseEdgePec: 0.18 }),
					}),
					setup(2, 'Scimitar', {
						components: [component({ name: 'Scimitar', isLimited: false })],
					}),
				],
			}),
		);
		const comparator = createComparator();
		list(comparator, hit());
		list(comparator, hit({ catalogId: 'gauss', name: 'Gauss' }));
		list(comparator, unlimited('scimitar', 'Scimitar'));
		const [bc30, gauss, scimitar] = comparator.drafts;
		expect(comparator.referenceDraftId).toBeNull();
		await comparator.compare();
		expect(comparator.referenceDraftId).toBe(gauss.id);

		comparator.referenceId = bc30.id;
		expect(comparator.referenceId).toBe(bc30.id);
		expect(comparator.referenceDraftId).toBe(bc30.id);
		// An unlimited setup is never the reference.
		comparator.referenceId = scimitar.id;
		expect(comparator.referenceDraftId).toBe(gauss.id);

		// Removing the picked setup falls back to the cheapest.
		comparator.referenceId = bc30.id;
		comparator.remove(bc30.id);
		expect(comparator.referenceId).toBeNull();
		expect(comparator.referenceDraftId).toBe(gauss.id);
	});

	it("reads the player's own looter level up front, and the comparison's once shown", async () => {
		professions.mockResolvedValue([
			profession('Animal Looter', 30),
			profession('Mutant Looter', 60),
			profession('Robot Looter', 90),
			profession('Rifle', 99),
		]);
		const comparator = ready();
		expect(comparator.ownLooterLevel).toBeNull();
		await vi.waitFor(() => expect(comparator.ownLooterLevel).toBe(60));
		mocked.compareLoadouts.mockResolvedValue(
			comparison({ assumptions: { ...comparison().assumptions, ownLooterLevel: 61.25 } }),
		);
		await comparator.compare();
		expect(comparator.ownLooterLevel).toBe(61.25);
	});

	it('counts a missing looter as level 0, and every looter as 0 when the read fails', async () => {
		professions.mockResolvedValueOnce([profession('Animal Looter', 45)]);
		const partial = createComparator();
		await vi.waitFor(() => expect(partial.ownLooterLevel).toBe(15));
		professions.mockRejectedValueOnce(new Error('not ready'));
		const failed = createComparator();
		await vi.waitFor(() => expect(failed.ownLooterLevel).toBe(0));
	});

	it('keeps one comparator for the app session', () => {
		expect(sessionComparator()).toBe(sessionComparator());
	});
});
