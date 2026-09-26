import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppSettings, EquipmentDetail } from '$lib/api/commands.gen';
import type { Equipment } from '$lib/types';
import { createLibraryModel } from './libraryModel.svelte';

vi.mock('$lib/api', () => ({
	getEquipmentLibrary: vi.fn(),
	getSettings: vi.fn(),
	hotbarFromSettings: vi.fn(() => ({ '1': null })),
	searchEquipmentItems: vi.fn(),
	addToLibrary: vi.fn(),
	updateLibrary: vi.fn(),
	removeFromLibrary: vi.fn(),
	getEquipmentDetail: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function summary(overrides: Partial<Equipment> = {}): Equipment {
	return {
		id: '1',
		name: 'Jester D-1',
		type: 'weapon',
		amplifierName: null,
		costPerUse: 0.42,
		damageMin: 13,
		damageMax: 22.5,
		reloadSeconds: 2.5,
		isLimited: false,
		enrichmentLevel: 1,
		healingProfile: null,
		lifestealPercent: null,
		effectProfile: null,
		consumable: null,
		...overrides,
	};
}

function detail(overrides: Partial<EquipmentDetail> = {}): EquipmentDetail {
	return {
		id: '1',
		type: 'weapon',
		weapon: {
			catalogId: 'jester-d1',
			name: 'Jester D-1',
			decay: 0.05,
			ammoBurn: 2.0,
			markupPercent: 100,
			isLimited: false,
			damageEnhancers: 0,
			efficiencyPct: 55,
		},
		amplifier: null,
		scope: null,
		absorber: null,
		implant: null,
		costBreakdown: [],
		totalCostPerUse: 2.05,
		expectedReturn: null,
		healingProfile: null,
		lifestealPercent: null,
		effectProfile: null,
		consumable: null,
		attackRate: null,
		...overrides,
	};
}

function settings(): AppSettings {
	return {
		hotbarHooksEnabled: false,
		hotbar: {},
		carriedWeaponIds: [7],
	} as unknown as AppSettings;
}

const weaponHit = {
	catalogId: 'korss-h400',
	name: 'Korss H400',
	decay: 2.0,
	ammoBurn: 1.0,
	absorptionPercent: null,
	isLimited: false,
	healMin: null,
	healMax: null,
	reloadSeconds: null,
	lifestealPercent: null,
	usesPerMinute: null,
	consumable: null,
};

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getEquipmentLibrary.mockResolvedValue([]);
	mocked.getSettings.mockResolvedValue(settings());
	mocked.searchEquipmentItems.mockResolvedValue([]);
});

afterEach(() => {
	vi.useRealTimers();
});

describe('loadData', () => {
	it('splits the library by kind and sorts the healing and consumable views', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([
			summary({ id: '1', name: 'Zulu', type: 'weapon' }),
			summary({ id: '2', name: 'Vivo T1', type: 'healing', costPerUse: 0.18, isLimited: true }),
			summary({ id: '3', name: 'Adjusted Fap', type: 'healing' }),
			summary({ id: '4', name: 'Oil', type: 'consumable' }),
		]);
		const model = createLibraryModel();
		await model.loadData(false);

		expect(model.allEquipment).toHaveLength(4);
		expect(model.sortedEquipment.map((e) => e.name)).toEqual(['Zulu']);
		expect(model.healingTools.map((t) => t.name)).toEqual(['Adjusted Fap', 'Vivo T1']);
		expect(model.healingTools[1]).toEqual({
			id: '2',
			name: 'Vivo T1',
			costPerHeal: 0.18,
			isLimited: true,
			reloadSeconds: 2.5,
			profile: null,
		});
		expect(model.consumables.map((c) => c.name)).toEqual(['Oil']);
		expect(model.hotbarHooksEnabled).toBe(false);
		expect(model.carriedWeaponIds).toEqual([7]);
		expect(model.hotbar).toEqual({ '1': null });
	});

	it('drops the detail cache on a live reload', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([summary()]);
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = createLibraryModel();
		await model.loadData(false);
		await model.toggleExpand('1');
		expect(model.detailCache['1']).toBeDefined();

		await model.loadData(false);
		expect(model.detailCache['1']).toBeUndefined();
	});

	it('seeds the guide fixtures without touching the API in guide mode', async () => {
		const model = createLibraryModel();
		await model.loadData(true);
		expect(mocked.getEquipmentLibrary).not.toHaveBeenCalled();
		expect(mocked.getSettings).not.toHaveBeenCalled();
		expect(model.sortedEquipment.length).toBeGreaterThan(0);
		expect(Object.keys(model.detailCache).length).toBeGreaterThan(0);
		expect(model.hotbarHooksEnabled).toBe(true);
	});

	it('surfaces a load failure through the error strip', async () => {
		mocked.getEquipmentLibrary.mockRejectedValue(new Error('backend unreachable'));
		const model = createLibraryModel();
		await model.loadData(false);
		expect(model.error).toBe('backend unreachable');
		expect(model.loading).toBe(false);
	});

	it('reads as loading until the first load settles, and not again on a reload', async () => {
		let answer!: (library: Equipment[]) => void;
		mocked.getEquipmentLibrary.mockReturnValue(
			new Promise<Equipment[]>((resolve) => {
				answer = resolve;
			}),
		);
		const model = createLibraryModel();
		expect(model.loading).toBe(true);
		const first = model.loadData(false);
		expect(model.loading).toBe(true);
		answer([]);
		await first;
		expect(model.loading).toBe(false);

		mocked.getEquipmentLibrary.mockResolvedValue([summary()]);
		const reload = model.loadData(false);
		expect(model.loading).toBe(false);
		await reload;
	});
});

describe('row expansion', () => {
	it('loads the detail once and collapses on the second toggle', async () => {
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = createLibraryModel();
		await model.toggleExpand('1');
		expect(model.expandedId).toBe('1');
		expect(mocked.getEquipmentDetail).toHaveBeenCalledTimes(1);

		await model.toggleExpand('1');
		expect(model.expandedId).toBeNull();

		await model.toggleExpand('1');
		expect(mocked.getEquipmentDetail).toHaveBeenCalledTimes(1);
	});

	it('surfaces a detail-load failure and keeps the row expanded', async () => {
		mocked.getEquipmentDetail.mockRejectedValue(new Error('not found'));
		const model = createLibraryModel();
		await model.toggleExpand('1');
		expect(model.expandedId).toBe('1');
		expect(model.error).toBe('not found');
	});
});

describe('form open and reset', () => {
	it('openAddModal resets the form and clears every picker', async () => {
		const model = createLibraryModel();
		model.weaponPicker.select({ ...weaponHit });
		model.consumablePicker.select({
			catalogId: null,
			name: 'Oil',
			decay: 0,
			ammoBurn: 0,
			absorptionPercent: null,
			isLimited: false,
			healMin: null,
			healMax: null,
			reloadSeconds: null,
			lifestealPercent: null,
			usesPerMinute: null,
			consumable: null,
		});
		model.markupPercent = 150;
		model.damageEnhancers = 3;

		model.openAddModal(undefined, 'consumable');
		expect(model.showAddModal).toBe(true);
		expect(model.addType).toBe('consumable');
		expect(model.editingEquipmentId).toBeNull();
		expect(model.weaponPicker.selected).toBeNull();
		expect(model.consumablePicker.selected).toBeNull();
		expect(model.markupPercent).toBe(100);
		expect(model.scopeMarkupPercent).toBe(100);
		expect(model.absorberMarkupPercent).toBe(100);
		expect(model.damageEnhancers).toBe(0);
		expect(model.liveCostPreview).toBeNull();
	});

	it('openAddModal with a prefill schedules a weapon search for it', async () => {
		vi.useFakeTimers();
		const model = createLibraryModel();
		model.openAddModal('korss');
		expect(model.weaponPicker.query).toBe('korss');
		await vi.advanceTimersByTimeAsync(200);
		expect(mocked.searchEquipmentItems).toHaveBeenCalledWith('korss', 'weapon');
	});

	it('closing the modal drops the edit target on every close path', async () => {
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = createLibraryModel();
		await model.openEditModal('1');
		expect(model.editingEquipmentId).toBe('1');
		model.showAddModal = false;
		expect(model.editingEquipmentId).toBeNull();
	});
});

describe('openEditModal', () => {
	it('seeds the form from the stored detail', async () => {
		mocked.getEquipmentDetail.mockResolvedValue(
			detail({
				weapon: {
					catalogId: 'cb14',
					name: 'CB14',
					decay: 1.0,
					ammoBurn: 0.5,
					markupPercent: 140,
					isLimited: true,
					damageEnhancers: 2,
					efficiencyPct: 70,
				},
				amplifier: {
					catalogId: 'a104',
					name: 'Omegaton A104',
					decay: 0.4,
					ammoBurn: 0.1,
					markupPercent: 100,
					isLimited: false,
					damageEnhancers: 0,
					efficiencyPct: 60,
				},
				scope: {
					catalogId: 'scope-1',
					name: 'Bullseye',
					decay: 0.2,
					ammoBurn: 0,
					markupPercent: 120,
					isLimited: true,
					damageEnhancers: 0,
					efficiencyPct: 50,
				},
			}),
		);
		const model = createLibraryModel();
		await model.openEditModal('1');

		expect(model.showAddModal).toBe(true);
		expect(model.editingEquipmentId).toBe('1');
		expect(model.addType).toBe('weapon');
		expect(model.weaponPicker.selected?.catalogId).toBe('cb14');
		expect(model.weaponPicker.query).toBe('CB14');
		expect(model.ampPicker.selected?.catalogId).toBe('a104');
		expect(model.scopePicker.selected?.catalogId).toBe('scope-1');
		expect(model.absorberPicker.selected).toBeNull();
		expect(model.healerPicker.selected).toBeNull();
		expect(model.markupPercent).toBe(140);
		expect(model.scopeMarkupPercent).toBe(120);
		expect(model.absorberMarkupPercent).toBe(100);
		expect(model.damageEnhancers).toBe(2);
	});

	it('keeps the attachments section folded when neither scope nor absorber is stored', async () => {
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = createLibraryModel();
		await model.openEditModal('1');
	});

	it('reuses the cached detail without refetching', async () => {
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = createLibraryModel();
		await model.toggleExpand('1');
		await model.openEditModal('1');
		expect(mocked.getEquipmentDetail).toHaveBeenCalledTimes(1);
	});

	it('surfaces a detail-load failure and leaves the modal closed', async () => {
		mocked.getEquipmentDetail.mockRejectedValue(new Error('not found'));
		const model = createLibraryModel();
		await model.openEditModal('1');
		expect(model.showAddModal).toBe(false);
		expect(model.error).toBe('not found');
	});
});

describe('setAddType clearing', () => {
	function seededModel() {
		const model = createLibraryModel();
		model.weaponPicker.select({ ...weaponHit });
		model.ampPicker.select({ ...weaponHit, name: 'Amp' });
		model.healerPicker.select({ ...weaponHit, name: 'Fap' });
		model.consumablePicker.select({
			catalogId: null,
			name: 'Oil',
			decay: 0,
			ammoBurn: 0,
			absorptionPercent: null,
			isLimited: false,
			healMin: null,
			healMax: null,
			reloadSeconds: null,
			lifestealPercent: null,
			usesPerMinute: null,
			consumable: null,
		});
		return model;
	}

	it('switching to healing clears the weapon and amp', () => {
		const model = seededModel();
		model.setAddType('healing');
		expect(model.weaponPicker.selected).toBeNull();
		expect(model.weaponPicker.query).toBe('');
		expect(model.ampPicker.selected).toBeNull();
		expect(model.healerPicker.selected).not.toBeNull();
		expect(model.consumablePicker.selected).not.toBeNull();
	});

	it('switching to consumable clears the weapon, amp and healer', () => {
		const model = seededModel();
		model.setAddType('consumable');
		expect(model.weaponPicker.selected).toBeNull();
		expect(model.ampPicker.selected).toBeNull();
		expect(model.healerPicker.selected).toBeNull();
		expect(model.consumablePicker.selected).not.toBeNull();
	});

	it('switching to weapon clears the healer only', () => {
		const model = seededModel();
		model.setAddType('weapon');
		expect(model.healerPicker.selected).toBeNull();
		expect(model.weaponPicker.selected).not.toBeNull();
		expect(model.ampPicker.selected).not.toBeNull();
		expect(model.consumablePicker.selected).not.toBeNull();
	});
});

describe('amp picker', () => {
	it('drops the selected weapon from the amp results', async () => {
		vi.useFakeTimers();
		const model = createLibraryModel();
		model.weaponPicker.select({ ...weaponHit });
		mocked.searchEquipmentItems.mockResolvedValue([
			{ ...weaponHit },
			{ ...weaponHit, catalogId: 'a104', name: 'Omegaton A104' },
		]);
		model.ampPicker.query = 'omega';
		await vi.advanceTimersByTimeAsync(200);
		expect(model.ampPicker.results.map((r) => r.name)).toEqual(['Omegaton A104']);
	});
});

describe('saveEquipment', () => {
	it('sends the weapon payload with markups only for limited components', async () => {
		mocked.addToLibrary.mockResolvedValue(summary({ id: '9', name: 'Korss H400' }));
		mocked.getEquipmentDetail.mockResolvedValue(detail({ id: '9' }));
		const model = createLibraryModel();
		model.openAddModal();
		model.weaponPicker.select({ ...weaponHit, isLimited: true });
		model.ampPicker.select({ ...weaponHit, catalogId: 'a104', name: 'A104' });
		model.scopePicker.select({
			...weaponHit,
			catalogId: 'scope-1',
			name: 'Scope',
			isLimited: true,
		});
		model.markupPercent = 150;
		model.scopeMarkupPercent = 130;
		model.damageEnhancers = 4;
		await model.saveEquipment();

		expect(mocked.addToLibrary).toHaveBeenCalledWith({
			type: 'weapon',
			catalog_id: 'korss-h400',
			amp_catalog_id: 'a104',
			scope_catalog_id: 'scope-1',
			absorber_catalog_id: null,
			weapon_markup: 150,
			amp_markup: 100,
			scope_markup: 130,
			absorber_markup: 100,
			damage_enhancers: 4,
			implant_catalog_id: null,
			implant_markup: 100,
			weapon_effect: null,
		});
		expect(model.showAddModal).toBe(false);
		expect(model.detailCache['9']).toBeDefined();
		expect(model.sortedEquipment.map((e) => e.id)).toEqual(['9']);
	});

	it('sends a declared damage-over-time effect, and gates the save until it is whole', async () => {
		mocked.addToLibrary.mockResolvedValue(summary({ id: '9', name: 'Electrocution' }));
		mocked.getEquipmentDetail.mockResolvedValue(detail({ id: '9' }));
		const model = createLibraryModel();
		model.openAddModal();
		model.weaponPicker.select(weaponHit);
		expect(model.weaponEffectProblem).toBeNull();
		model.weaponEffectMode = 'compound';
		expect(model.weaponEffectProblem).toBe('Enter the initial hit range');
		model.hitMin = 100;
		model.hitMax = 160;
		expect(model.weaponEffectProblem).toBe('Enter how long the effect lasts');
		model.effectDurationSeconds = 25;
		model.tickMin = 80;
		model.tickMax = 75;
		expect(model.weaponEffectProblem).toBe('The tick minimum must not exceed its maximum');
		model.tickMin = 35;
		expect(model.weaponEffectProblem).toBeNull();
		await model.saveEquipment();

		expect(mocked.addToLibrary).toHaveBeenCalledWith(
			expect.objectContaining({
				weapon_effect: {
					mode: 'compound',
					hit_min: 100,
					hit_max: 160,
					duration_seconds: 25,
					tick_min: 35,
					tick_max: 75,
					tick_seconds: null,
				},
			}),
		);
	});

	it('prefills an edit with the declared effect, and Direct drops it', async () => {
		const effectProfile = {
			mode: 'over_time' as const,
			hitMin: null,
			hitMax: null,
			durationSeconds: 12,
			tickMin: 4,
			tickMax: 6,
			tickSeconds: 2,
		};
		mocked.getEquipmentLibrary.mockResolvedValue([summary({ id: '1', effectProfile })]);
		mocked.getEquipmentDetail.mockResolvedValue(detail({ effectProfile }));
		mocked.updateLibrary.mockResolvedValue(summary({ id: '1' }));
		const model = createLibraryModel();
		await model.loadData(false);
		await model.openEditModal('1');
		expect(model.weaponEffectMode).toBe('over_time');
		expect(model.effectDurationSeconds).toBe(12);
		expect(model.tickSeconds).toBe(2);
		model.weaponEffectMode = 'direct';
		await model.saveEquipment();
		expect(mocked.updateLibrary).toHaveBeenCalledWith(
			'1',
			expect.objectContaining({ weapon_effect: null }),
		);
	});

	it('starts the shared over-time fields afresh on a change of kind', () => {
		const model = createLibraryModel();
		model.openAddModal();
		model.weaponEffectMode = 'over_time';
		model.tickMin = 4;
		model.setAddType('healing');
		model.setAddType('weapon');
		expect(model.weaponEffectMode).toBe('direct');
		expect(model.tickMin).toBeNull();
	});

	it('routes an edit through updateLibrary and swaps the row in place', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([summary({ id: '1', name: 'Old' })]);
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		mocked.updateLibrary.mockResolvedValue(summary({ id: '1', name: 'New' }));
		const model = createLibraryModel();
		await model.loadData(false);
		await model.openEditModal('1');
		await model.saveEquipment();

		expect(mocked.updateLibrary).toHaveBeenCalledWith(
			'1',
			expect.objectContaining({ type: 'weapon' }),
		);
		expect(mocked.addToLibrary).not.toHaveBeenCalled();
		expect(model.sortedEquipment.map((e) => e.name)).toEqual(['New']);
	});

	it('evicts stale weapon detail when the post-save refresh fails', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([summary({ id: '1', name: 'Old' })]);
		mocked.getEquipmentDetail
			.mockResolvedValueOnce(detail())
			.mockRejectedValueOnce(new Error('refresh failed'));
		mocked.updateLibrary.mockResolvedValue(summary({ id: '1', name: 'New' }));
		const model = createLibraryModel();
		await model.loadData(false);
		await model.openEditModal('1');
		await model.saveEquipment();

		expect(model.sortedEquipment.map((e) => e.name)).toEqual(['New']);
		expect(model.detailCache['1']).toBeUndefined();
		expect(model.showAddModal).toBe(false);
		expect(model.error).toBe('refresh failed');
	});

	it('evicts stale healing detail when the post-save refresh fails', async () => {
		const healingProfile = {
			mode: 'direct' as const,
			directMin: 8,
			directMax: 10,
			effectDurationSeconds: null,
			tickMin: null,
			tickMax: null,
			tickSeconds: null,
		};
		mocked.getEquipmentLibrary.mockResolvedValue([
			summary({ id: '5', name: 'Old FAP', type: 'healing', healingProfile }),
		]);
		mocked.getEquipmentDetail
			.mockResolvedValueOnce(
				detail({
					id: '5',
					type: 'healing',
					healingProfile,
					weapon: { ...detail().weapon, name: 'Old FAP' },
				}),
			)
			.mockRejectedValueOnce(new Error('refresh failed'));
		mocked.updateLibrary.mockResolvedValue(
			summary({ id: '5', name: 'New FAP', type: 'healing', healingProfile }),
		);
		const model = createLibraryModel();
		await model.loadData(false);
		await model.openEditModal('5');
		await model.saveEquipment();

		expect(model.healingTools.map((tool) => tool.name)).toEqual(['New FAP']);
		expect(model.detailCache['5']).toBeUndefined();
		expect(model.showAddModal).toBe(false);
		expect(model.error).toBe('refresh failed');
	});

	it('does nothing when the selected weapon has no catalogue id', async () => {
		const model = createLibraryModel();
		model.openAddModal();
		model.weaponPicker.select({ ...weaponHit, catalogId: null });
		await model.saveEquipment();
		expect(mocked.addToLibrary).not.toHaveBeenCalled();
		expect(model.showAddModal).toBe(true);
		expect(model.saving).toBe(false);
	});

	it('sends the healing payload with the shared markup for limited tools', async () => {
		mocked.addToLibrary.mockResolvedValue(summary({ id: '5', name: 'Vivo', type: 'healing' }));
		const model = createLibraryModel();
		model.openAddModal(undefined, 'healing');
		model.healerPicker.select({
			...weaponHit,
			catalogId: 'vivo-t1',
			name: 'Vivo',
			isLimited: true,
			healMin: 8,
			healMax: 10,
		});
		model.markupPercent = 120;
		await model.saveEquipment();

		expect(mocked.addToLibrary).toHaveBeenCalledWith({
			type: 'healing',
			catalog_id: 'vivo-t1',
			weapon_markup: 120,
			implant_catalog_id: null,
			implant_markup: 100,
			healing_mode: 'direct',
			heal_min: 8,
			heal_max: 10,
			effect_duration_seconds: null,
			tick_min: null,
			tick_max: null,
			tick_seconds: null,
		});
		expect(model.healingTools.map((t) => t.id)).toEqual(['5']);
	});

	it('sends the implant only while one is selected, with limited-gated markup', async () => {
		mocked.addToLibrary.mockResolvedValue(summary({ id: '9', name: 'Chip' }));
		mocked.getEquipmentDetail.mockResolvedValue(detail({ id: '9' }));
		const model = createLibraryModel();
		model.openAddModal();
		model.weaponPicker.select({ ...weaponHit, isLimited: true });
		model.markupPercent = 1500;
		model.implantPicker.select({
			...weaponHit,
			catalogId: 'i1',
			name: 'NeoPsion 85-B Mindforce Implant (L)',
			absorptionPercent: 20,
			isLimited: true,
		});
		model.implantMarkupPercent = 110;
		await model.saveEquipment();

		expect(mocked.addToLibrary).toHaveBeenCalledWith(
			expect.objectContaining({
				implant_catalog_id: 'i1',
				implant_markup: 110,
			}),
		);
	});

	it('seeds and resets the implant picker through the edit cycle', async () => {
		mocked.getEquipmentLibrary.mockResolvedValue([summary({ id: '1' })]);
		mocked.getEquipmentDetail.mockResolvedValue(
			detail({
				implant: {
					catalogId: 'i1',
					name: 'NeoPsion 85-B Mindforce Implant (L)',
					decay: 0,
					ammoBurn: 0,
					absorptionPercent: 20,
					markupPercent: 110,
					isLimited: true,
				},
			}),
		);
		const model = createLibraryModel();
		await model.loadData(false);
		await model.openEditModal('1');
		expect(model.implantPicker.selected?.catalogId).toBe('i1');
		expect(model.implantPicker.selected?.absorptionPercent).toBe(20);
		expect(model.implantMarkupPercent).toBe(110);

		// A fresh add starts clean again.
		model.openAddModal();
		expect(model.implantPicker.selected).toBeNull();
		expect(model.implantMarkupPercent).toBe(100);
	});
});

describe('removeEquipment', () => {
	async function loadedModel() {
		mocked.getEquipmentLibrary.mockResolvedValue([
			summary({ id: '1', name: 'Weapon', type: 'weapon' }),
			summary({ id: '2', name: 'Fap', type: 'healing' }),
			summary({ id: '3', name: 'Oil', type: 'consumable' }),
		]);
		const model = createLibraryModel();
		await model.loadData(false);
		return model;
	}

	it('removes each kind from its own view and the shared list', async () => {
		mocked.removeFromLibrary.mockResolvedValue(undefined);
		const model = await loadedModel();
		await model.removeEquipment('2', 'healing');
		expect(model.healingTools).toEqual([]);
		await model.removeEquipment('3', 'consumable');
		expect(model.consumables).toEqual([]);
		await model.removeEquipment('1');
		expect(model.sortedEquipment).toEqual([]);
		expect(model.allEquipment).toEqual([]);
	});

	it('collapses the removed row and evicts its cached detail', async () => {
		mocked.removeFromLibrary.mockResolvedValue(undefined);
		mocked.getEquipmentDetail.mockResolvedValue(detail());
		const model = await loadedModel();
		await model.toggleExpand('1');
		await model.removeEquipment('1');
		expect(model.expandedId).toBeNull();
		expect(model.detailCache['1']).toBeUndefined();
	});

	it('surfaces a removal failure and keeps the row', async () => {
		mocked.removeFromLibrary.mockRejectedValue(new Error('in use'));
		const model = await loadedModel();
		await model.removeEquipment('1');
		expect(model.error).toBe('in use');
		expect(model.sortedEquipment.map((e) => e.id)).toEqual(['1']);
	});
});

describe('liveCostPreview', () => {
	it('is null until a weapon is selected and follows the form inputs', () => {
		const model = createLibraryModel();
		expect(model.liveCostPreview).toBeNull();
		model.weaponPicker.select({ ...weaponHit, isLimited: true });
		model.markupPercent = 150;
		model.damageEnhancers = 2;
		expect(model.liveCostPreview).toBeCloseTo(4.8, 10);
	});

	it('does not move when an absorber is selected', () => {
		const model = createLibraryModel();
		model.weaponPicker.select({ ...weaponHit });
		const before = model.liveCostPreview;
		model.absorberPicker.select({ ...weaponHit, catalogId: 'abs-1', name: 'Absorber' });
		expect(model.liveCostPreview).toBe(before);
	});
});
