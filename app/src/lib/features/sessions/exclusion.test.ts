import { describe, expect, it } from 'vitest';
import type { SessionExclusion } from '$lib/types/tracking';
import {
	countsEconomy,
	countsProgression,
	excludedFrom,
	excludedSince,
	exclusionMark,
	INCLUDED,
	toggled,
	withNote,
} from './exclusion';

const economyOnly: SessionExclusion = {
	economyExcludedAt: '2026-10-22T10:00:00+00:00',
	progressionExcludedAt: null,
	note: 'hotbar misread',
};
const both: SessionExclusion = {
	economyExcludedAt: '2026-10-22T10:00:00+00:00',
	progressionExcludedAt: '2026-10-20T08:00:00+00:00',
	note: null,
};

describe('what a session counts towards', () => {
	it('counts everywhere with no exclusion', () => {
		expect(countsEconomy(null)).toBe(true);
		expect(countsProgression(null)).toBe(true);
		expect(excludedFrom(null)).toBeNull();
		expect(exclusionMark(null)).toBeNull();
		expect(excludedSince(null)).toBeNull();
	});

	it('reads each scope on its own', () => {
		expect(countsEconomy(economyOnly)).toBe(false);
		expect(countsProgression(economyOnly)).toBe(true);
		expect(excludedFrom(economyOnly)).toBe('PED stats');
		expect(exclusionMark(economyOnly)).toBe('Not in PED stats');
		expect(
			excludedFrom({ ...economyOnly, economyExcludedAt: null, progressionExcludedAt: 'x' }),
		).toBe('skill stats');
		expect(excludedFrom(both)).toBe('PED and skill stats');
		expect(exclusionMark(both)).toBe('Not in stats');
	});

	it('dates the exclusion from the earliest stamp in force', () => {
		expect(excludedSince(economyOnly)).toBe(economyOnly.economyExcludedAt);
		expect(excludedSince(both)).toBe(both.progressionExcludedAt);
	});
});

describe('the writes the controls send', () => {
	it('flips one scope and keeps the other and the note', () => {
		expect(toggled(null, 'economy')).toEqual({ economy: true, progression: false, note: null });
		expect(toggled(economyOnly, 'progression')).toEqual({
			economy: true,
			progression: true,
			note: 'hotbar misread',
		});
		expect(toggled(economyOnly, 'economy')).toEqual({
			economy: false,
			progression: false,
			note: 'hotbar misread',
		});
	});

	it('replaces the note, a blank one clearing it', () => {
		expect(withNote(economyOnly, '  chat log stalled  ')).toEqual({
			economy: true,
			progression: false,
			note: 'chat log stalled',
		});
		expect(withNote(economyOnly, '   ').note).toBeNull();
	});

	it('includes the session everywhere again', () => {
		expect(INCLUDED).toEqual({ economy: false, progression: false, note: null });
	});
});
