// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { Quest } from '$lib/types';
import QuestActions from './QuestActions.svelte';

function quest(completionTrigger: Quest['completionTrigger']): Quest {
	return {
		id: '1',
		name: 'AI Daily',
		category: null,
		targetMobs: [],
		planet: 'Calypso',
		waypoint: null,
		cooldownDurationHours: null,
		cooldownExpiresAt: null,
		reward: null,
		rewardIsSkill: false,
		rewardDescription: '',
		notes: '',
		chainName: null,
		chainPosition: null,
		chainTotal: null,
		startedAt: 100,
		signalLootItem: null,
		completionTrigger,
		rewardPolicy: completionTrigger === 'manual_hand_in' ? 'completion_clump' : 'none',
		rewardItemNames: [],
		cooldownAnchor: 'completion',
		lastStartedAt: 100,
		familyId: null,
		familyName: null,
		familyCooldownDurationHours: null,
		familyCooldownAnchor: null,
		familyCooldownExpiresAt: null,
		rewardUndoAvailable: false,
		curated: false,
		catalogueKey: null,
		lastAbandonedAt: null,
		lastCompletedAt: null,
	};
}

function props(completionTrigger: Quest['completionTrigger']) {
	return {
		quest: quest(completionTrigger),
		status: 'ready' as const,
		remaining: null,
		pendingCancelChoice: false,
		onStart: vi.fn(),
		onComplete: vi.fn(),
		onCancel: vi.fn(),
		onToggleCancelChoice: vi.fn(),
	};
}

describe('QuestActions', () => {
	it('routes manual hand-in quests to the active overlay without generic completion', () => {
		const model = props('manual_hand_in');
		render(QuestActions, { props: model });

		expect(screen.getByText('Hand in from overlay')).not.toBeNull();
		expect(screen.queryByRole('button', { name: 'Complete' })).toBeNull();
		expect(model.onComplete).not.toHaveBeenCalled();
	});

	it('keeps generic completion available for mission-log quests', async () => {
		const model = props('mission_log');
		render(QuestActions, { props: model });

		await fireEvent.click(screen.getByRole('button', { name: 'Complete' }));
		expect(model.onComplete).toHaveBeenCalledOnce();
	});

	it('offers a confirmed reward undo even when the quest has no cooldown', async () => {
		const model = props('mission_log');
		model.quest.startedAt = null;
		model.quest.rewardUndoAvailable = true;
		const { rerender } = render(QuestActions, { props: model });

		await fireEvent.click(screen.getByRole('button', { name: 'Undo reward' }));
		expect(model.onToggleCancelChoice).toHaveBeenCalledOnce();
		await rerender({ ...model, pendingCancelChoice: true });
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm undo' }));
		expect(model.onCancel).toHaveBeenCalledWith(true);
	});

	it('routes manual quest starts through session Activities', () => {
		const model = props('manual_hand_in');
		model.quest.startedAt = null;
		render(QuestActions, { props: model });

		expect(screen.getByText('Start from session Activities')).not.toBeNull();
		expect(screen.queryByRole('button', { name: 'Start' })).toBeNull();
		expect(model.onStart).not.toHaveBeenCalled();
	});
});
