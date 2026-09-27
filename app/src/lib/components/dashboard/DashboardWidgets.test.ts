// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { RecentEvent } from '$lib/api';
import DashboardWidgets from './DashboardWidgets.svelte';

function event(overrides: Partial<RecentEvent> = {}): RecentEvent {
	return {
		id: 'ne-0',
		type: 'global',
		eventType: 'global_kill',
		description: 'Global! Combibo Young',
		value: 52.4,
		timestamp: '2026-01-01T14:05:00.000Z',
		...overrides,
	};
}

function props(recentEvents: RecentEvent[] | null) {
	return {
		trackingPending: recentEvents === null,
		recentEvents,
		sessionId: 's1',
		multiplierHistory: null,
		cumulativeNetHistory: null,
		// Unread, so the Quests tab holds a placeholder and needs no backend.
		activityOptions: null,
		quests: [],
		pendingCancelChoiceQuestId: null,
		copiedWp: null,
		onQuestStart: vi.fn(),
		onQuestComplete: vi.fn(),
		onQuestCancel: vi.fn(),
		onToggleCancelChoice: vi.fn(),
		onCopyWaypoint: vi.fn(),
		onEditSession: vi.fn(),
		getCooldownRemaining: () => null,
	};
}

const tab = (name: RegExp) => screen.getByRole('tab', { name });
const attention = () => screen.queryByTestId('tab-attention');

describe('dashboard widgets', () => {
	it('opens on the recent events feed', () => {
		render(DashboardWidgets, props([event()]));
		expect(tab(/Recent Events/).getAttribute('aria-selected')).toBe('true');
		expect(screen.getByText('Global! Combibo Young')).toBeTruthy();
		expect(screen.getByText('52.40 PED')).toBeTruthy();
		expect(screen.getByText('2:05 PM')).toBeTruthy();
	});

	it('marks the events tab when an event arrives while another tab is open', async () => {
		const first = event();
		const view = render(DashboardWidgets, props([first]));
		await fireEvent.click(tab(/Quests/));
		expect(attention()).toBeNull();

		const hof = event({
			type: 'hof',
			description: 'HOF! Atrox',
			value: 312,
			timestamp: '2026-01-01T14:09:00.000Z',
		});
		await view.rerender(props([hof, { ...first, id: 'ne-1' }]));
		expect(attention()).toBeTruthy();
		expect(tab(/Recent Events\s*, new/)).toBeTruthy();

		// Opening the feed acknowledges it, and the mark stays gone on leaving.
		await fireEvent.click(tab(/Recent Events/));
		expect(attention()).toBeNull();
		expect(screen.getByText('HOF! Atrox')).toBeTruthy();
		await fireEvent.click(tab(/Quests/));
		expect(attention()).toBeNull();
	});

	it('does not mark events that arrive while the feed is open', async () => {
		const view = render(DashboardWidgets, props([event()]));
		await view.rerender(
			props([event({ timestamp: '2026-01-01T14:09:00.000Z' }), event({ id: 'ne-1' })]),
		);
		await fireEvent.click(tab(/Quests/));
		expect(attention()).toBeNull();
	});

	it('does not treat the first read of the feed as news', async () => {
		const view = render(DashboardWidgets, props(null));
		await fireEvent.click(tab(/Quests/));
		await view.rerender(props([event()]));
		expect(attention()).toBeNull();
	});
});
