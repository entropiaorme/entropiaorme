// @vitest-environment happy-dom

import { cubicOut } from 'svelte/easing';
import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { settleTween, shouldSettleInstantly } from './testMotion';

// Drive `prefers-reduced-motion` deterministically: matchMedia is the only
// signal `shouldSettleInstantly` reads at runtime (the build-time e2e flag is
// unset under Vitest), so stubbing it controls the whole decision.
function setReducedMotion(matches: boolean): void {
	vi.stubGlobal(
		'matchMedia',
		(query: string): MediaQueryList =>
			({
				matches: query.includes('prefers-reduced-motion') ? matches : false,
				media: query,
				onchange: null,
				addEventListener: () => {},
				removeEventListener: () => {},
				addListener: () => {},
				removeListener: () => {},
				dispatchEvent: () => false,
			}) as unknown as MediaQueryList,
	);
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('shouldSettleInstantly', () => {
	it('is true when the user prefers reduced motion', () => {
		setReducedMotion(true);
		expect(shouldSettleInstantly()).toBe(true);
	});

	it('is false when the user does not prefer reduced motion', () => {
		setReducedMotion(false);
		expect(shouldSettleInstantly()).toBe(false);
	});
});

describe('settleTween', () => {
	it('exposes the seed value before any set', () => {
		setReducedMotion(false);
		const t = settleTween(7, { duration: 600, easing: cubicOut });
		expect(get(t)).toBe(7);
	});

	it('settles instantly to the target when reduced motion is preferred', async () => {
		setReducedMotion(true);
		const t = settleTween(0, { duration: 600, easing: cubicOut });
		// With the freeze applied the tween is constructed with duration 0, so the
		// set promise resolves on the next tick with the value already at target,
		// rather than animating across 600ms.
		await t.set(100);
		expect(get(t)).toBe(100);
	});
});
