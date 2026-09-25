import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionDetail, TrackingSession } from '$lib/types/tracking';
import { createInstancesModel, PAGE_SIZE } from './instancesModel.svelte';

vi.mock('$lib/api', () => ({
	PROTECTION_TOPIC: 'protection:updated',
	getTrackingSessions: vi.fn(),
	getSessionDetail: vi.fn(),
	getUnrecordedArmourSessions: vi.fn(async () => []),
	deleteSession: vi.fn(),
	reassignSession: vi.fn(),
}));

const protectionListeners: Array<() => void> = [];
vi.mock('@tauri-apps/api/event', () => ({
	listen: vi.fn(async (_topic: string, handler: () => void) => {
		protectionListeners.push(handler);
		return () => {};
	}),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

function session(overrides: Partial<TrackingSession> = {}): TrackingSession {
	return {
		id: 's1',
		startTime: '2026-07-01T10:00:00Z',
		duration: 3600,
		primaryMobs: ['Atrox Young'],
		net: 1.5,
		globals: 0,
		hofs: 0,
		...overrides,
	} as TrackingSession;
}

function detail(): SessionDetail {
	return { id: 's1' } as unknown as SessionDetail;
}

function page(
	sessions: TrackingSession[],
	nextCursor: string | null = null,
	total: number = sessions.length,
) {
	return { sessions, nextCursor, total };
}

beforeEach(() => {
	vi.clearAllMocks();
});

describe('loadSessions', () => {
	it('loads the session list and clears the loading flag', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		const model = createInstancesModel();
		await model.loadSessions();

		expect(model.sessions).toHaveLength(1);
		expect(model.loading).toBe(false);
		expect(model.error).toBeNull();
	});

	it('surfaces a load failure and clears a stale error on entry', async () => {
		mocked.getTrackingSessions.mockRejectedValueOnce(new Error('backend unreachable'));
		const model = createInstancesModel();
		await model.loadSessions();
		expect(model.error).toBe('backend unreachable');

		mocked.getTrackingSessions.mockResolvedValue(page([]));
		await model.loadSessions();
		expect(model.error).toBeNull();
	});
});

describe('paging', () => {
	it('pages ten sessions per page in backend order', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page(Array.from({ length: 23 }, (_, i) => session({ id: `s${i}` }))),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		expect(PAGE_SIZE).toBe(10);
		expect(model.table.totalPages).toBe(3);
		expect(model.table.pageRows.map((s) => s.id)).toEqual(
			Array.from({ length: 10 }, (_, i) => `s${i}`),
		);

		model.table.page = 2;
		expect(model.table.pageRows.map((s) => s.id)).toEqual(['s20', 's21', 's22']);
	});

	it('clamps the page when the list shrinks past the last page', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page(Array.from({ length: 11 }, (_, i) => session({ id: `s${i}` }))),
		);
		mocked.deleteSession.mockResolvedValue(undefined);
		const model = createInstancesModel();
		await model.loadSessions();

		model.table.page = 1;
		await model.handleDelete('s10');
		expect(model.table.totalPages).toBe(1);
		expect(model.table.page).toBe(0);
	});
});

describe('loadMoreSessions', () => {
	it('appends the next keyset page and grows the pager range', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(
			page([session({ id: 's1' }), session({ id: 's2' })], 'cursor-1'),
		);
		const model = createInstancesModel();
		await model.loadSessions();
		expect(model.nextCursor).toBe('cursor-1');

		mocked.getTrackingSessions.mockResolvedValueOnce(page([session({ id: 's3' })], null));
		await model.loadMoreSessions();

		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith('cursor-1', undefined, undefined);
		expect(model.sessions.map((s) => s.id)).toEqual(['s1', 's2', 's3']);
		expect(model.nextCursor).toBeNull();
	});

	it('is a no-op with no cursor and while a fetch is in flight', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		const model = createInstancesModel();
		await model.loadSessions();

		await model.loadMoreSessions();
		expect(mocked.getTrackingSessions).toHaveBeenCalledTimes(1);
	});

	it('surfaces a load-more failure and keeps the loaded window', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(page([session()], 'cursor-1'));
		const model = createInstancesModel();
		await model.loadSessions();

		mocked.getTrackingSessions.mockRejectedValueOnce(new Error('backend unreachable'));
		await model.loadMoreSessions();
		expect(model.error).toBe('backend unreachable');
		expect(model.sessions).toHaveLength(1);
		expect(model.nextCursor).toBe('cursor-1');
	});
});

describe('on-demand paging', () => {
	it('reports totals from the server, not the loaded window', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 10 }, (_, i) => session({ id: `s${i}` })),
				'cursor-1',
				43,
			),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		expect(model.total).toBe(43);
		expect(model.totalPages).toBe(5);
	});

	it('fetches the next keyset page when Next steps past the loaded window', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 10 }, (_, i) => session({ id: `s${i}` })),
				'cursor-1',
				15,
			),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 5 }, (_, i) => session({ id: `s${10 + i}` })),
				null,
				15,
			),
		);
		await model.nextPage();

		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith('cursor-1', undefined, undefined);
		expect(model.table.page).toBe(1);
		expect(model.table.pageRows.map((s) => s.id)).toEqual(
			Array.from({ length: 5 }, (_, i) => `s${10 + i}`),
		);
	});

	it('steps within the loaded window without a fetch', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 20 }, (_, i) => session({ id: `s${i}` })),
				'cursor-1',
				30,
			),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		await model.nextPage();
		expect(mocked.getTrackingSessions).toHaveBeenCalledTimes(1);
		expect(model.table.page).toBe(1);

		model.prevPage();
		expect(model.table.page).toBe(0);
	});

	it('holds the page when the on-demand fetch fails, and stops at the true last page', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 10 }, (_, i) => session({ id: `s${i}` })),
				'cursor-1',
				15,
			),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		mocked.getTrackingSessions.mockRejectedValueOnce(new Error('backend unreachable'));
		await model.nextPage();
		expect(model.table.page).toBe(0);
		expect(model.error).toBe('backend unreachable');

		mocked.getTrackingSessions.mockResolvedValueOnce(
			page(
				Array.from({ length: 5 }, (_, i) => session({ id: `s${10 + i}` })),
				null,
				15,
			),
		);
		await model.nextPage();
		expect(model.table.page).toBe(1);
		await model.nextPage();
		expect(model.table.page).toBe(1);
	});
});

describe('toggleSession', () => {
	it('expands a row with its detail and collapses on the second toggle', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		mocked.getSessionDetail.mockResolvedValue(detail());
		const model = createInstancesModel();
		await model.loadSessions();

		await model.toggleSession('s1');
		expect(model.expandedSessionId).toBe('s1');
		expect(model.expandedDetail).not.toBeNull();
		expect(model.loadingDetail).toBe(false);

		await model.toggleSession('s1');
		expect(model.expandedSessionId).toBeNull();
		expect(model.expandedDetail).toBeNull();
	});

	it('keeps the row expanded with no detail when the detail fetch fails', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		mocked.getSessionDetail.mockRejectedValue(new Error('gone'));
		const model = createInstancesModel();
		await model.loadSessions();

		await model.toggleSession('s1');
		expect(model.expandedSessionId).toBe('s1');
		expect(model.expandedDetail).toBeNull();
	});
});

describe('handleDelete', () => {
	it('removes the session, collapses it if expanded, and resets the confirm state', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session(), session({ id: 's2' })]));
		mocked.getSessionDetail.mockResolvedValue(detail());
		mocked.deleteSession.mockResolvedValue(undefined);
		const model = createInstancesModel();
		await model.loadSessions();
		await model.toggleSession('s1');
		model.confirmDeleteId = 's1';

		await model.handleDelete('s1');
		expect(model.sessions.map((s) => s.id)).toEqual(['s2']);
		expect(model.expandedSessionId).toBeNull();
		expect(model.confirmDeleteId).toBeNull();
	});

	it('ignores a re-entrant delete while one is in flight', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		let resolveDelete!: () => void;
		mocked.deleteSession.mockImplementation(
			() =>
				new Promise<void>((res) => {
					resolveDelete = res;
				}),
		);
		const model = createInstancesModel();
		await model.loadSessions();

		const first = model.handleDelete('s1');
		await model.handleDelete('s1');
		expect(mocked.deleteSession).toHaveBeenCalledTimes(1);
		resolveDelete();
		await first;
		expect(model.sessions).toHaveLength(0);
	});

	it('surfaces a delete failure and keeps the row', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		mocked.deleteSession.mockRejectedValue(new Error('delete failed'));
		const model = createInstancesModel();
		await model.loadSessions();

		await model.handleDelete('s1');
		expect(model.error).toBe('delete failed');
		expect(model.sessions).toHaveLength(1);
	});
});

describe('collapseAll', () => {
	it('closes the open row and drops its loaded detail', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session(), session({ id: 's2' })]));
		mocked.getSessionDetail.mockResolvedValue(detail());
		const model = createInstancesModel();
		await model.loadSessions();

		await model.toggleSession('s2');
		expect(model.expandedSessionId).toBe('s2');

		model.collapseAll();
		expect(model.expandedSessionId).toBeNull();
		expect(model.expandedDetail).toBeNull();
	});
});

describe('the definition scope', () => {
	it('passes the scope to every read, including the cursor page', async () => {
		mocked.getTrackingSessions.mockResolvedValueOnce(page([session()], 'cursor-1', 2));
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();
		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith(undefined, undefined, '7');

		mocked.getTrackingSessions.mockResolvedValueOnce(page([session({ id: 's2' })], null, 2));
		await model.loadMoreSessions();
		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith('cursor-1', undefined, '7');
	});

	it('reads the scope at fetch time, so switching definition reloads the new one', async () => {
		let current = '7';
		mocked.getTrackingSessions.mockResolvedValue(page([session()]));
		const model = createInstancesModel({ definitionId: () => current });
		await model.loadSessions();
		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith(undefined, undefined, '7');

		current = '9';
		await model.loadSessions();
		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith(undefined, undefined, '9');
	});

	it('resets the pager and any open row on reload, so a switch never lands mid-window', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page(Array.from({ length: PAGE_SIZE + 1 }, (_, i) => session({ id: `s${i}` }))),
		);
		mocked.getSessionDetail.mockResolvedValue(detail());
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();
		await model.nextPage();
		await model.toggleSession('s0');
		expect(model.table.page).toBe(1);
		expect(model.expandedSessionId).toBe('s0');

		await model.loadSessions();
		expect(model.table.page).toBe(0);
		expect(model.expandedSessionId).toBeNull();
		expect(model.expandedDetail).toBeNull();
	});
});

describe('reassign', () => {
	it('drops the moved instance from a scoped list and shrinks the total', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page([session({ id: 's1' }), session({ id: 's2' })], null, 2),
		);
		mocked.reassignSession.mockResolvedValue({
			sessionId: 's1',
			definitionId: '9',
			sessionName: 'Carabok Skilling',
		});
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();

		expect(await model.reassign('s1', '9')).toBe(true);
		expect(mocked.reassignSession).toHaveBeenCalledWith('s1', '9');
		expect(model.sessions.map((s) => s.id)).toEqual(['s2']);
		expect(model.total).toBe(1);
	});

	it('keeps the row in an unscoped list, where it still belongs', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session({ id: 's1' })], null, 1));
		mocked.reassignSession.mockResolvedValue({
			sessionId: 's1',
			definitionId: '9',
			sessionName: 'Carabok Skilling',
		});
		const model = createInstancesModel();
		await model.loadSessions();

		expect(await model.reassign('s1', '9')).toBe(true);
		expect(model.sessions.map((s) => s.id)).toEqual(['s1']);
		expect(model.total).toBe(1);
	});

	it('refreshes an open row after an unscoped move and keeps its last-good detail', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session({ id: 's1' })], null, 1));
		mocked.getSessionDetail.mockResolvedValue(detail());
		mocked.reassignSession.mockResolvedValue({
			sessionId: 's1',
			definitionId: '9',
			sessionName: 'Carabok Skilling',
		});
		const model = createInstancesModel();
		await model.loadSessions();
		await model.toggleSession('s1');

		mocked.getSessionDetail.mockClear();
		expect(await model.reassign('s1', '9')).toBe(true);
		expect(mocked.getSessionDetail).toHaveBeenCalledWith('s1');
		const lastGood = model.expandedDetail;

		mocked.getSessionDetail.mockRejectedValueOnce(new Error('detail unavailable'));
		expect(await model.reassign('s1', '9')).toBe(true);
		expect(model.expandedDetail).toBe(lastGood);
	});

	it('surfaces a refusal and leaves the list untouched', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session({ id: 's1' })], null, 1));
		mocked.reassignSession.mockRejectedValueOnce(new Error('Session definition not found'));
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();

		expect(await model.reassign('s1', '9')).toBe(false);
		expect(model.error).toBe('Session definition not found');
		expect(model.sessions).toHaveLength(1);
		expect(model.total).toBe(1);
	});

	it('refuses a second write while one is in flight', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page([session({ id: 's1' }), session({ id: 's2' })], null, 2),
		);
		let release: (() => void) | undefined;
		mocked.reassignSession.mockImplementationOnce(
			() =>
				new Promise((resolve) => {
					release = () => resolve({ sessionId: 's1', definitionId: '9', sessionName: null });
				}),
		);
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();

		const first = model.reassign('s1', '9');
		expect(await model.reassign('s2', '9')).toBe(false);
		release?.();
		expect(await first).toBe(true);
		expect(mocked.reassignSession).toHaveBeenCalledTimes(1);
	});
});

// Armour is recorded when the player repairs, so a row can net without an
// armour cost that is still to come; the list marks those rows.
describe('armour still to record', () => {
	it('marks the loaded sessions no recording covers yet', async () => {
		mocked.getTrackingSessions.mockResolvedValue(
			page([session({ id: 's1' }), session({ id: 's2' })]),
		);
		mocked.getUnrecordedArmourSessions.mockResolvedValue(['s2']);
		const model = createInstancesModel();
		await model.loadSessions();
		expect(mocked.getUnrecordedArmourSessions).toHaveBeenCalledWith(['s1', 's2']);
		expect(model.armourPending('s1')).toBe(false);
		expect(model.armourPending('s2')).toBe(true);
	});

	it('marks the sessions a later page brings in', async () => {
		mocked.getTrackingSessions
			.mockResolvedValueOnce(page([session({ id: 's1' })], 'c1', 2))
			.mockResolvedValueOnce(page([session({ id: 's2' })], null, 2));
		mocked.getUnrecordedArmourSessions.mockResolvedValueOnce([]).mockResolvedValueOnce(['s2']);
		const model = createInstancesModel();
		await model.loadSessions();
		await model.loadMoreSessions();
		expect(mocked.getUnrecordedArmourSessions).toHaveBeenLastCalledWith(['s1', 's2']);
		expect(model.armourPending('s2')).toBe(true);
	});

	it('re-reads rows and marks in place after a recording, keeping the page and open row', async () => {
		const sessions = Array.from({ length: 12 }, (_, i) => session({ id: `s${i}`, net: 1 }));
		mocked.getTrackingSessions.mockResolvedValue(page(sessions));
		mocked.getUnrecordedArmourSessions.mockResolvedValue(['s11']);
		mocked.getSessionDetail.mockResolvedValue(detail());
		const model = createInstancesModel({ definitionId: () => '7' });
		await model.loadSessions();
		await model.nextPage();
		await model.toggleSession('s11');

		mocked.getTrackingSessions.mockResolvedValue(
			page([...sessions.slice(0, 11), session({ id: 's11', net: -3 })]),
		);
		mocked.getUnrecordedArmourSessions.mockResolvedValue([]);
		await model.subscribeProtection();
		protectionListeners.at(-1)?.();
		await vi.waitFor(() => expect(model.armourPending('s11')).toBe(false));

		expect(mocked.getTrackingSessions).toHaveBeenLastCalledWith(undefined, 12, '7');
		expect(model.sessions.find((s) => s.id === 's11')?.net).toBe(-3);
		expect(model.table.page).toBe(1);
		expect(model.expandedSessionId).toBe('s11');
	});

	it('keeps the marks it had when the re-read fails', async () => {
		mocked.getTrackingSessions.mockResolvedValue(page([session({ id: 's1' })]));
		mocked.getUnrecordedArmourSessions.mockResolvedValueOnce(['s1']);
		const model = createInstancesModel();
		await model.loadSessions();
		mocked.getUnrecordedArmourSessions.mockRejectedValueOnce(new Error('offline'));
		await model.refreshAfterProtection();
		expect(model.armourPending('s1')).toBe(true);
	});
});
