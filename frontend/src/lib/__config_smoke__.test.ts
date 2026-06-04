import { describe, expect, it } from 'vitest';
// Imported via the $lib alias on purpose: exercises the vitest.config.ts
// resolve.alias so a broken alias fails here rather than across every suite.
import { formatPed } from '$lib/utils/format';

describe('vitest config smoke', () => {
	it('resolves the $lib alias under Vitest', () => {
		expect(formatPed(1.5)).toBe('1.50');
	});

	it('pins the timezone to UTC for deterministic date formatting', () => {
		expect(process.env.TZ).toBe('UTC');
		// 2026-03-24T00:00:00Z must render as Mar 24 (would shift a day under a
		// negative-offset local zone if TZ were not pinned).
		const rendered = new Date('2026-03-24T00:00:00Z').toLocaleDateString('en-US', {
			month: 'short',
			day: 'numeric',
		});
		expect(rendered).toBe('Mar 24');
	});
});
