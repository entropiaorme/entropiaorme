import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MarketCommitResult, MarketPastePreview } from '$lib/api';
import { createImportModel, PREVIEW_DEBOUNCE_MS } from './importModel.svelte';

vi.mock('$lib/api', () => ({
	previewMarketPaste: vi.fn(),
	commitMarketPaste: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

const reading = { markupPct: 106.88, salesPed: 451.9 };

function preview(overrides: Partial<MarketPastePreview> = {}): MarketPastePreview {
	return {
		rows: [
			{
				itemName: 'Carabok Hide',
				tier: 0,
				day: reading,
				week: reading,
				month: reading,
				year: reading,
				decade: reading,
			},
		],
		skipped: [],
		...overrides,
	};
}

function commitResult(overrides: Partial<MarketCommitResult> = {}): MarketCommitResult {
	return {
		submissionId: 1,
		itemCount: 1,
		skippedCount: 0,
		observedAt: 1_752_000_000,
		...overrides,
	};
}

// Fake timers throughout: every edit arms the debounced parse, and a
// real timer left pending would fire into a later test.
beforeEach(() => {
	vi.clearAllMocks();
	vi.useFakeTimers();
});

afterEach(() => {
	vi.clearAllTimers();
	vi.useRealTimers();
});

describe('createImportModel', () => {
	it('previews the pasted text and enables the commit', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		const model = createImportModel();
		expect(model.canCommit).toBe(false);

		model.setText('Carabok Hide\t0\t...');
		expect(model.canCommit).toBe(false);

		await model.runPreview();
		expect(model.preview?.rows).toHaveLength(1);
		expect(model.canCommit).toBe(true);
		expect(model.error).toBeNull();
	});

	it('editing the text drops the standing preview', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		const model = createImportModel();
		model.setText('first');
		await model.runPreview();
		expect(model.preview).not.toBeNull();

		model.setText('first edited');
		expect(model.preview).toBeNull();
		expect(model.canCommit).toBe(false);
	});

	it('a fully-skipped paste surfaces the skip explanation and blocks commit', async () => {
		mocked.previewMarketPaste.mockResolvedValue(
			preview({
				rows: [],
				skipped: [{ lineNumber: 1, content: 'garbage', reason: 'expected 12 columns, found 1' }],
			}),
		);
		const model = createImportModel();
		model.setText('garbage');
		await model.runPreview();
		expect(model.canCommit).toBe(false);
		expect(model.error).toContain('every line was skipped');
	});

	it('commit stores the raw text, clears the box, and reports success', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		mocked.commitMarketPaste.mockResolvedValue(commitResult({ itemCount: 7 }));
		const model = createImportModel();
		model.setText('the paste');
		await model.runPreview();

		const landed = await model.commit();
		expect(landed).toBe(true);
		expect(mocked.commitMarketPaste).toHaveBeenCalledWith('the paste');
		expect(model.text).toBe('');
		expect(model.preview).toBeNull();
		expect(model.committed?.itemCount).toBe(7);
	});

	it('a failed commit keeps the text for retry and surfaces the error', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		mocked.commitMarketPaste.mockRejectedValue(new Error('boom'));
		const model = createImportModel();
		model.setText('the paste');
		await model.runPreview();

		const landed = await model.commit();
		expect(landed).toBe(false);
		expect(model.text).toBe('the paste');
		expect(model.error).not.toBeNull();
	});

	it('a stale preview response never overwrites a newer edit', async () => {
		let resolveFirst: (value: MarketPastePreview) => void = () => {};
		mocked.previewMarketPaste.mockImplementationOnce(
			() => new Promise((resolve) => (resolveFirst = resolve)),
		);
		const model = createImportModel();
		model.setText('first');
		const first = model.runPreview();

		model.setText('second');
		resolveFirst(preview());
		await first;
		expect(model.preview).toBeNull();
		// The stale response must not leave the model wedged: the flag
		// clears and a new preview is immediately possible.
		expect(model.previewing).toBe(false);
	});

	it('parses the text once it settles, without a separate preview action', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		const model = createImportModel();

		model.setText('Carabok');
		model.setText('Carabok Hide\t0\t...');
		await vi.advanceTimersByTimeAsync(PREVIEW_DEBOUNCE_MS - 1);
		expect(mocked.previewMarketPaste).not.toHaveBeenCalled();

		await vi.advanceTimersByTimeAsync(1);
		expect(mocked.previewMarketPaste).toHaveBeenCalledTimes(1);
		expect(mocked.previewMarketPaste).toHaveBeenCalledWith('Carabok Hide\t0\t...');
		expect(model.canCommit).toBe(true);
	});

	it('clearing the box cancels a pending parse', async () => {
		mocked.previewMarketPaste.mockResolvedValue(preview());
		const model = createImportModel();

		model.setText('the paste');
		model.setText('   ');
		await vi.advanceTimersByTimeAsync(PREVIEW_DEBOUNCE_MS * 2);
		expect(mocked.previewMarketPaste).not.toHaveBeenCalled();
		expect(model.preview).toBeNull();
	});
});
