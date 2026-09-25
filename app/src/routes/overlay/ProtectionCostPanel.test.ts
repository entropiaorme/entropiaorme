// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	ProtectionCandidateSession,
	ProtectionCostWindow,
	ProtectionRecordingCandidates,
	ProtectionSet,
} from '$lib/api';

const api = vi.hoisted(() => ({
	confirmProtectionObservation: vi.fn(),
	confirmProtectionRepair: vi.fn(),
	getProtectionOverview: vi.fn(),
	getProtectionRecordingCandidates: vi.fn(),
	scanRepairCost: vi.fn(),
	scanTradeTerminalValue: vi.fn(),
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

function renderPanel(onClose = vi.fn()) {
	render(ProtectionCostPanel, { props: { repairOcrEnabled: false, onClose } });
	return onClose;
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
