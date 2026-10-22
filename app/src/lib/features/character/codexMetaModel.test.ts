import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CodexMetaProgress } from '$lib/api/commands.gen';
import { createCodexMetaModel, parseEarned } from './codexMetaModel.svelte';

vi.mock('$lib/api', () => ({
	getCodexMetaAttributes: vi.fn(),
	getCodexMetaProgress: vi.fn(),
	calibrateCodexMeta: vi.fn(),
	claimCodexMeta: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function progress(earned: number, tracked = 40): CodexMetaProgress {
	return { earned, tracked, cap: 750 };
}

const ATTRIBUTES = [
	{ name: 'Agility', currentLevel: 32.1 },
	{ name: 'Health', currentLevel: null },
];

beforeEach(() => {
	vi.clearAllMocks();
	mocked.getCodexMetaAttributes.mockResolvedValue(ATTRIBUTES);
	mocked.getCodexMetaProgress.mockResolvedValue(progress(40));
});

async function loaded() {
	const model = createCodexMetaModel();
	await model.load();
	return model;
}

describe('parseEarned', () => {
	it('accepts a whole number within the cap', () => {
		expect(parseEarned('0', 750)).toBe(0);
		expect(parseEarned(' 42 ', 750)).toBe(42);
		expect(parseEarned(750, 750)).toBe(750);
	});

	it('refuses an empty, fractional, negative, or over-cap entry', () => {
		for (const value of ['', null, '4.5', '-1', '751', 'abc']) {
			expect(parseEarned(value, 750)).toBeNull();
		}
	});
});

describe('createCodexMetaModel', () => {
	it('loads the attributes and the metas earned together', async () => {
		const model = createCodexMetaModel();
		expect(model.loading).toBe(true);
		await model.load();
		expect(model.loading).toBe(false);
		expect(model.attributes).toEqual(ATTRIBUTES);
		expect(model.progress).toEqual(progress(40));
		expect(model.adjustment).toBe(0);
		expect(model.draft).toBe(40);
	});

	it('reports a failed load', async () => {
		mocked.getCodexMetaProgress.mockRejectedValue(new Error('database is locked'));
		const model = await loaded();
		expect(model.loadError).toBe('database is locked');
	});

	it('calibrates the typed count and reads back the adjustment', async () => {
		mocked.calibrateCodexMeta.mockResolvedValue(progress(43));
		const model = await loaded();
		model.toggleCalibrating();
		expect(model.calibrating).toBe(true);

		model.draft = '43';
		await model.commitDraft();
		expect(mocked.calibrateCodexMeta).toHaveBeenCalledWith(43);
		expect(model.progress?.earned).toBe(43);
		expect(model.adjustment).toBe(3);
		expect(model.draft).toBe(43);
	});

	it('steps by one within the cap, and skips an unchanged count', async () => {
		mocked.calibrateCodexMeta.mockResolvedValue(progress(39));
		const model = await loaded();

		model.step(-1);
		await vi.waitFor(() => expect(model.saving).toBe(false));
		expect(mocked.calibrateCodexMeta).toHaveBeenCalledWith(39);

		mocked.calibrateCodexMeta.mockClear();
		await model.calibrate(39);
		expect(mocked.calibrateCodexMeta).not.toHaveBeenCalled();
	});

	it('reverts an unusable entry without writing', async () => {
		const model = await loaded();
		model.draft = '9000';
		await model.commitDraft();
		expect(mocked.calibrateCodexMeta).not.toHaveBeenCalled();
		expect(model.draft).toBe(40);
		expect(model.notice).toEqual({ tone: 'error', text: 'Enter a whole number from 0 to 750.' });
	});

	it('keeps the saved count when the write fails', async () => {
		mocked.calibrateCodexMeta.mockRejectedValue(new Error('Metas earned must be 0-750'));
		const model = await loaded();
		model.draft = 12;
		await model.commitDraft();
		expect(model.progress?.earned).toBe(40);
		expect(model.draft).toBe(40);
		expect(model.notice).toEqual({ tone: 'error', text: 'Metas earned must be 0-750' });
	});

	it('claims a meta reward and refreshes the attribute levels', async () => {
		mocked.claimCodexMeta.mockResolvedValue({ attributeName: 'Agility', pedValue: 1 });
		const model = await loaded();
		await model.claim('Agility');
		expect(mocked.claimCodexMeta).toHaveBeenCalledWith('Agility');
		expect(mocked.getCodexMetaAttributes).toHaveBeenCalledTimes(2);
		expect(model.notice).toEqual({ tone: 'success', text: 'Claimed! Agility +1.00 PES' });
	});
});
