import { flushSync } from 'svelte';
import { describe, expect, it } from 'vitest';
import type { ToolSourceView } from '$lib/api';
import { createHeldToolModel } from './heldToolModel.svelte';

const reading: ToolSourceView = {
	readingScreen: true,
	notReading: null,
	manual: false,
	held: { kind: 'nothing' },
};

describe('createHeldToolModel', () => {
	it('makes each panel available only in its own state', () => {
		let source = $state<ToolSourceView | null>(reading);
		const cleanup = $effect.root(() => {
			const model = createHeldToolModel({ source: () => source });
			model.screen.toggle();
			model.unread.toggle();
			flushSync();
			expect(model.screen.expanded).toBe(true);
			expect(model.unread.expanded).toBe(false);
			// The screen stops being read: the screen panel closes for good.
			source = { ...reading, readingScreen: false, notReading: 'stopped' };
			flushSync();
			expect(model.screen.expanded).toBe(false);
			model.unread.toggle();
			flushSync();
			expect(model.unread.expanded).toBe(true);
			source = reading;
			flushSync();
			expect(model.screen.expanded).toBe(false);
			expect(model.unread.expanded).toBe(false);
			model.destroy();
		});
		cleanup();
	});

	it('closes both when the overlay hides', () => {
		const cleanup = $effect.root(() => {
			const model = createHeldToolModel({ source: () => reading });
			model.screen.toggle();
			flushSync();
			model.close();
			flushSync();
			expect(model.screen.expanded).toBe(false);
			model.destroy();
		});
		cleanup();
	});
});
