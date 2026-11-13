// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import NotTracked from './NotTracked.svelte';
import { createNotTrackedModel } from './notTrackedModel.svelte';

vi.mock('$lib/api', () => ({
	getDismissedTools: vi.fn(),
	trackToolAgain: vi.fn(),
}));

import * as api from '$lib/api';

const mocked = vi.mocked(api);

const chip = { name: 'Teleport Chip', dismissedAt: 10 };
const repair = { name: 'Repair Unit', dismissedAt: 20 };

describe('what is not tracked', () => {
	beforeEach(() => vi.clearAllMocks());

	it('reads nothing and shows nothing while the reading is off', () => {
		render(NotTracked, { props: { readingOn: false } });
		expect(mocked.getDismissedTools).not.toHaveBeenCalled();
		expect(screen.queryByTestId('not-tracked')).toBeNull();
	});

	it('shows nothing when nothing is set aside', async () => {
		mocked.getDismissedTools.mockResolvedValueOnce([]);
		render(NotTracked, { props: { readingOn: true } });
		await vi.waitFor(() => expect(mocked.getDismissedTools).toHaveBeenCalled());
		expect(screen.queryByTestId('not-tracked')).toBeNull();
	});

	it('names what is set aside in one quiet line and brings each back', async () => {
		mocked.getDismissedTools.mockResolvedValueOnce([chip, repair]);
		render(NotTracked, { props: { readingOn: true } });
		const line = await screen.findByRole('button', {
			name: 'Not tracked: Teleport Chip and Repair Unit',
		});
		expect(line.getAttribute('aria-expanded')).toBe('false');
		expect(screen.queryByRole('button', { name: 'Track Teleport Chip again' })).toBeNull();

		await fireEvent.click(line);
		mocked.trackToolAgain.mockResolvedValueOnce([repair]);
		await fireEvent.click(screen.getByRole('button', { name: 'Track Teleport Chip again' }));
		expect(mocked.trackToolAgain).toHaveBeenCalledWith('Teleport Chip');
		await vi.waitFor(() =>
			expect(screen.getByRole('button', { name: 'Not tracked: Repair Unit' })).toBeTruthy(),
		);
	});

	it('drops a read overtaken by a newer one', async () => {
		const model = createNotTrackedModel();
		let first!: (names: (typeof chip)[]) => void;
		mocked.getDismissedTools.mockReturnValueOnce(
			new Promise((resolve) => {
				first = resolve;
			}),
		);
		const stale = model.refresh();
		mocked.getDismissedTools.mockResolvedValueOnce([repair]);
		await model.refresh();
		first([chip, repair]);
		await stale;
		expect(model.names).toEqual([repair]);
	});
});
