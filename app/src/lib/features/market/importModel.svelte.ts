/**
 * The Market tab's paste box: the text parses as it arrives (a short
 * debounce absorbs typing), the parse summary shows what an import would
 * store, and one action commits it. Also the PED-per-unit quote form.
 *
 * The commit sends the raw text again rather than echoing the preview:
 * the backend re-parses, so what gets stored always comes from the one
 * parser. The preview here is purely the user's review surface.
 */

import type { MarketCommitResult, MarketPastePreview, MarketUnitPriceResult } from '$lib/api';
import { commitMarketPaste, previewMarketPaste, setMarketUnitPrice } from '$lib/api';
import { describeError } from '$lib/view/errorState';

/** Quiet period after the last edit before the text is parsed. */
export const PREVIEW_DEBOUNCE_MS = 250;

export function createImportModel() {
	let text = $state('');
	let preview = $state<MarketPastePreview | null>(null);
	let previewing = $state(false);
	let committing = $state(false);
	let error = $state<string | null>(null);
	let committed = $state<MarketCommitResult | null>(null);
	let unitItemName = $state('Hyperion Daily Voucher');
	let unitPriceInput = $state('');
	let unitPriceSaving = $state(false);
	let unitPriceSaved = $state<MarketUnitPriceResult | null>(null);
	let unitPriceError = $state<string | null>(null);

	// Monotonic token so a slow preview for superseded text cannot
	// overwrite the newer preview (or its error) after an edit.
	let previewEpoch = 0;
	let previewTimer: ReturnType<typeof setTimeout> | null = null;

	const canCommit = $derived(
		preview !== null && preview.rows.length > 0 && !previewing && !committing,
	);
	const parsedUnitPrice = $derived(Number(unitPriceInput));
	const canSaveUnitPrice = $derived(
		unitItemName.trim().length > 0 &&
			unitPriceInput.trim().length > 0 &&
			Number.isFinite(parsedUnitPrice) &&
			parsedUnitPrice >= 0 &&
			!unitPriceSaving,
	);

	function setText(value: string) {
		text = value;
		// Edited text invalidates the standing preview: the review must
		// always describe exactly what a commit would store.
		preview = null;
		committed = null;
		error = null;
		// Invalidate any in-flight preview AND clear its flag here: the
		// stale request's finally block deliberately no-ops once the
		// epoch moved, so this is the only place left to reset it.
		previewEpoch++;
		previewing = false;
		if (previewTimer !== null) clearTimeout(previewTimer);
		previewTimer = null;
		if (value.trim()) {
			previewTimer = setTimeout(() => {
				previewTimer = null;
				void runPreview();
			}, PREVIEW_DEBOUNCE_MS);
		}
	}

	async function runPreview() {
		if (!text.trim()) return;
		const epoch = ++previewEpoch;
		previewing = true;
		error = null;
		committed = null;
		try {
			const parsed = await previewMarketPaste(text);
			if (epoch !== previewEpoch) return;
			preview = parsed;
			if (parsed.rows.length === 0) {
				error =
					parsed.skipped.length > 0
						? 'No readable market rows; every line was skipped (see reasons below).'
						: 'Nothing to import; the paste is empty.';
			}
		} catch (e) {
			if (epoch !== previewEpoch) return;
			error = describeError(e, 'Failed to read the paste');
		} finally {
			if (epoch === previewEpoch) previewing = false;
		}
	}

	/** Commits the current text; returns true when observations landed
	 * (the page uses it to refresh the overview). */
	async function commit(): Promise<boolean> {
		if (preview === null || preview.rows.length === 0) return false;
		committing = true;
		error = null;
		try {
			committed = await commitMarketPaste(text);
			text = '';
			preview = null;
			return true;
		} catch (e) {
			error = describeError(e, 'Failed to store the observations');
			return false;
		} finally {
			committing = false;
		}
	}

	async function saveUnitPrice(): Promise<boolean> {
		if (!canSaveUnitPrice) return false;
		unitPriceSaving = true;
		unitPriceError = null;
		unitPriceSaved = null;
		try {
			unitPriceSaved = await setMarketUnitPrice(unitItemName.trim(), parsedUnitPrice);
			return true;
		} catch (e) {
			unitPriceError = describeError(e, 'Failed to store the unit price');
			return false;
		} finally {
			unitPriceSaving = false;
		}
	}

	return {
		get text() {
			return text;
		},
		setText,
		get preview() {
			return preview;
		},
		get previewing() {
			return previewing;
		},
		get committing() {
			return committing;
		},
		get error() {
			return error;
		},
		get committed() {
			return committed;
		},
		get canCommit() {
			return canCommit;
		},
		get unitItemName() {
			return unitItemName;
		},
		setUnitItemName(value: string) {
			unitItemName = value;
			unitPriceSaved = null;
			unitPriceError = null;
		},
		get unitPriceInput() {
			return unitPriceInput;
		},
		setUnitPriceInput(value: string) {
			unitPriceInput = value;
			unitPriceSaved = null;
			unitPriceError = null;
		},
		get unitPriceSaving() {
			return unitPriceSaving;
		},
		get unitPriceSaved() {
			return unitPriceSaved;
		},
		get unitPriceError() {
			return unitPriceError;
		},
		get canSaveUnitPrice() {
			return canSaveUnitPrice;
		},
		saveUnitPrice,
		runPreview,
		commit,
	};
}

export type ImportModel = ReturnType<typeof createImportModel>;
