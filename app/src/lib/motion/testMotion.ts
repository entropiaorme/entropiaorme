import { type Tweened, tweened } from 'svelte/motion';

// The options type `tweened` accepts is not exported by name; derive it from the
// function's own signature so the wrapper stays exactly in step with it.
type TweenOpts<T> = Parameters<typeof tweened<T>>[1];

// A `tweened` wrapper that collapses to an instant settle (duration 0) when the
// user prefers reduced motion. The charts' "breathing" y-axis rescale is exactly
// the kind of non-essential, decorative motion WCAG 2.3.3 asks us to drop.
//
// Why a wrapper at all: `svelte/motion` drives its values in JavaScript (via
// requestAnimationFrame), so the `prefers-reduced-motion` CSS block in app.css
// cannot reach these tweens. This is the only hook that can.

function prefersReducedMotion(): boolean {
	return (
		typeof window !== 'undefined' &&
		typeof window.matchMedia === 'function' &&
		window.matchMedia('(prefers-reduced-motion: reduce)').matches
	);
}

/** Whether motion should settle instantly (the reduced-motion preference). */
export function shouldSettleInstantly(): boolean {
	return prefersReducedMotion();
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
