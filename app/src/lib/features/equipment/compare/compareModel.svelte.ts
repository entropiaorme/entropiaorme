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
import type { SearchKind } from '$lib/api/commands.gen';
import { describeError } from '$lib/view/errorState';
import { createTypeahead, type Typeahead } from '$lib/view/typeahead.svelte';

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
	let sequence = 0;
	let nextId = 1;

	function request(): { setups: SetupPick[]; ids: number[]; key: string } | null {
		const complete = drafts.flatMap((draft) => {
			const pick = draft.pick();
			return pick ? [{ id: draft.id, pick }] : [];
		});
		if (complete.length === 0) return null;
		const setups = complete.map((entry) => entry.pick);
		const ids = complete.map((entry) => entry.id);
		return { setups, ids, key: JSON.stringify([ids, setups]) };
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
				const result = await compareLoadouts(next.setups);
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
