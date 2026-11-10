import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ProtectionRecordingCandidates, TerminalReadResult } from '$lib/api';

const api = vi.hoisted(() => ({
	confirmProtectionObservation: vi.fn(),
	confirmProtectionRepair: vi.fn(),
	getProtectionOverview: vi.fn(),
	getProtectionRecordingCandidates: vi.fn(),
	scanRepairCost: vi.fn<(sessionId: string) => Promise<TerminalReadResult>>(),
	scanTradeTerminalValue: vi.fn<() => Promise<TerminalReadResult>>(),
}));

vi.mock('$lib/api', () => api);

import { createArmourRecording } from './armourRecordingModel.svelte';

const NO_CANDIDATES: ProtectionRecordingCandidates = {
	stream: { kind: 'unlimited' },
	since: null,
	baselineTtPed: null,
	sessions: [],
	earlier: [],
};

function read(text: string): TerminalReadResult {
	return { kind: 'read', valuePed: Number(text), text, rawText: `${text} PED` };
}

/** A promise the test settles when it chooses, for a read still out. */
function pending<T>() {
	let settle!: (value: T) => void;
	const promise = new Promise<T>((resolve) => {
		settle = resolve;
	});
	return { promise, settle };
}

beforeEach(() => {
	vi.clearAllMocks();
	api.getProtectionRecordingCandidates.mockResolvedValue(NO_CANDIDATES);
});

describe('a terminal read still out', () => {
	it('is dropped when a second read is started before it answers', async () => {
		const recording = createArmourRecording({
			repairOcrEnabled: () => true,
			limitedEnabled: () => false,
		});
		await recording.load();
		const first = pending<TerminalReadResult>();
		const second = pending<TerminalReadResult>();
		api.scanRepairCost.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

		const firstScan = recording.scan();
		const secondScan = recording.scan();
		second.settle(read('3.10'));
		await secondScan;
		first.settle(read('9.99'));
		await firstScan;

		expect(recording.value).toBe('3.10');
		expect(recording.phase).toBe('review');
	});

	it('is dropped when the player has moved to another stream', async () => {
		const recording = createArmourRecording({
			repairOcrEnabled: () => true,
			limitedEnabled: () => false,
		});
		await recording.load();
		const out = pending<TerminalReadResult>();
		api.scanRepairCost.mockReturnValueOnce(out.promise);

		const scan = recording.scan();
		await recording.chooseStream({ kind: 'limited', setId: 9 });
		out.settle(read('2.20'));
		await scan;

		expect(recording.value).toBe('');
		expect(recording.phase).toBe('ready');
	});
});
