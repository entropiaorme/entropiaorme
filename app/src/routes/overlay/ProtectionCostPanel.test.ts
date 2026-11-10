// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	ProtectionCandidateSession,
	ProtectionCostWindow,
	ProtectionRecordingCandidates,
	ProtectionSet,
	TerminalReadResult,
} from '$lib/api';

const api = vi.hoisted(() => ({
	confirmProtectionObservation: vi.fn(),
	confirmProtectionRepair: vi.fn(),
	getProtectionOverview: vi.fn(),
	getProtectionRecordingCandidates: vi.fn(),
	scanRepairCost: vi.fn<(sessionId: string) => Promise<TerminalReadResult>>(),
	scanTradeTerminalValue: vi.fn<() => Promise<TerminalReadResult>>(),
}));

vi.mock('$lib/api', () => api);

import ProtectionCostPanel from './ProtectionCostPanel.svelte';

function session(
	id: string,
	startedAt: number,
	hitCount: number,
	definition: [string, string] | null = null,
	covered = false,
	unrecorded = !covered,
): ProtectionCandidateSession {
	return {
		sessionId: id,
		sessionName: definition?.[1] ?? null,
		definitionId: definition?.[0] ?? null,
		definitionName: definition?.[1] ?? null,
		startedAt,
		endedAt: startedAt + 3600,
		hitCount,
		covered,
		unrecorded,
	};
}

const ARIS: [string, string] = ['2', 'ARIS Dailies'];
const TREES: [string, string] = ['3', 'Tree Cutting'];

function candidates(
	sessions: ProtectionCandidateSession[],
	overrides: Partial<ProtectionRecordingCandidates> = {},
): ProtectionRecordingCandidates {
	return {
		stream: { kind: 'unlimited' },
		since: null,
		baselineTtPed: null,
		sessions,
		earlier: [],
		...overrides,
	};
}

function costWindow(costPed: number, sessionIds: string[]): ProtectionCostWindow {
	return {
		id: 'w',
		kind: 'repair',
		setId: null,
		setName: null,
		consumedTtPed: null,
		markupPercent: null,
		costPed,
		costKnown: true,
		status: sessionIds.length > 0 ? 'booked' : 'pending',
		reason: null,
		createdAt: 1,
		supersededAt: null,
		undoable: true,
		allocations: sessionIds.map((sessionId) => ({
			sessionId,
			sessionName: null,
			definitionName: null,
			startedAt: 1,
			hitCount: 1,
			allocationShare: 1 / sessionIds.length,
			costPed: costPed / sessionIds.length,
			contexts: [],
		})),
	};
}

const hyperion: ProtectionSet = {
	id: '9',
	kind: 'armour',
	name: 'Hyperion',
	markupPercent: 200,
	latestObservation: null,
	basisLocked: false,
	backlog: { lastRecordedAt: null, sessions: 0, hits: 0 },
};

const EMPTY_BACKLOG = { lastRecordedAt: null, sessions: 0, hits: 0 };

function renderPanel(onClose = vi.fn(), repairOcrEnabled = false) {
	render(ProtectionCostPanel, { props: { repairOcrEnabled, onClose } });
	return onClose;
}

function input(): HTMLInputElement {
	return screen.getByPlaceholderText('0.00 PED') as HTMLInputElement;
}

async function enterAmount(value: string) {
	await fireEvent.click(await screen.findByText('Enter manually'));
	await fireEvent.input(screen.getByPlaceholderText('0.00 PED'), { target: { value } });
}

function lastRepairInput() {
	return api.confirmProtectionRepair.mock.calls.at(-1)?.[0];
}

beforeEach(() => {
	vi.clearAllMocks();
	api.getProtectionOverview.mockResolvedValue({
		sets: [],
		unlimited: EMPTY_BACKLOG,
		recentCostWindows: [],
		unrecorded: { sessions: 0, hits: 0 },
	});
	api.confirmProtectionRepair.mockImplementation(
		async (input: { costPed: number; sessionIds: string[] }) => ({
			costWindow: costWindow(input.costPed, input.sessionIds),
		}),
	);
});

describe('recording an unlimited repair', () => {
	it('spreads over the one session since the last repair without asking which', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('s1', 1_700_000_000, 80, ARIS)]),
		);
		renderPanel();

		await enterAmount('8');
		expect(await screen.findByText(/Covers ARIS Dailies/)).toBeTruthy();
		expect(screen.queryByTestId('armour-session-picker')).toBeNull();
		await fireEvent.click(screen.getByText('Record'));

		await waitFor(() =>
			expect(lastRepairInput()).toMatchObject({ costPed: 8, sessionIds: ['s1'] }),
		);
		expect(await screen.findByText('8.00 PED recorded')).toBeTruthy();
	});

	it('drops a whole session type in one tick', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([
				session('a1', 1_700_000_000, 30, ARIS),
				session('t1', 1_700_100_000, 50, TREES),
				session('a2', 1_700_200_000, 10, ARIS),
			]),
		);
		renderPanel();
		await enterAmount('4');

		expect(await screen.findByTestId('armour-session-picker')).toBeTruthy();
		await fireEvent.click(screen.getByLabelText('Include Tree Cutting'));
		expect(screen.getByText('2 sessions · 40 hits')).toBeTruthy();
		// Hit-weighted preview: 30 and 10 of 40 hits.
		expect(screen.getByText('4.00')).toBeTruthy();

		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()?.sessionIds).toEqual(['a1', 'a2']));
	});

	it('drops a single session through its open session type', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('a1', 1_700_000_000, 30, ARIS), session('a2', 1_700_200_000, 10, ARIS)]),
		);
		renderPanel();
		await enterAmount('4');

		await fireEvent.click(await screen.findByText('ARIS Dailies'));
		const singles = screen.getAllByLabelText(/Include the session of/);
		expect(singles).toHaveLength(2);
		await fireEvent.click(singles[1]);
		expect((screen.getByLabelText('Include ARIS Dailies') as HTMLInputElement).indeterminate).toBe(
			true,
		);

		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()?.sessionIds).toEqual(['a1']));
	});

	it('moves the look-back start later from the since line', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates(
				[
					session('july', 1_689_000_000, 20, ARIS),
					session('sept-1', 1_694_000_000, 15, ARIS),
					session('sept-2', 1_694_100_000, 5, ARIS),
				],
				{ since: 1_688_000_000 },
			),
		);
		renderPanel();
		await enterAmount('2');

		const since = (await screen.findByLabelText('Cover sessions from')) as HTMLSelectElement;
		await fireEvent.change(since, { target: { value: 'sept-1' } });
		expect(screen.getByText('2 sessions · 20 hits')).toBeTruthy();

		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()?.sessionIds).toEqual(['sept-1', 'sept-2']));
	});

	it('re-includes an earlier session only when asked to', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('new', 1_700_200_000, 30, ARIS)], {
				since: 1_700_100_000,
				earlier: [session('old', 1_700_000_000, 10, ARIS, true)],
			}),
		);
		renderPanel();
		await enterAmount('4');

		await fireEvent.click(await screen.findByText('Include sessions from before the last repair'));
		const earlier = screen.getByLabelText(/Include the earlier session of/) as HTMLInputElement;
		expect(earlier.checked).toBe(false);
		expect(screen.getByText('recorded')).toBeTruthy();
		await fireEvent.click(earlier);

		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()?.sessionIds).toEqual(['old', 'new']));
	});

	it('draws attention to earlier sessions no recording has reached', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('new', 1_700_200_000, 30, ARIS)], {
				since: 1_700_100_000,
				earlier: [
					session('forgotten', 1_699_000_000, 12, ['4', 'Caly AI Dailies'], false, true),
					session('repaired', 1_700_000_000, 10, ARIS, true, false),
				],
			}),
		);
		renderPanel();
		await enterAmount('4');

		const toggle = await screen.findByText(/1 has no armour cost/);
		await fireEvent.click(toggle);
		expect(screen.getByText('not recorded')).toBeTruthy();
		expect(screen.getByText('recorded')).toBeTruthy();
	});

	it('says a repair with no hits behind it counts toward overall costs only', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([], { since: 1_700_000_000 }),
		);
		renderPanel();
		await enterAmount('1');
		expect(await screen.findByText(/No hits recorded since the last repair/)).toBeTruthy();
		await fireEvent.click(screen.getByText('Record'));
		expect(await screen.findByText(/counts toward your overall costs only/)).toBeTruthy();
	});

	it('retries a failed recording under the same token', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('s1', 1_700_000_000, 5)]),
		);
		api.confirmProtectionRepair.mockRejectedValueOnce(new Error('connection lost'));
		renderPanel();
		await enterAmount('1');

		await fireEvent.click(screen.getByText('Record'));
		expect(await screen.findByText('connection lost')).toBeTruthy();
		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(api.confirmProtectionRepair).toHaveBeenCalledTimes(2));
		const [first, second] = api.confirmProtectionRepair.mock.calls.map(
			(call) => call[0].clientToken,
		);
		expect(second).toBe(first);
	});
});

describe('recording a limited set', () => {
	beforeEach(() => {
		api.getProtectionOverview.mockResolvedValue({
			sets: [hyperion],
			unlimited: EMPTY_BACKLOG,
			recentCostWindows: [],
			unrecorded: { sessions: 0, hits: 0 },
		});
	});

	it('takes a first reading as a baseline that spreads nothing', async () => {
		api.getProtectionRecordingCandidates.mockImplementation(async (stream: { kind: string }) =>
			stream.kind === 'limited'
				? candidates([], { stream: { kind: 'limited', setId: 9 } })
				: candidates([session('s1', 1_700_000_000, 5)]),
		);
		api.confirmProtectionObservation.mockResolvedValue({
			observation: {
				id: '1',
				setId: '9',
				ttValuePed: 30,
				source: 'manual',
				rawText: null,
				observedAt: 1,
				resetReason: null,
			},
			costWindow: null,
		});
		renderPanel();

		await fireEvent.click(await screen.findByText('Hyperion'));
		await enterAmount('30');
		expect(screen.getByText(/first reading/)).toBeTruthy();
		await fireEvent.click(screen.getByText('Set baseline'));

		await waitFor(() =>
			expect(api.confirmProtectionObservation).toHaveBeenCalledWith(
				expect.objectContaining({ setId: 9, ttValuePed: 30, sessionIds: [] }),
			),
		);
		expect(await screen.findByText('Baseline set at 30.00 PED')).toBeTruthy();
	});

	it('previews the markup-priced loss and needs a reason for a higher reading', async () => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('s1', 1_700_000_000, 5)], {
				stream: { kind: 'limited', setId: 9 },
				baselineTtPed: 30,
				since: 1_699_000_000,
			}),
		);
		renderPanel();
		await fireEvent.click(await screen.findByText('Hyperion'));

		await enterAmount('28');
		expect(await screen.findByText('4.00 PED cost')).toBeTruthy();

		await fireEvent.input(screen.getByPlaceholderText('0.00 PED'), { target: { value: '31' } });
		const reset = screen.getByText('Reset baseline') as HTMLButtonElement;
		expect(reset.disabled).toBe(true);
		await fireEvent.input(screen.getByPlaceholderText('Pieces replaced or reading corrected'), {
			target: { value: 'Replaced the helmet' },
		});
		expect((screen.getByText('Reset baseline') as HTMLButtonElement).disabled).toBe(false);
	});
});

/** A read of `text` as the terminal prints it, `2.20` or `1,234.56789`. */
function read(text: string, rawText = `${text} PED`): TerminalReadResult {
	return { kind: 'read', valuePed: Number(text.replaceAll(',', '')), text, rawText };
}

function refused(
	reason: Extract<TerminalReadResult, { kind: 'refused' }>['reason'],
): TerminalReadResult {
	return { kind: 'refused', reason };
}

/** A promise the test settles when it chooses, for a read still out. */
function pending<T>() {
	let settle!: (value: T) => void;
	const promise = new Promise<T>((resolve) => {
		settle = resolve;
	});
	return { promise, settle };
}

function withHyperion() {
	api.getProtectionOverview.mockResolvedValue({
		sets: [hyperion],
		unlimited: EMPTY_BACKLOG,
		recentCostWindows: [],
		unrecorded: { sessions: 0, hits: 0 },
	});
	api.getProtectionRecordingCandidates.mockImplementation(async (stream: { kind: string }) =>
		stream.kind === 'limited'
			? candidates([], { stream: { kind: 'limited', setId: 9 } })
			: candidates([session('s1', 1_700_000_000, 80, ARIS)]),
	);
}

describe('reading the figure off a terminal', () => {
	beforeEach(() => {
		api.getProtectionRecordingCandidates.mockResolvedValue(
			candidates([session('s1', 1_700_000_000, 80, ARIS)]),
		);
	});

	it('puts the repair cost in the field as printed and records it', async () => {
		api.scanRepairCost.mockResolvedValue(read('2.20'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		await waitFor(() => expect(input().value).toBe('2.20'));
		// What the recogniser saw is evidence, not something to show.
		expect(screen.queryByText(/OCR/)).toBeNull();
		expect(screen.queryByText(/2\.20 PED/)).toBeNull();
		await fireEvent.click(screen.getByText('Record'));

		await waitFor(() =>
			expect(lastRepairInput()).toMatchObject({ costPed: 2.2, sessionIds: ['s1'] }),
		);
	});

	it('keeps every printed digit of a grouped figure', async () => {
		api.scanRepairCost.mockResolvedValue(read('1,234.56789'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		await waitFor(() => expect(input().value).toBe('1234.56789'));
		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()).toMatchObject({ costPed: 1234.56789 }));
	});

	it('says why there is no figure, leaves the field empty, and takes a typed one', async () => {
		api.scanRepairCost.mockResolvedValue(refused('figureNotVisible'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		expect(
			await screen.findByText('The repair cost is covered by another window or off the screen'),
		).toBeTruthy();
		expect(input().value).toBe('');
		expect((screen.getByText('Record') as HTMLButtonElement).disabled).toBe(true);

		await fireEvent.input(input(), { target: { value: '3' } });
		await fireEvent.click(screen.getByText('Record'));
		await waitFor(() => expect(lastRepairInput()).toMatchObject({ costPed: 3 }));
	});

	it('drops an earlier read when a re-scan gives no figure', async () => {
		api.scanRepairCost
			.mockResolvedValueOnce(read('2.20'))
			.mockResolvedValueOnce(refused('terminalNotOpen'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		await waitFor(() => expect(input().value).toBe('2.20'));
		await fireEvent.click(screen.getByText('Re-scan'));

		expect(
			await screen.findByText('Open the Repair Terminal in the game, with its title in view'),
		).toBeTruthy();
		expect(input().value).toBe('');
		expect((screen.getByText('Record') as HTMLButtonElement).disabled).toBe(true);
	});

	it('drops an earlier read when a re-scan fails outright', async () => {
		api.scanRepairCost
			.mockResolvedValueOnce(read('2.20'))
			.mockRejectedValueOnce(new Error('ipc down'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		await waitFor(() => expect(input().value).toBe('2.20'));
		await fireEvent.click(screen.getByText('Re-scan'));

		expect(await screen.findByText('Repair Terminal scan failed')).toBeTruthy();
		expect(input().value).toBe('');
	});

	it('keeps a typed figure when a re-scan gives none', async () => {
		api.scanRepairCost.mockResolvedValue(refused('gameWindowNotFound'));
		renderPanel(vi.fn(), true);

		await enterAmount('5');
		await fireEvent.click(screen.getByText('Re-scan'));

		expect(await screen.findByText('The Entropia Universe window was not found')).toBeTruthy();
		expect(input().value).toBe('5');
	});

	it('keeps a figure typed over a read when a re-scan gives none', async () => {
		api.scanRepairCost
			.mockResolvedValueOnce(read('2.20'))
			.mockResolvedValueOnce(refused('figureNotRead'));
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Scan Repair Terminal'));
		await waitFor(() => expect(input().value).toBe('2.20'));
		await fireEvent.input(input(), { target: { value: '2.25' } });
		await fireEvent.click(screen.getByText('Re-scan'));

		expect(
			await screen.findByText('The repair cost did not read clearly; enter it by hand'),
		).toBeTruthy();
		expect(input().value).toBe('2.25');
	});

	it('drops a read that resolves after the player switched stream', async () => {
		withHyperion();
		const trade = pending<TerminalReadResult>();
		api.scanTradeTerminalValue.mockReturnValueOnce(trade.promise);
		renderPanel(vi.fn(), true);

		await fireEvent.click(await screen.findByText('Hyperion'));
		await fireEvent.click(await screen.findByText('Scan Trade Terminal'));
		await fireEvent.click(screen.getByText('Unlimited repair'));
		trade.settle(read('1,234.56'));

		// The Trade Terminal total never becomes the repair cost.
		expect(await screen.findByText('Scan Repair Terminal')).toBeTruthy();
		await fireEvent.click(screen.getByText('Enter manually'));
		expect(input().value).toBe('');
	});
});

describe('reading a limited set off the Trade Terminal', () => {
	beforeEach(withHyperion);

	it('records the read figure as read, with what the recogniser saw', async () => {
		api.scanTradeTerminalValue.mockResolvedValue(read('28.50', '28.50PED'));
		api.confirmProtectionObservation.mockResolvedValue({ observation: null, costWindow: null });
		renderPanel();

		await fireEvent.click(await screen.findByText('Hyperion'));
		await fireEvent.click(await screen.findByText('Scan Trade Terminal'));
		await waitFor(() => expect(input().value).toBe('28.50'));
		await fireEvent.click(screen.getByText('Set baseline'));

		await waitFor(() =>
			expect(api.confirmProtectionObservation).toHaveBeenCalledWith(
				expect.objectContaining({ ttValuePed: 28.5, source: 'ocr', rawText: '28.50PED' }),
			),
		);
	});

	it('records a figure typed over a read as typed, without the read evidence', async () => {
		api.scanTradeTerminalValue.mockResolvedValue(read('28.50', '28.50PED'));
		api.confirmProtectionObservation.mockResolvedValue({ observation: null, costWindow: null });
		renderPanel();

		await fireEvent.click(await screen.findByText('Hyperion'));
		await fireEvent.click(await screen.findByText('Scan Trade Terminal'));
		await waitFor(() => expect(input().value).toBe('28.50'));
		await fireEvent.input(input(), { target: { value: '28.40' } });
		await fireEvent.click(screen.getByText('Set baseline'));

		await waitFor(() =>
			expect(api.confirmProtectionObservation).toHaveBeenCalledWith(
				expect.objectContaining({ ttValuePed: 28.4, source: 'manual', rawText: null }),
			),
		);
	});

	it('says when the terminal is closed and reads it once open', async () => {
		api.scanTradeTerminalValue
			.mockResolvedValueOnce(refused('terminalNotOpen'))
			.mockResolvedValueOnce(read('1,234.56789'));
		renderPanel();

		await fireEvent.click(await screen.findByText('Hyperion'));
		await fireEvent.click(await screen.findByText('Scan Trade Terminal'));
		expect(
			await screen.findByText('Open the Trade Terminal in the game, with its title in view'),
		).toBeTruthy();
		expect(input().value).toBe('');

		await fireEvent.click(screen.getByText('Re-scan'));
		await waitFor(() => expect(input().value).toBe('1234.56789'));
		expect(api.scanRepairCost).not.toHaveBeenCalled();
	});

	it('drops a read when a re-scan of the Trade Terminal gives no figure', async () => {
		api.scanTradeTerminalValue
			.mockResolvedValueOnce(read('30.00'))
			.mockResolvedValueOnce(refused('figureNotVisible'));
		renderPanel();

		await fireEvent.click(await screen.findByText('Hyperion'));
		await fireEvent.click(await screen.findByText('Scan Trade Terminal'));
		await waitFor(() => expect(input().value).toBe('30.00'));
		await fireEvent.click(screen.getByText('Re-scan'));

		expect(
			await screen.findByText('The total is covered by another window or off the screen'),
		).toBeTruthy();
		expect(input().value).toBe('');
	});
});
