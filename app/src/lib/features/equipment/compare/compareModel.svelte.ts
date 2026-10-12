/**
 * The comparator: a list of setup drafts (a catalogue weapon with an
 * optional amplifier, absorber, and implant, each limited item at a markup)
 * and the comparison the backend computed for the complete ones.
 *
 * One comparator lives for the app session ([`sessionComparator`]), so
 * leaving the tab and coming back keeps the list; a restart starts clean.
 * Nothing is persisted.
 */

import {
	type ComparedSetup,
	type ComponentPick,
	compareLoadouts,
	type EquipmentSearchResult,
	type LoadoutComparison,
	type SetupComponentRole,
	type SetupPick,
	searchEquipmentItems,
} from '$lib/api';
import { getCharacterProfessions } from '$lib/api/character';
import type { SearchKind } from '$lib/api/commands.gen';
import {
	LOOTER_BASES,
	type LooterBasis,
	type LooterLevels,
	looterBasis,
	looterBasisLevel,
	looterLevelsFrom,
} from '$lib/looterBasis.svelte';
import { describeError } from '$lib/view/errorState';
import { createTypeahead, type Typeahead } from '$lib/view/typeahead.svelte';
import { isLimitedSetup, lossPerKill, parseAmount } from './comparisonView';

export const SETUP_ROLES: SetupComponentRole[] = ['weapon', 'amplifier', 'absorber', 'implant'];
export const OPTIONAL_ROLES: SetupComponentRole[] = ['amplifier', 'absorber', 'implant'];

const SEARCH_KIND: Record<SetupComponentRole, SearchKind> = {
	weapon: 'weapon',
	amplifier: 'amp',
	absorber: 'absorber',
	implant: 'implant',
};

export const ROLE_LABEL: Record<SetupComponentRole, string> = {
	weapon: 'Weapon',
	amplifier: 'Amplifier',
	absorber: 'Absorber',
	implant: 'Implant',
};

/** A limited item's markup until the player sets one: TT, as the library's
 * add form starts. */
export const DEFAULT_MARKUP_PERCENT = 100;

/** Why a markup cannot be used, or null when it can. */
export function markupProblem(label: string, markup: number | null): string | null {
	if (markup === null || !Number.isFinite(markup) || markup < 100) {
		return `${label} markup is a percentage of at least 100.`;
	}
	return null;
}

/** The HP range the backend accepts for the curve's reach. */
export const MIN_CURVE_HP = 10;
export const MAX_CURVE_HP = 100_000;

/** The most setups compared at once: one per chart colour. */
export const MAX_SETUPS = 8;

/** One setup's draft: a picker per part and the markup typed for each.
 * `slot` is its chart colour, kept for as long as the setup is listed. */
export function createSetupDraft(id: number, slot: number) {
	const pickers = Object.fromEntries(
		SETUP_ROLES.map((role) => [
			role,
			createTypeahead<EquipmentSearchResult>({
				search: (query) => searchEquipmentItems(query, SEARCH_KIND[role]),
				labelOf: (item) => item.name,
			}),
		]),
	) as Record<SetupComponentRole, Typeahead<EquipmentSearchResult>>;

	// A markup belongs to the item it was typed for: picking another item
	// starts it again at TT.
	const markups = $state<Record<SetupComponentRole, { id: string | null; percent: number | null }>>(
		{
			weapon: { id: null, percent: DEFAULT_MARKUP_PERCENT },
			amplifier: { id: null, percent: DEFAULT_MARKUP_PERCENT },
			absorber: { id: null, percent: DEFAULT_MARKUP_PERCENT },
			implant: { id: null, percent: DEFAULT_MARKUP_PERCENT },
		},
	);
	// What an unlimited part's markup cost, as typed: capital held while
	// the item is owned. Like a markup, it belongs to the item it was typed for.
	const capitals = $state<Record<SetupComponentRole, { id: string | null; text: string }>>({
		weapon: { id: null, text: '' },
		amplifier: { id: null, text: '' },
		absorber: { id: null, text: '' },
		implant: { id: null, text: '' },
	});
	function capitalTextFor(role: SetupComponentRole): string {
		const id = pickers[role].selected?.catalogId ?? null;
		return capitals[role].id === id ? capitals[role].text : '';
	}

	// Optional parts the player has opened, so an empty picker stays put.
	const shown = $state<Record<SetupComponentRole, boolean>>({
		weapon: true,
		amplifier: false,
		absorber: false,
		implant: false,
	});

	function markupFor(role: SetupComponentRole): number | null {
		const id = pickers[role].selected?.catalogId ?? null;
		return markups[role].id === id ? markups[role].percent : DEFAULT_MARKUP_PERCENT;
	}

	function problem(): string | null {
		if (!pickers.weapon.selected) return null;
		for (const role of SETUP_ROLES) {
			const selected = pickers[role].selected;
			if (!selected?.isLimited) continue;
			const issue = markupProblem(ROLE_LABEL[role], markupFor(role));
			if (issue) return issue;
		}
		return null;
	}

	function component(role: SetupComponentRole): ComponentPick | null {
		const selected = pickers[role].selected;
		if (!selected?.catalogId) return null;
		return {
			catalogId: selected.catalogId,
			markupPercent: selected.isLimited ? markupFor(role) : null,
		};
	}

	return {
		id,
		slot,
		pickers,
		markupFor,
		setMarkup(role: SetupComponentRole, percent: number | null) {
			markups[role] = { id: pickers[role].selected?.catalogId ?? null, percent };
		},
		capitalTextFor,
		setCapitalText(role: SetupComponentRole, text: string) {
			capitals[role] = { id: pickers[role].selected?.catalogId ?? null, text };
		},
		/** The markup paid for the setup's unlimited parts, in PED. */
		get capitalPed(): number {
			return SETUP_ROLES.reduce((total, role) => {
				const selected = pickers[role].selected;
				if (!selected || selected.isLimited) return total;
				return total + (parseAmount(capitalTextFor(role)) ?? 0);
			}, 0);
		},
		isShown(role: SetupComponentRole) {
			return shown[role] || pickers[role].selected !== null;
		},
		show(role: SetupComponentRole) {
			shown[role] = true;
		},
		/** Take an optional part off the setup. */
		removePart(role: SetupComponentRole) {
			pickers[role].clear();
			shown[role] = false;
		},
		get problem() {
			return problem();
		},
		/** The request for this setup, or null until it has a weapon and valid markups. */
		pick(): SetupPick | null {
			const weapon = component('weapon');
			if (!weapon || problem() !== null) return null;
			return {
				weapon,
				amplifier: component('amplifier'),
				absorber: component('absorber'),
				implant: component('implant'),
			};
		},
	};
}

export type SetupDraft = ReturnType<typeof createSetupDraft>;

export function createComparator() {
	// Raw: drafts carry their own reactive state, and the list is only ever
	// replaced, never mutated.
	let drafts = $state.raw<SetupDraft[]>([]);
	/** The trailing empty row: picking its weapon makes it a listed setup. */
	let blank = $state.raw<SetupDraft | null>(null);
	let comparison = $state.raw<LoadoutComparison | null>(null);
	/** The draft ids the shown comparison answers, in its setups' order. */
	let computedIds = $state.raw<number[]>([]);
	let computedKey = $state<string | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	/** The mob HP the per-kill figures answer for; the chart pins it too. */
	let pinnedHp = $state(100);
	/** The HP slider's upper end, which is also how far the curve reaches. */
	let hpMax = $state(3000);
	/** The kills a year the table answers for, and its slider's upper end. */
	let killCount = $state(10_000);
	let killMax = $state(1_000_000);
	/** The yearly rate capital held in unlimited gear is charged at. */
	let capitalRate = $state(0.1);
	/** The limited setup an unlimited one is weighed against; null follows
	 * the best limited setup listed. */
	let referenceId = $state<number | null>(null);
	/** A looter level to compare at instead of the app-wide basis's. */
	let looterLevel = $state<number | null>(null);
	/** The player's three hunting looters, read up front so the looter
	 * slider and its ticks stand before any setup is listed. */
	let ownLooterLevels = $state<LooterLevels | null>(null);
	void getCharacterProfessions()
		.then((professions) => {
			ownLooterLevels = looterLevelsFrom(professions);
		})
		.catch(() => {
			ownLooterLevels = { animal: 0, mutant: 0, robot: 0 };
		});

	/** The community model counts looter skill up to 100, so the slider and
	 * its ticks stop there. */
	const LOOTER_CEILING = 100;
	/** Where each basis sits on the looter slider, in menu order. */
	function looterTicks(): { basis: LooterBasis; value: number }[] {
		const levels = ownLooterLevels;
		if (!levels) return [];
		return LOOTER_BASES.map((basis) => ({
			basis,
			value: Math.min(LOOTER_CEILING, looterBasisLevel(levels, basis)),
		}));
	}
	/** The level the app-wide basis stands for, which the slider starts at;
	 * null until the player's levels are read. */
	function basisLevel(): number | null {
		return looterTicks().find((tick) => tick.basis === looterBasis.current)?.value ?? null;
	}
	/** The basis a looter level stands for, or null for a level between
	 * them. Where levels coincide, the app-wide basis names it. */
	function basisOfLevel(level: number): LooterBasis | null {
		const matches = looterTicks().filter((tick) => Math.abs(tick.value - level) < 0.05);
		return (
			matches.find((tick) => tick.basis === looterBasis.current)?.basis ?? matches[0]?.basis ?? null
		);
	}
	/** The level sent with a request. Null asks for the player's own mean,
	 * so a basis other than the mean sends its level. */
	function requestedLooterLevel(): number | null {
		if (looterLevel !== null) return looterLevel;
		return looterBasis.current === 'three_looter_mean' ? null : basisLevel();
	}
	let sequence = 0;
	let nextId = 1;

	function request(): {
		setups: SetupPick[];
		ids: number[];
		hpMax: number;
		looterLevel: number | null;
		key: string;
	} | null {
		const complete = drafts.flatMap((draft) => {
			const pick = draft.pick();
			return pick ? [{ id: draft.id, pick }] : [];
		});
		if (complete.length === 0) return null;
		const setups = complete.map((entry) => entry.pick);
		const ids = complete.map((entry) => entry.id);
		const level = requestedLooterLevel();
		return {
			setups,
			ids,
			hpMax,
			looterLevel: level,
			key: JSON.stringify([ids, setups, hpMax, level]),
		};
	}

	function figures(id: number): ComparedSetup | null {
		const index = computedIds.indexOf(id);
		return index === -1 ? null : (comparison?.setups[index] ?? null);
	}
	function referenceDraftId(): number | null {
		const limited = drafts.filter((draft) => {
			const setup = figures(draft.id);
			return setup !== null && isLimitedSetup(setup);
		});
		if (referenceId !== null && limited.some((draft) => draft.id === referenceId))
			return referenceId;
		let best: number | null = null;
		let bestCost = Infinity;
		for (const draft of limited) {
			const cost = lossPerKill(figures(draft.id) as ComparedSetup, pinnedHp);
			if (cost !== null && cost < bestCost) [best, bestCost] = [draft.id, cost];
		}
		return best;
	}
	/** Keep one empty row at the end of the list while there is room. */
	function ensureBlank() {
		if (blank !== null || drafts.length >= MAX_SETUPS) return;
		const used = new Set(drafts.map((draft) => draft.slot));
		const slot = Array.from({ length: MAX_SETUPS }, (_, index) => index + 1).find(
			(candidate) => !used.has(candidate),
		);
		if (slot !== undefined) blank = createSetupDraft(nextId++, slot);
	}
	ensureBlank();

	return {
		get drafts() {
			return drafts;
		},
		get blank(): SetupDraft | null {
			return blank;
		},
		draft(id: number): SetupDraft | undefined {
			return drafts.find((draft) => draft.id === id);
		},
		/** List the empty row once its weapon is picked, and open a new one. */
		commitBlank() {
			if (!blank?.pickers.weapon.selected) return;
			drafts = [...drafts, blank];
			blank = null;
			ensureBlank();
		},
		remove(id: number) {
			drafts = drafts.filter((draft) => draft.id !== id);
			ensureBlank();
		},
		/** The computed figures for a listed setup, if the shown comparison
		 * has them. */
		figuresFor(id: number): ComparedSetup | null {
			const index = computedIds.indexOf(id);
			return index === -1 ? null : (comparison?.setups[index] ?? null);
		},
		get comparison() {
			return comparison;
		},
		/** The draft each of the shown comparison's setups came from. */
		get computedIds(): number[] {
			return computedIds;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		set error(value: string | null) {
			error = value;
		},
		get killCount() {
			return killCount;
		},
		set killCount(value: number) {
			if (Number.isFinite(value)) killCount = Math.min(killMax, Math.max(0, Math.round(value)));
		},
		get killMax() {
			return killMax;
		},
		set killMax(value: number) {
			if (!Number.isFinite(value) || value < 1) return;
			killMax = Math.round(value);
			killCount = Math.min(killCount, killMax);
		},
		get capitalRate() {
			return capitalRate;
		},
		set capitalRate(value: number) {
			if (Number.isFinite(value) && value > 0) capitalRate = value;
		},
		get referenceId() {
			return referenceId !== null && drafts.some((draft) => draft.id === referenceId)
				? referenceId
				: null;
		},
		/** The limited setup unlimited ones are weighed against: the one
		 * picked, or else the limited setup that loses least per kill. */
		get referenceDraftId(): number | null {
			return referenceDraftId();
		},
		get looterLevel() {
			return looterLevel;
		},
		/** The player's own three-looter mean: the shown comparison's when
		 * there is one. */
		get ownLooterLevel(): number | null {
			const levels = ownLooterLevels;
			return (
				comparison?.assumptions.ownLooterLevel ??
				(levels ? looterBasisLevel(levels, 'three_looter_mean') : null)
			);
		},
		/** The level the looter slider shows: the one slid to, or else the
		 * app-wide basis's; null until the player's levels are read. */
		get sliderLooterLevel(): number | null {
			return looterLevel ?? basisLevel();
		},
		/** The level the app-wide basis stands for, where the slider rests. */
		get basisLooterLevel(): number | null {
			return basisLevel();
		},
		/** The mean and each looter, marked on the slider for a quick set. */
		get looterTicks() {
			return looterTicks();
		},
		/** The basis the slider's level stands for, or null for a level
		 * between them. */
		get looterBasisAt(): LooterBasis | null {
			const shown = looterLevel ?? basisLevel();
			return shown === null ? null : basisOfLevel(shown);
		},
		basisOfLevel,
		/** Compare at this looter level; null returns to the app-wide basis. */
		set looterLevel(value: number | null) {
			looterLevel =
				value === null || !Number.isFinite(value)
					? null
					: Math.min(LOOTER_CEILING, Math.max(0, value));
		},
		set referenceId(value: number | null) {
			referenceId = value;
		},
		get hpMax() {
			return hpMax;
		},
		set hpMax(value: number) {
			if (!Number.isFinite(value)) return;
			hpMax = Math.min(MAX_CURVE_HP, Math.max(MIN_CURVE_HP, Math.round(value)));
			pinnedHp = Math.min(pinnedHp, hpMax);
		},
		get pinnedHp() {
			return pinnedHp;
		},
		set pinnedHp(value: number) {
			if (Number.isFinite(value)) pinnedHp = Math.min(hpMax, Math.max(1, Math.round(value)));
		},
		/** The current request's identity, or null while no setup is complete. */
		get requestKey(): string | null {
			return request()?.key ?? null;
		},
		/** Whether the shown comparison answers the current setups. */
		get current(): boolean {
			const key = request()?.key ?? null;
			return key !== null && key === computedKey;
		},
		/** Compare the current setups, unless that answer is already shown. A
		 * slower answer to an earlier request never replaces a later one. */
		async compare() {
			const next = request();
			if (!next || next.key === computedKey) return;
			const mine = ++sequence;
			loading = true;
			try {
				const result = await compareLoadouts(next.setups, next.hpMax, next.looterLevel);
				if (mine !== sequence) return;
				comparison = result;
				computedIds = next.ids;
				computedKey = next.key;
				error = null;
			} catch (caught) {
				if (mine !== sequence) return;
				error = describeError(caught, 'Could not compare these setups');
			} finally {
				if (mine === sequence) loading = false;
			}
		},
	};
}

export type Comparator = ReturnType<typeof createComparator>;

let session: Comparator | null = null;

/** The comparator for this app session: the list survives leaving the tab. */
export function sessionComparator(): Comparator {
	session ??= createComparator();
	return session;
}
