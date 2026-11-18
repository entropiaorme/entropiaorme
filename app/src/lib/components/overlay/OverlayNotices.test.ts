// @vitest-environment happy-dom

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import OverlayNotices from './OverlayNotices.svelte';

const guardrail =
	'Harvest guardrail: Short Boards were looted while ChopChop Jr was equipped; costs are attributed to Timber Saw';

describe('OverlayNotices', () => {
	it('renders each message in full inside a live region', () => {
		render(OverlayNotices, {
			props: {
				notices: [
					{ id: 1, text: guardrail, tone: 'warning' },
					{ id: 2, text: 'Protection selection failed', tone: 'error' },
				],
			},
		});
		const rail = screen.getByTestId('overlay-notices');
		expect(rail.getAttribute('role')).toBe('status');
		const notice = screen.getByText(guardrail);
		expect(notice.classList.contains('truncate')).toBe(false);
		expect(notice.classList.contains('notice-warning')).toBe(true);
		expect(screen.getByText('Protection selection failed').classList.contains('notice-error')).toBe(
			true,
		);
	});

	it('holds while the pointer is over the rail and releases when it leaves', async () => {
		const onHold = vi.fn();
		const onRelease = vi.fn();
		render(OverlayNotices, {
			props: { notices: [{ id: 1, text: guardrail, tone: 'warning' }], onHold, onRelease },
		});
		const rail = screen.getByTestId('overlay-notices');
		await fireEvent.pointerEnter(rail);
		expect(onHold).toHaveBeenCalledTimes(1);
		await fireEvent.pointerLeave(rail);
		expect(onRelease).toHaveBeenCalledTimes(1);
	});
});
