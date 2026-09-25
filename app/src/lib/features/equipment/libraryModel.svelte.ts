/**
 * Equipment-library view model: the library data set split by kind, the
 * row expansion and detail cache, the add/edit form with its catalogue
 * pickers, and the CRUD handlers. Presentation lives in the feature
 * components; they compose over this state.
 */

import {
	addToLibrary,
	type EquipmentSearchResult,
	getEquipmentDetail,
	getEquipmentLibrary,
	getSettings,
	hotbarFromSettings,
	removeFromLibrary,
	searchEquipmentItems,
	updateLibrary,
} from '$lib/api';
import {
	equipmentDemoCarriedWeaponIds,
	equipmentDemoDetails,
	equipmentDemoHotbar,
	equipmentDemoLibrary,
} from '$lib/guide/fixtures/equipment';
import type { Equipment, EquipmentDetail, HealingMode, HealingTool } from '$lib/types';
import type {
	HarvestGuardrailSettings,
	Hotbar,
	PassiveEffectSourceView,
} from '$lib/types/settings';
import { describeError } from '$lib/view/errorState';
import { createTypeahead } from '$lib/view/typeahead.svelte';
import { previewCostPerUse } from './costPreview';

export type EquipmentFormType = 'weapon' | 'healing' | 'consumable' | 'tool';

export function createLibraryModel() {
	// ── Data ──
	let allEquipment = $state<Equipment[]>([]);
	let equipmentList = $state<Equipment[]>([]);
	let healingTools = $state<HealingTool[]>([]);
	let consumables = $state<Equipment[]>([]);
	let harvestingTools = $state<Equipment[]>([]);
	let hotbar = $state<Hotbar>({});
	let hotbarHooksEnabled = $state(true);
	let carriedWeaponIds = $state<number[]>([]);
	let harvestGuardrail = $state<HarvestGuardrailSettings>({
		enabled: false,
		shortToolId: null,
		longToolId: null,
		hugeToolId: null,
	});
	let passiveEffectSources = $state<PassiveEffectSourceView[]>([]);
	// True until the first load settles: an empty library before then means
	// "not read yet", not "nothing added", so the views hold their empty states.
	let loading = $state(true);
	let error = $state<string | null>(null);

	// ── Rows ──
	let expandedId = $state<string | null>(null);
	let detailCache = $state<Record<string, EquipmentDetail>>({});

	// ── Form modal ──
	let showAddModal = $state(false);
	let addType = $state<EquipmentFormType>('weapon');
	let editingEquipmentId = $state<string | null>(null);
	let saving = $state(false);
	let markupPercent = $state(100);
	let ampMarkupPercent = $state(100);
	let scopeMarkupPercent = $state(100);
	let absorberMarkupPercent = $state(100);
	let damageEnhancers = $state(0);
	let implantMarkupPercent = $state(100);
	let healingMode = $state<HealingMode>('direct');
	let healMin = $state<number | null>(null);
	let healMax = $state<number | null>(null);
	let effectDurationSeconds = $state<number | null>(null);
	let tickMin = $state<number | null>(null);
	let tickMax = $state<number | null>(null);
	let tickSeconds = $state<number | null>(null);
	let seededHealerId = $state<string | null>(null);

	// ── Catalogue pickers ──
	const label = (item: EquipmentSearchResult) => item.name;
	const weaponPicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'weapon'),
		labelOf: label,
	});
	// The amp list never offers the selected weapon back as its own amp.
	const ampPicker = createTypeahead<EquipmentSearchResult>({
		search: async (q) => {
			const results = await searchEquipmentItems(q, 'amp');
			return results.filter((r) => r.name !== weaponPicker.selected?.name);
		},
		labelOf: label,
	});
	const healerPicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'healer'),
		labelOf: label,
	});
	const scopePicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'scope'),
		labelOf: label,
	});
	const absorberPicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'absorber'),
		labelOf: label,
	});
	const implantPicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'implant'),
		labelOf: label,
	});
	const consumablePicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'consumable'),
		labelOf: label,
	});
	const toolPicker = createTypeahead<EquipmentSearchResult>({
		search: (q) => searchEquipmentItems(q, 'tool'),
		labelOf: label,
	});
	const pickers = [
		weaponPicker,
		ampPicker,
		healerPicker,
		scopePicker,
		absorberPicker,
		implantPicker,
		consumablePicker,
		toolPicker,
	];

	$effect(() => {
		const selected = healerPicker.selected;
		if (!selected?.catalogId || selected.catalogId === seededHealerId || editingEquipmentId) return;
		seededHealerId = selected.catalogId;
		healingMode = 'direct';
		healMin = selected.healMin;
		healMax = selected.healMax;
		effectDurationSeconds = null;
		tickMin = null;
		tickMax = null;
		tickSeconds = null;
	});

	// ── Computed ──
	const sortedEquipment = $derived([...equipmentList].sort((a, b) => a.name.localeCompare(b.name)));

	const liveCostPreview = $derived(
		previewCostPerUse({
			weapon: weaponPicker.selected,
			amp: ampPicker.selected,
			scope: scopePicker.selected,
			absorber: absorberPicker.selected,
			implant: implantPicker.selected,
			markupPercent,
			ampMarkupPercent,
			scopeMarkupPercent,
			absorberMarkupPercent,
			implantMarkupPercent,
			damageEnhancers,
		}),
	);

	// Split the flat library into the per-kind lists the view renders. Weapons
	// stay unsorted here; the view's sorted derivation owns their ordering.
	function splitByKind(library: Equipment[]) {
		equipmentList = library.filter((e) => e.type === 'weapon');
		healingTools = library
			.filter((e) => e.type === 'healing')
			.map((e) => ({
				id: e.id,
				name: e.name,
				costPerHeal: e.costPerUse,
				isLimited: e.isLimited,
				reloadSeconds: e.reloadSeconds,
				profile: e.healingProfile,
			}))
			.sort((a, b) => a.name.localeCompare(b.name));
		consumables = library
			.filter((e) => e.type === 'consumable')
			.sort((a, b) => a.name.localeCompare(b.name));
		harvestingTools = library
			.filter((e) => e.type === 'tool')
			.sort((a, b) => a.name.localeCompare(b.name));
	}

	async function loadData(guideMode: boolean): Promise<void> {
		try {
			if (guideMode) {
				const library = equipmentDemoLibrary.map((e) => ({ ...e }));
				allEquipment = library;
				splitByKind(library);
				hotbar = { ...equipmentDemoHotbar };
				hotbarHooksEnabled = true;
				carriedWeaponIds = [...equipmentDemoCarriedWeaponIds];
				passiveEffectSources = [];
				detailCache = Object.fromEntries(
					Object.entries(equipmentDemoDetails).map(([k, v]) => [k, { ...v }]),
				);
			} else {
				const [library, settings] = await Promise.all([getEquipmentLibrary(), getSettings()]);
				allEquipment = library;
				splitByKind(library);
				hotbar = hotbarFromSettings(settings);
				hotbarHooksEnabled = settings.hotbarHooksEnabled;
				carriedWeaponIds = [...settings.carriedWeaponIds];
				harvestGuardrail = settings.harvestGuardrail;
				passiveEffectSources = (settings.passiveEffectSources ?? []).map((source) => ({
					...source,
					effects: source.effects.map((effect) => ({ ...effect })),
				}));
				detailCache = {};
			}
		} catch (e) {
			error = describeError(e, 'Failed to load equipment');
		} finally {
			loading = false;
		}
	}

	function replaceEquipment(updated: Equipment) {
		allEquipment = allEquipment.some((item) => item.id === updated.id)
			? allEquipment.map((item) => (item.id === updated.id ? updated : item))
			: [...allEquipment, updated];
		splitByKind(allEquipment);
	}

	function openAddModal(prefill?: string, type: EquipmentFormType = 'weapon') {
		editingEquipmentId = null;
		addType = type;
		for (const picker of pickers) picker.clear();
		if (prefill) weaponPicker.query = prefill;
		markupPercent = 100;
		ampMarkupPercent = 100;
		scopeMarkupPercent = 100;
		absorberMarkupPercent = 100;
		damageEnhancers = 0;
		implantMarkupPercent = 100;
		healingMode = 'direct';
		healMin = null;
		healMax = null;
		effectDurationSeconds = null;
		tickMin = null;
		tickMax = null;
		tickSeconds = null;
		seededHealerId = null;
		showAddModal = true;
	}

	async function openEditModal(id: string) {
		let detail = detailCache[id];
		if (!detail) {
			try {
				detail = await getEquipmentDetail(id);
			} catch (e) {
				error = describeError(e, 'Failed to load equipment detail');
				return;
			}
		}
		detailCache[id] = detail;
		editingEquipmentId = id;
		addType = detail.type;
		const primary = {
			catalogId: detail.weapon.catalogId,
			name: detail.weapon.name,
			decay: detail.weapon.decay,
			ammoBurn: detail.weapon.ammoBurn,
			markupPercent: detail.weapon.markupPercent,
			isLimited: detail.weapon.isLimited,
			absorptionPercent: null,
			damageEnhancers: detail.weapon.damageEnhancers,
			healMin: detail.healingProfile?.directMin ?? null,
			healMax: detail.healingProfile?.directMax ?? null,
			reloadSeconds: null,
			lifestealPercent: detail.lifestealPercent,
		};
		if (detail.type === 'healing') healerPicker.select(primary);
		else weaponPicker.select(primary);
		if (detail.amplifier) {
			selectCompanion(ampPicker, detail.amplifier);
		} else {
			ampPicker.clear();
		}
		if (detail.scope) {
			scopePicker.select({
				catalogId: detail.scope.catalogId,
				name: detail.scope.name,
				decay: detail.scope.decay,
				ammoBurn: detail.scope.ammoBurn,
				markupPercent: detail.scope.markupPercent,
				isLimited: detail.scope.isLimited,
				absorptionPercent: null,
				damageEnhancers: detail.scope.damageEnhancers,
				healMin: null,
				healMax: null,
				reloadSeconds: null,
				lifestealPercent: null,
			});
		} else {
			scopePicker.clear();
		}
		if (detail.absorber) {
			selectCompanion(absorberPicker, detail.absorber);
		} else {
			absorberPicker.clear();
		}
		if (detail.implant) {
			selectCompanion(implantPicker, detail.implant);
		} else {
			implantPicker.clear();
		}
		if (detail.type !== 'healing') healerPicker.clear();
		markupPercent = detail.weapon.markupPercent;
		ampMarkupPercent = detail.amplifier?.markupPercent ?? 100;
		scopeMarkupPercent = detail.scope?.markupPercent ?? 100;
		absorberMarkupPercent = detail.absorber?.markupPercent ?? 100;
		implantMarkupPercent = detail.implant?.markupPercent ?? 100;
		damageEnhancers = detail.weapon.damageEnhancers;
		healingMode = detail.healingProfile?.mode ?? 'direct';
		healMin = detail.healingProfile?.directMin ?? null;
		healMax = detail.healingProfile?.directMax ?? null;
		effectDurationSeconds = detail.healingProfile?.effectDurationSeconds ?? null;
		tickMin = detail.healingProfile?.tickMin ?? null;
		tickMax = detail.healingProfile?.tickMax ?? null;
		tickSeconds = detail.healingProfile?.tickSeconds ?? null;
		seededHealerId = detail.type === 'healing' ? detail.weapon.catalogId : null;
		showAddModal = true;
	}

	// Amp, absorber and implant components seed with no enhancer slots of
	// their own; absorption rides along for the cost preview when present.
	function selectCompanion(
		picker: (typeof pickers)[number],
		component: {
			catalogId: string | null;
			name: string;
			decay: number;
			ammoBurn: number;
			markupPercent: number;
			isLimited: boolean;
			absorptionPercent?: number;
		},
	) {
		picker.select({
			catalogId: component.catalogId,
			name: component.name,
			decay: component.decay,
			ammoBurn: component.ammoBurn,
			markupPercent: component.markupPercent,
			isLimited: component.isLimited,
			absorptionPercent: component.absorptionPercent ?? null,
			damageEnhancers: 0,
			healMin: null,
			healMax: null,
			reloadSeconds: null,
			lifestealPercent: null,
		});
	}

	function setAddType(type: EquipmentFormType) {
		addType = type;
		if (type === 'weapon') {
			healerPicker.clear();
			toolPicker.clear();
		} else if (type === 'healing') {
			weaponPicker.clear();
			ampPicker.clear();
			toolPicker.clear();
		} else if (type === 'tool') {
			weaponPicker.clear();
			ampPicker.clear();
			healerPicker.clear();
			implantPicker.clear();
		} else {
			weaponPicker.clear();
			ampPicker.clear();
			healerPicker.clear();
			toolPicker.clear();
			implantPicker.clear();
		}
	}

	function selectConsumableCustom(name: string) {
		const trimmed = name.trim();
		if (!trimmed) return;
		consumablePicker.select({
			catalogId: null,
			name: trimmed,
			decay: 0,
			ammoBurn: 0,
			absorptionPercent: null,
			isLimited: false,
			healMin: null,
			healMax: null,
			reloadSeconds: null,
			lifestealPercent: null,
		});
	}

	async function toggleExpand(id: string) {
		if (expandedId === id) {
			expandedId = null;
			return;
		}
		expandedId = id;
		if (!detailCache[id]) {
			try {
				detailCache[id] = await getEquipmentDetail(id);
			} catch (e) {
				error = describeError(e, 'Failed to load equipment detail');
			}
		}
	}

	async function saveEquipment() {
		error = null;
		saving = true;
		try {
			if (addType === 'weapon') {
				const weapon = weaponPicker.selected;
				if (!weapon?.catalogId) return;
				const payload = {
					type: 'weapon' as const,
					catalog_id: weapon.catalogId,
					amp_catalog_id: ampPicker.selected?.catalogId ?? null,
					scope_catalog_id: scopePicker.selected?.catalogId ?? null,
					absorber_catalog_id: absorberPicker.selected?.catalogId ?? null,
					weapon_markup: weapon.isLimited ? markupPercent : 100,
					amp_markup: ampPicker.selected?.isLimited ? ampMarkupPercent : 100,
					scope_markup: scopePicker.selected?.isLimited ? scopeMarkupPercent : 100,
					absorber_markup: absorberPicker.selected?.isLimited ? absorberMarkupPercent : 100,
					damage_enhancers: damageEnhancers,
					implant_catalog_id: implantPicker.selected?.catalogId ?? null,
					implant_markup: implantPicker.selected?.isLimited ? implantMarkupPercent : 100,
				};
				const item = editingEquipmentId
					? await updateLibrary(editingEquipmentId, payload)
					: await addToLibrary(payload);
				replaceEquipment(item);
				// The save has succeeded, so close the form before the detail fetch:
				// a fetch failure must not leave the modal open with a retry path
				// that would create a duplicate entry.
				showAddModal = false;
				editingEquipmentId = null;
				delete detailCache[item.id];
				detailCache[item.id] = await getEquipmentDetail(item.id);
			} else if (addType === 'healing') {
				const healer = healerPicker.selected;
				if (!healer?.catalogId) return;
				const payload = {
					type: 'healing',
					catalog_id: healer.catalogId,
					weapon_markup: healer.isLimited ? markupPercent : 100,
					implant_catalog_id: implantPicker.selected?.catalogId ?? null,
					implant_markup: implantPicker.selected?.isLimited ? implantMarkupPercent : 100,
					healing_mode: healingMode,
					heal_min: healMin ?? healer.healMin,
					heal_max: healMax ?? healer.healMax,
					effect_duration_seconds: effectDurationSeconds,
					tick_min: tickMin,
					tick_max: tickMax,
					tick_seconds: tickSeconds,
				} as const;
				const item = editingEquipmentId
					? await updateLibrary(editingEquipmentId, payload)
					: await addToLibrary(payload);
				replaceEquipment(item);
				showAddModal = false;
				editingEquipmentId = null;
				delete detailCache[item.id];
				detailCache[item.id] = await getEquipmentDetail(item.id);
			} else if (addType === 'tool') {
				const tool = toolPicker.selected;
				if (!tool?.catalogId) return;
				const item = await addToLibrary({
					type: 'tool',
					catalog_id: tool.catalogId,
					weapon_markup: tool.isLimited ? markupPercent : 100,
				});
				replaceEquipment(item);
			} else {
				const consumable = consumablePicker.selected;
				if (!consumable) return;
				const item = await addToLibrary({
					type: 'consumable',
					catalog_id: consumable.catalogId ?? null,
					name: consumable.catalogId ? null : consumable.name,
				});
				replaceEquipment(item);
			}
			showAddModal = false;
			editingEquipmentId = null;
		} catch (e) {
			error = describeError(e, 'Failed to save equipment');
		} finally {
			saving = false;
		}
	}

	async function removeEquipment(id: string, type: EquipmentFormType = 'weapon') {
		error = null;
		try {
			await removeFromLibrary(id);
			allEquipment = allEquipment.filter((e) => e.id !== id);
			if (type === 'healing') {
				healingTools = healingTools.filter((e) => e.id !== id);
			} else if (type === 'consumable') {
				consumables = consumables.filter((e) => e.id !== id);
			} else if (type === 'tool') {
				harvestingTools = harvestingTools.filter((e) => e.id !== id);
			} else {
				equipmentList = equipmentList.filter((e) => e.id !== id);
			}
			if (expandedId === id) expandedId = null;
			delete detailCache[id];
		} catch (e) {
			error = describeError(e, 'Failed to remove equipment');
		}
	}

	function destroy() {
		for (const picker of pickers) picker.destroy();
	}

	return {
		// ── Data ──
		get loading() {
			return loading;
		},
		get allEquipment() {
			return allEquipment;
		},
		get sortedEquipment() {
			return sortedEquipment;
		},
		get healingTools() {
			return healingTools;
		},
		get consumables() {
			return consumables;
		},
		get harvestingTools() {
			return harvestingTools;
		},
		get hotbar() {
			return hotbar;
		},
		set hotbar(value: Hotbar) {
			hotbar = value;
		},
		get hotbarHooksEnabled() {
			return hotbarHooksEnabled;
		},
		get carriedWeaponIds() {
			return carriedWeaponIds;
		},
		set carriedWeaponIds(value: number[]) {
			carriedWeaponIds = value;
		},
		get harvestGuardrail() {
			return harvestGuardrail;
		},
		set harvestGuardrail(value: HarvestGuardrailSettings) {
			harvestGuardrail = value;
		},
		get passiveEffectSources() {
			return passiveEffectSources;
		},
		set passiveEffectSources(value: PassiveEffectSourceView[]) {
			passiveEffectSources = value;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},

		// ── Rows ──
		get expandedId() {
			return expandedId;
		},
		set expandedId(value: string | null) {
			expandedId = value;
		},
		get detailCache() {
			return detailCache;
		},

		// ── Form modal ──
		get showAddModal() {
			return showAddModal;
		},
		set showAddModal(value: boolean) {
			showAddModal = value;
			// Any close path (cancel, backdrop, escape) drops the edit target.
			if (!value) editingEquipmentId = null;
		},
		get addType() {
			return addType;
		},
		get editingEquipmentId() {
			return editingEquipmentId;
		},
		get saving() {
			return saving;
		},
		get markupPercent() {
			return markupPercent;
		},
		set markupPercent(value: number) {
			markupPercent = value;
		},
		get ampMarkupPercent() {
			return ampMarkupPercent;
		},
		set ampMarkupPercent(value: number) {
			ampMarkupPercent = value;
		},
		get scopeMarkupPercent() {
			return scopeMarkupPercent;
		},
		set scopeMarkupPercent(value: number) {
			scopeMarkupPercent = value;
		},
		get absorberMarkupPercent() {
			return absorberMarkupPercent;
		},
		set absorberMarkupPercent(value: number) {
			absorberMarkupPercent = value;
		},
		get damageEnhancers() {
			return damageEnhancers;
		},
		set damageEnhancers(value: number) {
			damageEnhancers = value;
		},
		get liveCostPreview() {
			return liveCostPreview;
		},
		get implantMarkupPercent() {
			return implantMarkupPercent;
		},
		set implantMarkupPercent(value: number) {
			implantMarkupPercent = value;
		},
		get healingMode() {
			return healingMode;
		},
		set healingMode(value: HealingMode) {
			healingMode = value;
		},
		get healMin() {
			return healMin;
		},
		set healMin(value: number | null) {
			healMin = value;
		},
		get healMax() {
			return healMax;
		},
		set healMax(value: number | null) {
			healMax = value;
		},
		get effectDurationSeconds() {
			return effectDurationSeconds;
		},
		set effectDurationSeconds(value: number | null) {
			effectDurationSeconds = value;
		},
		get tickMin() {
			return tickMin;
		},
		set tickMin(value: number | null) {
			tickMin = value;
		},
		get tickMax() {
			return tickMax;
		},
		set tickMax(value: number | null) {
			tickMax = value;
		},
		get tickSeconds() {
			return tickSeconds;
		},
		set tickSeconds(value: number | null) {
			tickSeconds = value;
		},

		// ── Pickers ──
		weaponPicker,
		ampPicker,
		healerPicker,
		scopePicker,
		absorberPicker,
		implantPicker,
		consumablePicker,
		toolPicker,

		loadData,
		openAddModal,
		openEditModal,
		setAddType,
		selectConsumableCustom,
		toggleExpand,
		saveEquipment,
		removeEquipment,
		destroy,
	};
}

export type LibraryModel = ReturnType<typeof createLibraryModel>;
