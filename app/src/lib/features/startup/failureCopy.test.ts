import { describe, expect, it } from 'vitest';
import type { SubstrateDeclineReason } from '$lib/api/shell';
import { failureCopy, failureReport } from './failureCopy';

const REASONS: SubstrateDeclineReason[] = [
	'data_dir_unavailable',
	'database_below_baseline',
	'database_unreadable',
	'game_data_unavailable',
	'tracking_unavailable',
	'unexpected',
];

describe('failureCopy', () => {
	it.each(REASONS)('gives %s a sentence and a remedy', (reason) => {
		const copy = failureCopy(reason);
		expect(copy.summary).toMatch(/^[A-Z].*\.$/);
		expect(copy.remedy).toMatch(/^[A-Z].*\.$/);
	});

	it('keeps the logged detail out of the prose', () => {
		for (const reason of REASONS) {
			const { summary, remedy } = failureCopy(reason);
			expect(`${summary} ${remedy}`).not.toContain(reason);
		}
	});

	it('points a missing bundle at a reinstall rather than a restart', () => {
		expect(failureCopy('game_data_unavailable').remedy).toMatch(/Reinstall/);
	});
});

describe('failureReport', () => {
	it('carries the version, the reason, and the detail', () => {
		expect(
			failureReport(
				{ reason: 'database_unreadable', detail: 'database open failed (locked)' },
				'0.3.0',
			),
		).toBe(
			[
				'EntropiaOrme 0.3.0 could not start.',
				'Reason: database_unreadable',
				'Detail: database open failed (locked)',
			].join('\n'),
		);
	});

	it('omits the version when it is unknown', () => {
		expect(failureReport({ reason: 'unexpected', detail: 'panic' }, null)).toMatch(
			/^EntropiaOrme could not start\./,
		);
	});
});
