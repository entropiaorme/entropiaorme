// @vitest-environment happy-dom

import { fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	AccountSessionMeasure,
	MeasuredCostDetail,
	MeasuredCostLine,
} from '$lib/api/commands.gen';
import { NO_DATA } from '$lib/utils/format';
import { carried, exact, gain, measure, SPAN, totals, wholePec } from './__fixtures__/measure';
import { minutes } from './__fixtures__/minutes';
import { NOTHING_TRACKED, type TrackedFigures } from './measure';

const api = vi.hoisted(() => ({
	ACCOUNT_TOPIC: 'account:updated',
	getAccountStatus: vi.fn(),
	disconnectAccount: vi.fn(),
	getSessionMeasure: vi.fn(),
	getMeasuredSessions: vi.fn(),
	getMeasuredSummary: vi.fn(),
	// The detail reads the session minute by minute too: none here.
	getSessionMinutes: vi.fn(async () => minutes()),
}));
const frames = vi.hoisted(() => ({ handlers: [] as Array<() => void> }));

vi.mock('$lib/api/account', () => api);
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (topic: string, handler: () => void) => {
		if (topic === 'account:updated') frames.handlers.push(handler);
		return () => {};
	}),
}));

import SessionMeasure from './SessionMeasure.svelte';

/** The comparison row for a category, in the review. */
function row(label: string): HTMLElement {
	const found = screen.getByText(label, { selector: 'td span' }).closest('tr');
	if (!found) throw new Error(`no ${label} row`);
	return found;
}

const tracked: TrackedFigures = { weapon: 10, healing: 1, armour: 0.5, loot: 9 };

/** A complete measure whose totals match its lines, as the backend
 * sends it. */
function measured(overrides: Partial<AccountSessionMeasure> = {}): AccountSessionMeasure {
	return measure({
		ammo: [
			{
				name: 'Universal Ammo',
				netUnits: -95000,
				net: exact(-9.5),
				split: {
					kind: 'settled',
					burnedUnits: 95000,
					burned: exact(9.5),
					strongboxUnits: 0,
					strongbox: exact(0),
					convertedUnits: 0,
					converted: exact(0),
				},
			},
		],
		decay: [
			{ item: carried('Rifle'), role: 'weapon', decay: exact(0.75) },
			{ item: carried('Harness'), role: 'armour', decay: wholePec(0.4) },
		],
		gained: [
			gain('Animal Oil Residue', exact(7)),
			gain('Fireworks', exact(2), 'possiblyStrongbox'),
		],
		gainedTotal: exact(9),
		gainedPossiblyStrongbox: exact(2),
		repairs: [
			{
				between: SPAN,
				items: [{ item: carried('Harness'), amount: exact(1) }],
				restored: exact(1),
				cost: { kind: 'paid', amount: wholePec(1.02) },
			},
		],
		events: [
			{
				kind: 'sale',
				between: SPAN,
				items: [{ item: carried('Shrapnel'), units: 1240, amount: wholePec(12.4) }],
				amount: wholePec(12.4),
			},
		],
		totals: totals({
			ammo: { kind: 'settled', burned: exact(9.5) },
			weaponDecay: exact(0.75),
			armourDecay: wholePec(0.4),
			repairCost: wholePec(1.02),
			gained: exact(9),
			gainedPossiblyStrongbox: exact(2),
		}),
		...overrides,
	});
}

/** A promise the test settles by hand. */
function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

beforeEach(() => {
	vi.clearAllMocks();
	frames.handlers.length = 0;
});

describe("a session's measure in its detail", () => {
	it('renders nothing at all when the session has no measure', async () => {
		api.getSessionMeasure.mockResolvedValue(null);
		const { container } = render(SessionMeasure, {
			props: { sessionId: 's1', costBasis: 'measured' },
		});
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s1'));
		// Wait for the absent answer itself before asserting nothing shows.
		await api.getSessionMeasure.mock.results[0].value;
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});

	it('renders nothing when the first read fails', async () => {
		api.getSessionMeasure.mockRejectedValue(new Error('the account connection was not composed'));
		const { container } = render(SessionMeasure, {
			props: { sessionId: 's1', costBasis: 'measured' },
		});
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalled());
		await api.getSessionMeasure.mock.results[0].value.catch(() => undefined);
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});

	it('says where the measured costs came from, and sets no tracked figure beside them', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		expect(await screen.findByText('Costs from your account')).toBeTruthy();
		expect(screen.getByText('Complete: the readings cover the whole session.')).toBeTruthy();
		expect(screen.queryByRole('table')).toBeNull();
		expect(screen.queryByText('Tracked estimate')).toBeNull();
		expect(screen.queryByText('Difference')).toBeNull();
		expect(screen.getByRole('button', { name: 'Item by item' })).toBeTruthy();
	});

	it('claims to hold the costs only when they rest on the readings', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'tracked' } });
		expect(await screen.findByText('Your account readings')).toBeTruthy();
		expect(screen.queryByText('Costs from your account')).toBeNull();
	});

	it('keeps the items collapsed until asked', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		const toggle = await screen.findByRole('button', { name: 'Item by item' });
		expect(toggle.getAttribute('aria-expanded')).toBe('false');
		expect(toggle.getAttribute('aria-controls')).toBeNull();
		expect(screen.queryByText(/Sold to the Trade Terminal/)).toBeNull();
		await fireEvent.click(toggle);
		expect(toggle.getAttribute('aria-expanded')).toBe('true');
		const region = toggle.getAttribute('aria-controls');
		expect(region && document.getElementById(region)).toBeTruthy();
		expect(screen.getByText('Sold to the Trade Terminal: 12.40 PED; 1240 Shrapnel')).toBeTruthy();
		// The label does not change as it opens, so nothing beside it moves.
		expect(toggle.textContent?.trim()).toBe('Item by item');
	});

	it('lists each item with what it lost, and says which amounts rest on rounded values', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		await fireEvent.click(await screen.findByRole('button', { name: 'Item by item' }));
		const details = screen.getByTestId('measure-details');
		const rifle = within(details).getByText('Rifle').parentElement;
		expect(rifle?.textContent).toContain('0.7500');
		const harness = within(details).getAllByText('Harness')[0].parentElement;
		expect(harness?.textContent).toContain('0.40');
		expect(within(details).getByText('Universal Ammo').parentElement?.textContent).toContain(
			'95000 burned',
		);
		expect(within(details).getByTestId('measure-rounded-note').textContent).toMatch(
			/rounds down to the PEC/,
		);
	});

	describe('item by item in the session detail, by the bucket each item was billed to', () => {
		function costLine(overrides: Partial<MeasuredCostLine>): MeasuredCostLine {
			return {
				kind: 'decay',
				item: 'Item',
				units: null,
				amount: 0.1,
				precision: 'exact',
				markupPercent: null,
				markupNotIncluded: false,
				unpriced: null,
				peggedLedgerEntryId: null,
				peggedLedgerEntryDescription: null,
				...overrides,
			};
		}

		const costs: MeasuredCostDetail = {
			buckets: [
				{
					bucket: 'weapon',
					lines: [
						costLine({ kind: 'ammoBurned', item: 'Universal Ammo', units: 95000, amount: 9.5 }),
						costLine({ item: 'Rifle', amount: 0.75 }),
					],
				},
				{
					bucket: 'healing',
					lines: [
						costLine({
							kind: 'consumed',
							item: 'Vivo Pill',
							units: 4,
							amount: null,
							unpriced: 'boughtThroughLedger',
						}),
					],
				},
				{
					bucket: 'armour',
					lines: [
						costLine({
							kind: 'limitedDecay',
							item: 'Ghost Helmet (L)',
							amount: 0.2,
							markupPercent: 140,
						}),
					],
				},
				{ bucket: 'travel', lines: [costLine({ item: 'Teleport Chip', amount: 0.05 })] },
			],
			armourParts: [
				{
					part: 'limitedSet',
					setId: 1,
					setName: 'Ghost (L)',
					markupPercent: 140,
					lost: 0.2,
					lostPrecision: 'exact',
					cost: 0.28,
					costPrecision: 'exact',
				},
			],
		};

		it('groups the lines by Cycled bucket, with the armour in parts and unpriced lines saying why', async () => {
			api.getSessionMeasure.mockResolvedValue(measured());
			render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured', costs } });
			await fireEvent.click(await screen.findByRole('button', { name: 'Item by item' }));
			const details = screen.getByTestId('measure-details');
			const weapon = within(details).getByTestId('cost-group-weapon');
			expect(weapon.textContent).toContain('Universal Ammo');
			expect(weapon.textContent).toContain('95000 units burned');
			expect(weapon.textContent).toContain('Rifle');
			expect(within(details).getByTestId('cost-group-healing').textContent).toContain(
				'not charged here: you record its purchase in the ledger',
			);
			const parts = within(details).getByTestId('armour-parts');
			expect(parts.textContent).toContain('Ghost (L)');
			expect(parts.textContent).toContain('at 140.0% markup');
			expect(parts.textContent).toContain('0.2800');
			// The raw grouping is replaced, and overhead is listed with the overhead.
			expect(within(details).queryByText('Wear')).toBeNull();
			expect(within(details).queryByText('Ammo')).toBeNull();
			expect(within(details).queryByTestId('cost-group-travel')).toBeNull();
			expect(details.textContent).not.toContain('Teleport Chip');
			// What is not a cost keeps its own place.
			expect(within(details).getByText('Repairs')).toBeTruthy();
			// Only what this view lists decides the rounding note: the wear the
			// readings saw in whole PEC is not listed here, and every line is exact.
			expect(within(details).queryByTestId('measure-rounded-note')).toBeNull();
		});

		it('says a session measured across a clean gap was measured, and never that its end was missed', async () => {
			api.getSessionMeasure.mockResolvedValue(
				measured({
					coverage: {
						level: 'partial',
						opening: 'read',
						closing: 'unread',
						readingCount: 40,
						gapCount: 0,
					},
				}),
			);
			render(SessionMeasure, {
				props: { sessionId: 's1', costBasis: 'measured', costs, acrossGapMinutes: 7 },
			});
			const coverage = await screen.findByTestId('measure-coverage');
			expect(coverage.textContent?.trim()).toBe(
				'Complete: measured across the gap noted with the costs.',
			);
			expect(screen.queryByText(/missed/)).toBeNull();
		});
	});

	it('shows a repair as settlement, never as a cost', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		await fireEvent.click(await screen.findByRole('button', { name: 'Item by item' }));
		const details = screen.getByTestId('measure-details');
		expect(
			within(details).getByText('A repair settles wear already counted, so it is not a cost.'),
		).toBeTruthy();
		const repair = within(details).getByText('restored 1.0000 PED').parentElement;
		if (!repair) throw new Error('no repair line');
		// What the PED Card paid is a cross-check beside the item, not the line's figure.
		expect(repair.textContent).toContain('Harness; 1.02 PED paid');
		expect(repair.lastElementChild?.textContent?.trim()).toBe('restored 1.0000 PED');
	});

	it('keeps the block when a later read fails, and says why', async () => {
		api.getSessionMeasure
			.mockResolvedValueOnce(measured())
			.mockRejectedValueOnce(new Error('the readings could not be read'));
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		await screen.findByText('Costs from your account');
		await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
		for (const handler of frames.handlers) handler();
		expect((await screen.findByRole('alert')).textContent).toMatch(/could not be read/);
		expect(screen.getByText('Costs from your account')).toBeTruthy();
		expect(screen.getByText('Complete: the readings cover the whole session.')).toBeTruthy();
	});

	it('re-reads when the account topic fires', async () => {
		api.getSessionMeasure.mockResolvedValueOnce(null).mockResolvedValue(measured());
		render(SessionMeasure, { props: { sessionId: 's1', costBasis: 'measured' } });
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledTimes(1));
		await waitFor(() => expect(frames.handlers.length).toBeGreaterThan(0));
		for (const handler of frames.handlers) handler();
		expect(await screen.findByText('Costs from your account')).toBeTruthy();
	});

	it('never shows a measure read for one session under another', async () => {
		const first = deferred<AccountSessionMeasure | null>();
		const second = deferred<AccountSessionMeasure | null>();
		api.getSessionMeasure.mockImplementation((id: string) =>
			id === 's1' ? first.promise : second.promise,
		);
		const { rerender, container } = render(SessionMeasure, {
			props: { sessionId: 's1', costBasis: 'measured' },
		});
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s1'));
		await rerender({ sessionId: 's2', costBasis: 'measured' });
		// The first session's measure arrives after the player moved on.
		first.resolve(measured({ sessionId: 's1' }));
		await waitFor(() => expect(api.getSessionMeasure).toHaveBeenCalledWith('s2'));
		expect(container.textContent?.trim()).toBe('');
		second.resolve(null);
		await new Promise((settle) => setTimeout(settle, 0));
		expect(container.textContent?.trim()).toBe('');
	});
});

describe('the measure opened from the review', () => {
	/** Mounted as the review mounts it: unframed, with its tracked figures. */
	function review(props: { tracked?: TrackedFigures; ended?: boolean } = {}) {
		return render(SessionMeasure, {
			props: { sessionId: 's1', tracked, framed: false, ...props },
		});
	}

	it('shows its loading state, then the measure', async () => {
		const read = deferred<AccountSessionMeasure | null>();
		api.getSessionMeasure.mockReturnValue(read.promise);
		const { container } = review();
		expect(container.querySelector('[aria-busy="true"]')).toBeTruthy();
		read.resolve(measured());
		expect(await screen.findByTestId('measure-block')).toBeTruthy();
		expect(screen.queryByText('Costs from your account')).toBeNull();
	});

	it('says why when the measure cannot be read', async () => {
		api.getSessionMeasure.mockRejectedValue(new Error('Session not found'));
		review();
		expect((await screen.findByRole('alert')).textContent).toMatch(/Session not found/);
	});

	it('sets each tracked estimate beside its measured figure, read from the totals, with no total', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		review();
		await screen.findByTestId('measure-block');
		expect(screen.getByText('Complete: the readings cover the whole session.')).toBeTruthy();
		const weapon = row('Weapon');
		// Exact measured: tracked to four decimals too, so the row adds up.
		expect(within(weapon).getByText('10.0000')).toBeTruthy();
		expect(within(weapon).getByText('10.2500')).toBeTruthy();
		expect(within(weapon).getByText('+0.2500')).toBeTruthy();
		const armour = row('Armour');
		// Whole-PEC measured: tracked stays at two.
		expect(within(armour).getByText('0.50')).toBeTruthy();
		expect(within(armour).getByText('0.40')).toBeTruthy();
		expect(screen.queryByText(/^Total$/)).toBeNull();
		expect(screen.queryByText(/Net/)).toBeNull();
	});

	it('takes the comparison from the measure totals, not from its lines', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				totals: totals({ ammo: { kind: 'settled', burned: exact(3) }, gained: exact(9) }),
			}),
		);
		review();
		await screen.findByTestId('measure-block');
		expect(within(row('Weapon')).getByText('3.0000')).toBeTruthy();
	});

	it('sets each aside beneath the measured figure it qualifies', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		review();
		await screen.findByTestId('measure-block');
		const loot = within(row('Loot')).getAllByRole('cell');
		expect(loot[2].textContent).toContain('9.0000');
		expect(loot[2].textContent).toContain("of which 2.0000 PED may be a strongbox's contents");
		expect(loot[0].textContent?.trim()).toBe('Loot');
		const armour = within(row('Armour')).getAllByRole('cell');
		expect(armour[2].textContent).toMatch(/repairs cost/);
	});

	it('marks a partial measure as partial, and differences none of it against the whole', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				coverage: {
					level: 'partial',
					opening: 'read',
					closing: 'read',
					readingCount: 50,
					gapCount: 3,
				},
			}),
		);
		review();
		expect(
			await screen.findByText('Partial: the readings missed part of this session.'),
		).toBeTruthy();
		expect(screen.getByText('Measured, part of the session')).toBeTruthy();
		for (const label of ['Weapon', 'Healing', 'Armour', 'Other', 'Loot']) {
			const cells = within(row(label)).getAllByRole('cell');
			expect(cells[3].textContent?.trim()).toBe(NO_DATA);
		}
	});

	it('treats a measure still being read like a partial one, and says an ended session awaits its closing readings', async () => {
		api.getSessionMeasure.mockResolvedValue(
			measured({
				sealed: false,
				coverage: {
					level: 'partial',
					opening: 'read',
					closing: 'unread',
					readingCount: 9,
					gapCount: 0,
				},
			}),
		);
		review({ ended: true });
		expect(
			await screen.findByText('The session has ended; its closing readings are still being taken.'),
		).toBeTruthy();
		expect(within(row('Weapon')).getAllByRole('cell')[3].textContent?.trim()).toBe(NO_DATA);
	});

	it('shows a dash, never zero, where nothing was tracked', async () => {
		api.getSessionMeasure.mockResolvedValue(measured());
		review({ tracked: NOTHING_TRACKED });
		await screen.findByTestId('measure-block');
		const cells = within(row('Loot')).getAllByRole('cell');
		expect(cells[1].textContent?.trim()).toBe(NO_DATA);
		expect(cells[3].textContent?.trim()).toBe(NO_DATA);
	});
});
