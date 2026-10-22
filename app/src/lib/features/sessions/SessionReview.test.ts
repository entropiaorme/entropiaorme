// @vitest-environment happy-dom

import { fireEvent, render, screen, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDefinition } from '$lib/api';
import type { SessionExclusion, TrackingSession } from '$lib/types/tracking';
import { createInstancesModel } from './instancesModel.svelte';
import { createReviewModel } from './reviewModel.svelte';
import SessionReview from './SessionReview.svelte';

vi.mock('$lib/api', () => ({
	PROTECTION_TOPIC: 'protection:updated',
	HEALING_TOPIC: 'healing:updated',
	WEAPONS_TOPIC: 'weapons:updated',
	CONSUMABLES_TOPIC: 'consumables:updated',
	getTrackingSessions: vi.fn(),
	getSessionDetail: vi.fn(async () => {
		throw new Error('no detail in this test');
	}),
	getUnrecordedArmourSessions: vi.fn(async () => []),
	getUnpricedShotSessions: vi.fn(async () => []),
	deleteSession: vi.fn(),
	reassignSession: vi.fn(),
	setSessionExclusion: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async () => () => {}),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

const economyOut: SessionExclusion = {
	economyExcludedAt: '2026-10-22T10:00:00+00:00',
	progressionExcludedAt: null,
	note: 'hotbar misread the weapon',
};

function session(id: string, overrides: Partial<TrackingSession> = {}): TrackingSession {
	return {
		id,
		startTime: '2026-10-20T10:00:00+00:00',
		endTime: '2026-10-20T11:00:00+00:00',
		duration: 3600,
		primaryMobs: ['Carabok'],
		primaryWeapons: [],
		cost: 10,
		returns: 12,
		net: 2,
		returnRate: 1.2,
		globals: 0,
		hofs: 0,
		exclusion: null,
		...overrides,
	};
}

const definition: SessionDefinition = {
	id: '1',
	name: 'Carabok run',
	adHocSegments: false,
	trackProtectionCosts: true,
	questPlanet: null,
	isProtected: false,
	isActive: true,
	instanceCount: 3,
	createdAt: 1,
	updatedAt: null,
	roster: [],
};

async function mount(sessions: TrackingSession[]) {
	mocked.getTrackingSessions.mockResolvedValue({
		sessions,
		nextCursor: null,
		total: sessions.length,
	});
	const model = createReviewModel({
		listAllDefinitions: vi.fn(async () => [definition]),
		restoreDefinition: vi.fn(),
		refreshPlayableDefinitions: vi.fn(async () => {}),
		createInstances: (definitionId) => createInstancesModel({ definitionId }),
	});
	await model.openReview('1');
	render(SessionReview, { props: { model } });
	await tick();
	return model;
}

function rowOf(text: string): HTMLElement {
	const row = screen.getByText(text, { selector: 'td *, td' }).closest('tr');
	if (!row) throw new Error(`no row for ${text}`);
	return row as HTMLElement;
}

beforeEach(() => vi.clearAllMocks());

describe('stats exclusion in the session review', () => {
	it('marks an excluded row and dims its net, leaving counted rows alone', async () => {
		await mount([
			session('kept', { primaryMobs: ['Atrox'] }),
			session('out', { exclusion: economyOut }),
		]);

		const marks = screen.getAllByTestId('exclusion-mark');
		expect(marks).toHaveLength(1);
		expect(marks[0].textContent?.trim()).toBe('Not in PED stats');
		expect(marks[0].getAttribute('title')).toBe('hotbar misread the weapon');

		const out = rowOf('Carabok');
		const net = within(out).getByText(/\+.*2/);
		expect(net.className).toContain('text-text-tertiary');
		const kept = rowOf('Atrox');
		expect(within(kept).getByText(/\+.*2/).className).toContain('text-positive');
	});

	it('toggles a scope from the row menu and shows the stored state', async () => {
		mocked.setSessionExclusion.mockResolvedValue({ sessionId: 'kept', exclusion: economyOut });
		await mount([session('kept')]);

		await fireEvent.click(screen.getByLabelText('Statistics this session counts towards'));
		const [ped, skills] = screen.getAllByRole('menuitemcheckbox');
		expect(ped.getAttribute('aria-checked')).toBe('true');
		expect(skills.getAttribute('aria-checked')).toBe('true');

		await fireEvent.click(ped);
		await tick();

		expect(mocked.setSessionExclusion).toHaveBeenCalledWith('kept', true, false, null);
		expect(screen.getByTestId('exclusion-mark').textContent?.trim()).toBe('Not in PED stats');
		// The menu stays open on the stored state, now offering the note.
		expect(screen.getAllByRole('menuitemcheckbox')[0].getAttribute('aria-checked')).toBe('false');
		expect(screen.getByLabelText('Why (optional)')).toBeTruthy();
	});

	it('saves an edited note on Enter, and only when it changed', async () => {
		mocked.setSessionExclusion.mockResolvedValue({
			sessionId: 'out',
			exclusion: { ...economyOut, note: 'chat log stalled' },
		});
		await mount([session('out', { exclusion: economyOut })]);
		await fireEvent.click(screen.getByLabelText('Statistics this session counts towards'));

		const note = screen.getByLabelText('Why (optional)') as HTMLInputElement;
		await fireEvent.keyDown(note, { key: 'Enter' });
		expect(mocked.setSessionExclusion).not.toHaveBeenCalled();

		note.value = 'chat log stalled';
		await fireEvent.keyDown(note, { key: 'Enter' });
		expect(mocked.setSessionExclusion).toHaveBeenCalledWith('out', true, false, 'chat log stalled');
	});

	it('offers no change on a running session', async () => {
		await mount([session('live', { endTime: null })]);
		await fireEvent.click(screen.getByLabelText('Statistics this session counts towards'));

		for (const item of screen.getAllByRole('menuitemcheckbox')) {
			expect((item as HTMLButtonElement).disabled).toBe(true);
		}
		expect(screen.getByText('Stop the session to change this.')).toBeTruthy();
	});

	it('says what an open row is left out of and includes it again', async () => {
		mocked.setSessionExclusion.mockResolvedValue({ sessionId: 'out', exclusion: null });
		await mount([session('out', { exclusion: economyOut })]);

		await fireEvent.click(rowOf('Carabok'));
		await tick();
		const line = screen.getByTestId('exclusion-line');
		expect(line.textContent).toContain('Left out of PED stats since');
		expect(line.textContent).toContain('hotbar misread the weapon');

		await fireEvent.click(within(line).getByText('Include again'));
		await tick();
		expect(mocked.setSessionExclusion).toHaveBeenCalledWith('out', false, false, null);
		expect(screen.queryByTestId('exclusion-line')).toBeNull();
		expect(screen.queryByTestId('exclusion-mark')).toBeNull();
	});
});
