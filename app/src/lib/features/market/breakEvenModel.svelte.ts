/**
 * Break-even-tab view model over configured loadouts. Component weighting,
 * limited-item premium drag, and every looter basis's figures are computed
 * backend-side; this model reads each row at the app-wide looter basis and
 * orders the readout.
 */

import type { MarketBreakEven, MarketWeaponBreakEven } from '$lib/api';
import { getMarketBreakEven } from '$lib/api';
import { atLooterBasis, looterBasis, looterLevelsFrom } from '$lib/looterBasis.svelte';
import { describeError } from '$lib/view/errorState';

/** A loadout's row with its looter-dependent figures at the chosen basis;
 * null where the loadout has no modelled offensive stream. */
export type BreakEvenRow = Omit<MarketWeaponBreakEven, 'byLooter'> & {
	expectedTtReturnPct: number | null;
	breakEvenLootMarkupPct: number | null;
	looterLevel: number | null;
};

export function createBreakEvenModel() {
	let data = $state<MarketBreakEven | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let loadEpoch = 0;

	async function loadData() {
		const epoch = ++loadEpoch;
		loading = true;
		error = null;
		try {
			const loaded = await getMarketBreakEven();
			if (epoch !== loadEpoch) return;
			data = loaded;
		} catch (e) {
			if (epoch !== loadEpoch) return;
			error = describeError(e, 'Failed to load the break-even readout');
		} finally {
			if (epoch === loadEpoch) loading = false;
		}
	}

	const looters = $derived(data?.looters ?? []);
	const looterLevels = $derived(looterLevelsFrom(looters));

	// Complete loadouts first, closest break-even requirement first. Missing
	// Efficiency remains visible at the end rather than acquiring a zero.
	const weapons = $derived.by(() => {
		const rows: BreakEvenRow[] = (data?.weapons ?? []).map(({ byLooter, ...row }) => {
			const atBasis = byLooter ? atLooterBasis(byLooter, looterBasis.current) : null;
			return {
				...row,
				expectedTtReturnPct: atBasis?.expectedTtReturnPct ?? null,
				breakEvenLootMarkupPct: atBasis?.breakEvenLootMarkupPct ?? null,
				looterLevel: atBasis?.looterLevel ?? null,
			};
		});
		return rows.sort(
			(a, b) => (a.breakEvenLootMarkupPct ?? Infinity) - (b.breakEvenLootMarkupPct ?? Infinity),
		);
	});

	return {
		get looters() {
			return looters;
		},
		/** The three hunting looters' levels the basis picker lists. */
		get looterLevels() {
			return looterLevels;
		},
		get weapons() {
			return weapons;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		loadData,
	};
}

export type BreakEvenModel = ReturnType<typeof createBreakEvenModel>;
