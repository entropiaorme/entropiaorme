// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { ActivityOptionsResult } from '$lib/api';
import type { UnresolvedQuestReward } from '$lib/types';
import { reactiveBox } from '$lib/view/__fixtures__/reactiveBox.svelte';
import { CATALOGUE, curatedRows, HOUR, NOW, unresolved } from './boardFixtures';
import QuestBoard from './QuestBoard.svelte';
import { createQuestBoardModel, type QuestBoardDeps } from './questBoardModel.svelte';

async function mount(
	options: {
		unresolved?: UnresolvedQuestReward[];
		edit?: (rows: ReturnType<typeof curatedRows>) => void;
	} = {},
) {
	const rows = curatedRows();
	options.edit?.(rows);
	const activity = reactiveBox<ActivityOptionsResult | null>(null);
	const deps: QuestBoardDeps = {
		quests: () => rows.quests,
		families: () => rows.families,
		activityOptions: () => activity.value,
		definition: () => null,
		getCatalogue: vi.fn(async () => CATALOGUE),
		getUnresolved: vi.fn(async () => options.unresolved ?? []),
		startQuest: vi.fn(async () => undefined),
		completeQuest: vi.fn(async () => undefined),
		abandonQuest: vi.fn(async () => undefined),
		cancelQuest: vi.fn(async () => undefined),
		reviewReward: vi.fn(async () => undefined),
		updateDefinition: vi.fn(async () => undefined),
		refresh: vi.fn(async () => undefined),
		refreshDefinitions: vi.fn(async () => undefined),
		getPreference: vi.fn(async <T>(_key: string, fallback: T) => fallback),
		setPreference: vi.fn(async () => undefined),
	};
	const model = createQuestBoardModel(deps);
	await model.init();
	model.now = NOW;
	const handlers = {
		onCopyWaypoint: vi.fn(),
		onCreateCustom: vi.fn(),
		onEditCustom: vi.fn(),
		onDeleteCustom: vi.fn(),
	};
	render(QuestBoard, { model, copiedKey: null, ...handlers });
	return { model, deps, handlers };
}

const rowFor = (label: string) => {
	const row = screen
		.getAllByTestId('quest-board-row')
		.find((candidate) => within(candidate).queryByText(label) !== null);
	if (!row) throw new Error(`no row for ${label}`);
	return row;
};

describe('the quest board', () => {
	it('lays a planet out by its categories, in the catalogue order', async () => {
		await mount();
		expect(screen.getByRole('button', { name: 'ARIS', pressed: true })).toBeTruthy();
		expect(screen.getByRole('heading', { name: 'Daily missions' })).toBeTruthy();
		expect(screen.getByRole('heading', { name: 'Bosses' })).toBeTruthy();
		expect(screen.getByText('Hyperion voucher each')).toBeTruthy();
		expect(within(rowFor('Hunting 1')).getByText("Pick up today's mission")).toBeTruthy();
		expect(within(rowFor('Verdathos')).getByText('Creature boss')).toBeTruthy();
	});

	it('switches planet, naming who hands each mission out and what it pays', async () => {
		await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Cyrene' }));
		const slimed = rowFor('Getting Slimed!');
		expect(within(slimed).getByText('Pick up from Captain Wrigley')).toBeTruthy();
		expect(within(slimed).getByText('2 tokens')).toBeTruthy();
		await fireEvent.click(within(slimed).getByRole('button', { name: /Copy the waypoint/ }));
	});

	it('shows a done slot with its variant and when it is back', async () => {
		await mount({
			edit: (rows) => {
				rows.families[0] = {
					...rows.families[0],
					cooldownExpiresAt: new Date(NOW + 3 * HOUR).toISOString(),
				};
				rows.quests[1] = { ...rows.quests[1], lastCompletedAt: NOW / 1000 - 60 };
			},
		});
		const slot = rowFor('Hunting 1');
		expect(slot.dataset.state).toBe('cooling');
		expect(within(slot).getByText('Defective Drones')).toBeTruthy();
		expect(within(slot).getByText('Done')).toBeTruthy();
		expect(within(slot).getByText('3h 00m')).toBeTruthy();
		expect(screen.getByText('1 of 2 done')).toBeTruthy();
	});

	it('asks once before abandoning a mission in progress', async () => {
		const { deps } = await mount({
			edit: (rows) => {
				rows.quests[0] = { ...rows.quests[0], startedAt: NOW / 1000 - 60 };
			},
		});
		const slot = rowFor('Hunting 1');
		expect(within(slot).getByText('Weak Mortirex')).toBeTruthy();
		expect(within(slot).getByText('In your mission log')).toBeTruthy();
		await fireEvent.click(within(slot).getByRole('button', { name: 'Abandon' }));
		expect(within(slot).getByText('Abandon it in game too?')).toBeTruthy();
		await fireEvent.click(within(slot).getByRole('button', { name: 'Keep' }));
		expect(deps.abandonQuest).not.toHaveBeenCalled();
		await fireEvent.click(within(slot).getByRole('button', { name: 'Abandon' }));
		await fireEvent.click(within(slot).getByRole('button', { name: 'Abandon' }));
		expect(deps.abandonQuest).toHaveBeenCalledWith('10');
	});

	it('settles a split reward with the exact loot line it arrived as', async () => {
		const { deps } = await mount({
			unresolved: [
				unresolved({
					candidates: [
						{
							id: 77,
							itemName: 'Hyperion Daily Voucher',
							quantity: 1,
							valuePed: 0,
							mobName: 'Weak Mortirex',
						},
					],
				}),
			],
		});
		const slot = rowFor('Hunting 1');
		await fireEvent.click(within(slot).getByRole('button', { name: 'Settle reward' }));
		const panel = screen.getByTestId('quest-settle-panel');
		expect(within(panel).getByText(/from Weak Mortirex/)).toBeTruthy();
		expect(within(panel).queryByRole('button', { name: 'It was paid' })).toBeNull();
		await fireEvent.click(within(panel).getByRole('button', { name: 'Use this' }));
		expect(deps.reviewReward).toHaveBeenCalledWith({ completionId: 1, lootItemIds: [77] });
	});

	it('lets an unevidenced named reward be declared paid', async () => {
		const { deps } = await mount({ unresolved: [unresolved({ questId: '11', completionId: 4 })] });
		await fireEvent.click(
			within(rowFor('Hunting 1')).getByRole('button', { name: 'Settle reward' }),
		);
		const panel = screen.getByTestId('quest-settle-panel');
		expect(within(panel).getByText(/Nothing recorded shows Hyperion Daily Voucher/)).toBeTruthy();
		await fireEvent.click(within(panel).getByRole('button', { name: 'It was paid' }));
		expect(deps.reviewReward).toHaveBeenCalledWith({ completionId: 4, declarePaid: true });
	});

	it('never offers a control a completion without evidence cannot honour', async () => {
		await mount({
			unresolved: [
				unresolved({
					questId: '30',
					declarable: false,
					policy: 'completion_clump',
					isolated: null,
					reason: 'Historical completion',
				}),
			],
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Cyrene' }));
		await fireEvent.click(
			within(rowFor('Getting Slimed!')).getByRole('button', { name: 'Settle reward' }),
		);
		const panel = screen.getByTestId('quest-settle-panel');
		expect(within(panel).getByText('Historical completion')).toBeTruthy();
		expect(within(panel).queryByText(/tick/i)).toBeNull();
		expect(
			within(panel)
				.getAllByRole('button')
				.map((button) => button.textContent?.trim()),
		).toEqual(['No reward']);
	});

	it("settles a deleted quest's reward from the foot of the board", async () => {
		const { deps } = await mount({
			unresolved: [
				unresolved({ completionId: 9, questId: '35', questName: 'ARIS - Daily Samples' }),
			],
		});
		await fireEvent.click(screen.getByRole('button', { name: '1 earlier reward to settle' }));
		const panel = screen.getByTestId('quest-settle-panel');
		await fireEvent.click(within(panel).getByRole('button', { name: 'It was paid' }));
		expect(deps.reviewReward).toHaveBeenCalledWith({ completionId: 9, declarePaid: true });
	});

	it('opens a custom quest on the planet in view', async () => {
		const { handlers } = await mount();
		await fireEvent.click(screen.getByRole('button', { name: 'Cyrene' }));
		await fireEvent.click(screen.getByRole('button', { name: '+ Custom quest' }));
		expect(handlers.onCreateCustom).toHaveBeenCalledWith('Cyrene');
	});
});
