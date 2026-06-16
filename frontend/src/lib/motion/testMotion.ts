import { type Tweened, tweened } from 'svelte/motion';

// The options type `tweened` accepts is not exported by name; derive it from the
// function's own signature so the wrapper stays exactly in step with it.
type TweenOpts<T> = Parameters<typeof tweened<T>>[1];

// A `tweened` wrapper that collapses to an instant settle (duration 0) when
// motion should be suppressed. Two independent reasons suppress it:
//
//   1. The user prefers reduced motion. The charts' "breathing" y-axis rescale
//      is exactly the kind of non-essential, decorative motion WCAG 2.3.3 asks
//      us to drop; honouring the preference here is a genuine accessibility
//      improvement, not merely a test affordance.
//   2. The build-time e2e flag is set, so the visual-regression suite captures
//      each chart's settled end-state deterministically rather than racing a
//      600ms rescale mid-flight.
//
// Why a wrapper at all: `svelte/motion` drives its values in JavaScript (via
// requestAnimationFrame), so neither the `prefers-reduced-motion` CSS block in
// app.css nor the e2e's `disableCSSAnimations` can reach these tweens. This is
// the only hook that can.
//
// The flag is a build-time `define` substitution (see vite.config.ts), so a
// normal production build (where the env var is unset) folds the comparison to
// a static `false` and the flag branch tree-shakes out: the shipped bundle is
// identical to a direct `tweened` call for users who do not prefer reduced
// motion. The reduced-motion branch is a real, shipped runtime feature.

const E2E_FREEZE = import.meta.env.E2E_FREEZE_TWEENS === '1';

function prefersReducedMotion(): boolean {
	return (
		typeof window !== 'undefined' &&
		typeof window.matchMedia === 'function' &&
		window.matchMedia('(prefers-reduced-motion: reduce)').matches
	);
}

/** Whether motion should settle instantly (e2e capture or reduced-motion preference). */
export function shouldSettleInstantly(): boolean {
	return E2E_FREEZE || prefersReducedMotion();
}

/**
 * Drop-in replacement for `svelte/motion`'s `tweened` that settles instantly
 * (duration 0) when {@link shouldSettleInstantly} holds, and animates normally
 * otherwise. The decision is made per-construction (matchMedia is read once,
 * mirroring how the tweens are constructed once at component init).
 */
export function settleTween<T>(value: T, opts: TweenOpts<T>): Tweened<T> {
	return tweened(value, shouldSettleInstantly() ? { ...opts, duration: 0 } : opts);
}
