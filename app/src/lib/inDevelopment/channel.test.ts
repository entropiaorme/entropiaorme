import { describe, expect, it } from 'vitest';
import { isInDevelopmentVisible } from './channel';

describe('in-development channel', () => {
	it('hides these surfaces in a published build', () => {
		// Protects someone who downloaded the app rather than building it: a
		// published artefact offers nothing unfinished.
		expect(isInDevelopmentVisible(true)).toBe(false);
	});

	it('shows them in any build not stamped as published', () => {
		// A locally built installer, a source build, and the dev server all land
		// here. These are the builds used while such a surface is still being
		// built, so they must show it rather than hide it.
		expect(isInDevelopmentVisible(false)).toBe(true);
	});
});
