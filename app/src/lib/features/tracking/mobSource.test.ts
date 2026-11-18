import { describe, expect, it } from 'vitest';
import type { DetectedCreature, MobSourceView } from '$lib/api';
import { expandedLines, isSteered, pinShownTarget, primaryAction, standing } from './mobSource';

const atrox: DetectedCreature = { name: 'Old Atrox', species: 'Atrox', maturity: 'Old' };
const daikiba: DetectedCreature = {
	name: 'Mature Daikiba',
	species: 'Daikiba',
	maturity: 'Mature',
};
const gloomrat: DetectedCreature = { name: 'Gloomrat', species: 'Gloomrat', maturity: '' };
const young: DetectedCreature = { name: 'Young Atrox', species: 'Atrox', maturity: 'Young' };

function view(overrides: Partial<MobSourceView> = {}): MobSourceView {
	return {
		readingScreen: true,
		pin: null,
		typed: null,
		reading: null,
		continuity: null,
		...overrides,
	};
}

describe('the one mob the control shows', () => {
	it('stands the pin above everything', () => {
		const pinned = view({
			readingScreen: false,
			pin: { creature: daikiba, by: 'read' },
			typed: young,
		});
		expect(standing(pinned)).toEqual({ kind: 'pinned', creature: daikiba });
		expect(primaryAction(pinned)).toBe('release');
		expect(isSteered(pinned)).toBe(true);
	});

	it('shows the mob read now over recent kills and the backstop, with Pin beside it', () => {
		const reading = view({ reading: atrox, continuity: atrox, typed: young });
		expect(standing(reading)).toEqual({ kind: 'reading', creature: atrox });
		expect(primaryAction(reading)).toBe('pin');
		expect(pinShownTarget(reading)).toEqual({ kind: 'shown', creature: atrox });
	});

	it('falls back to what recent kills suggest, then the backstop', () => {
		const continuity = view({ continuity: gloomrat, typed: young });
		expect(standing(continuity)).toEqual({ kind: 'continuity', creature: gloomrat });
		expect(pinShownTarget(continuity)).toEqual({ kind: 'shown', creature: gloomrat });
		const backstop = view({ typed: young });
		expect(standing(backstop)).toEqual({ kind: 'backstop', creature: young });
		expect(primaryAction(backstop)).toBe('pin');
		// The backstop alone is pinned as typed: nothing was read.
		expect(pinShownTarget(backstop)).toEqual({
			kind: 'typed',
			species: 'Atrox',
			maturity: 'Young',
		});
	});

	it('shows a quiet nothing, with no action, when nothing names a mob', () => {
		const nothing = view();
		expect(standing(nothing)).toEqual({ kind: 'none', creature: null });
		expect(primaryAction(nothing)).toBeNull();
		expect(pinShownTarget(nothing)).toBeNull();
		expect(standing(null)).toEqual({ kind: 'none', creature: null });
	});

	it('is the typed mob alone when the screen is not read and nothing is pinned', () => {
		const off = view({ readingScreen: false, typed: young });
		expect(isSteered(off)).toBe(false);
		expect(standing(off)).toEqual({ kind: 'backstop', creature: young });
		expect(primaryAction(off)).toBeNull();
		expect(isSteered(view({ readingScreen: false }))).toBe(false);
		expect(isSteered(null)).toBe(false);
	});
});

describe('what expanding it reveals', () => {
	it('names each source in plain words, the backstop always', () => {
		const lines = expandedLines(view({ reading: atrox, continuity: atrox, typed: young }));
		expect(lines.map((line) => [line.label, line.creature?.name ?? null])).toEqual([
			['Reading now', 'Old Atrox'],
			['Recent kills', 'Old Atrox'],
			['Your backstop', 'Young Atrox'],
		]);
		// Recent kills say when they are used, and what to do after changing
		// creature: never a grade, count or time.
		expect(lines[1].note).toMatch(/Kills not read take this/);
		expect(lines.map((line) => line.note).join(' ')).not.toMatch(/\d/);
	});

	it('leaves out what names nothing, and keeps an empty backstop to set', () => {
		expect(expandedLines(view()).map((line) => [line.kind, line.creature])).toEqual([
			['backstop', null],
		]);
		const pinned = expandedLines(
			view({ readingScreen: false, pin: { creature: atrox, by: 'typed' }, typed: young }),
		);
		expect(pinned.map((line) => line.kind)).toEqual(['backstop']);
		expect(pinned[0].note).toBeNull();
	});
});
