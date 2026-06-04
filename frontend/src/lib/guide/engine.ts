import { animate } from 'motion';
import { setPreference } from '$lib/preferences';
import { guideState, getDemoApi } from './state.svelte';
import type { CursorAPI, GuideStep, GuideSurface, PlayCtx } from './types';

/** Cursor element handles registered by GuideOverlay on mount. */
let cursorEl: HTMLElement | null = null;
let cursorRippleEl: HTMLElement | null = null;

export function setCursorElement(el: HTMLElement | null, ripple: HTMLElement | null): void {
	cursorEl = el;
	cursorRippleEl = ripple;
}

const cursorAPI: CursorAPI = {
	async moveTo(target, opts = {}) {
		if (!cursorEl) return;
		const rect = target instanceof HTMLElement ? target.getBoundingClientRect() : target;
		const offsetX = opts.offset?.x ?? rect.width / 2;
		const offsetY = opts.offset?.y ?? rect.height / 2;
		const targetX = rect.left + offsetX;
		const targetY = rect.top + offsetY;
		const durationSec = (opts.duration ?? 700) / 1000;
		// Instant-snap path (typically `duration: 0` between animation loops). Motion 12 uses
		// WAAPI under the hood, where a zero-duration animate() does NOT reliably commit the
		// final value; subsequent animate() calls then tween from the prior position rather
		// than the requested snap target. Bypass Motion for these snaps by writing the inline
		// transform directly and cancelling any in-flight WAAPI animation holding the cursor
		// at its prior position (Motion sets fill: forwards by default). The next animate()
		// call reads the freshly-written inline transform as its starting state.
		if (durationSec === 0) {
			cursorEl.getAnimations().forEach((a) => a.cancel());
			cursorEl.style.transform = `translate(${targetX}px, ${targetY}px)`;
			return;
		}
		// Explicit `[from, to]` keyframes when caller supplies `from`. The snap path
		// above writes inline transform directly, which doesn't update Motion's cached
		// x/y. Without explicit keyframes, Motion's next animate() reads its cache as
		// the start state, which on loop iter 2+ is the previous target (e.g. the
		// clicked button), so the cursor pops to the target instead of sliding from
		// the snap origin. Explicit keyframes override the cache.
		const keyframes = opts.from
			? { x: [opts.from.x, targetX], y: [opts.from.y, targetY] }
			: { x: targetX, y: targetY };
		await animate(cursorEl, keyframes, { duration: durationSec, ease: [0.4, 0.0, 0.2, 1] });
	},
	async clickRipple() {
		if (!cursorRippleEl) return;
		await animate(
			cursorRippleEl,
			{ scale: [0, 1.6], opacity: [0.7, 0] },
			{ duration: 0.45, ease: 'easeOut' },
		);
	},
	setState(state) {
		if (cursorEl) cursorEl.dataset.state = state;
	},
	show() {
		guideState.cursorVisible = true;
	},
	hide() {
		guideState.cursorVisible = false;
	},
};

/** Resolve an anchor target with retry; elements may only mount after a prior beat. */
async function resolveTarget(
	target: HTMLElement | (() => HTMLElement | null),
): Promise<HTMLElement | null> {
	if (target instanceof HTMLElement) return target;
	for (let i = 0; i < 40; i++) {
		const el = target();
		if (el) return el;
		await new Promise((r) => setTimeout(r, 50));
	}
	return null;
}

function makePlayCtx(surfaceId: string): PlayCtx {
	const demoApi = getDemoApi(surfaceId);
	return {
		cursor: cursorAPI,
		demoApi,
		async click(target) {
			const el = await resolveTarget(target);
			if (!el) return;
			cursorAPI.show();
			await cursorAPI.moveTo(el);
			await cursorAPI.clickRipple();
			el.click();
			cursorAPI.hide();
		},
		async hover(target) {
			const el = await resolveTarget(target);
			if (!el) return;
			cursorAPI.show();
			await cursorAPI.moveTo(el);
			cursorAPI.setState('hover');
			const rect = el.getBoundingClientRect();
			const init = {
				bubbles: true,
				clientX: rect.left + rect.width / 2,
				clientY: rect.top + rect.height / 2,
			};
			el.dispatchEvent(new MouseEvent('mouseenter', init));
			el.dispatchEvent(new MouseEvent('mousemove', init));
		},
		wait(ms) {
			return new Promise((r) => setTimeout(r, ms));
		},
	};
}

function currentStep(): GuideStep | null {
	if (!guideState.currentSurface) return null;
	return guideState.currentSurface.steps[guideState.currentStepIndex] ?? null;
}

async function playCurrentStep(): Promise<void> {
	const step = currentStep();
	if (!step || !guideState.currentSurface) return;
	guideState.isPlaying = true;
	// Cursor is an action indicator: hidden at step entry, shown by click()/hover() helpers
	// (or explicit cursor.show()) only when the step demonstrates an active interaction.
	guideState.cursorVisible = false;
	cursorAPI.setState('idle');
	const ctx = makePlayCtx(guideState.currentSurface.id);
	try {
		if (step.play) await step.play(ctx);
	} catch (e) {
		console.warn('[guide] step play() failed:', e);
	} finally {
		guideState.isPlaying = false;
		cursorAPI.setState('idle');
	}
}

export async function openGuide(surface: GuideSurface): Promise<void> {
	if (guideState.isActive) return;
	guideState.currentSurface = surface;
	guideState.currentStepIndex = 0;
	guideState.isActive = true;
	void setPreference(`guide_seen_${surface.id}`, true);
	// Let the overlay mount + the surface's $effect swap in demo data.
	await new Promise((r) => setTimeout(r, 100));
	if (surface.beforeStart) {
		await surface.beforeStart(getDemoApi(surface.id));
		// Beat for any normalisation state changes to settle.
		await new Promise((r) => setTimeout(r, 80));
	}
	await playCurrentStep();
}

export async function nextStep(): Promise<void> {
	if (!guideState.currentSurface) return;
	const step = currentStep();
	if (step?.resetDemo) step.resetDemo();
	guideState.currentStepIndex += 1;
	if (guideState.currentStepIndex >= guideState.currentSurface.steps.length) {
		closeGuide();
		return;
	}
	// Give the surface a beat to render any state reset.
	await new Promise((r) => setTimeout(r, 80));
	await playCurrentStep();
}

export async function previousStep(): Promise<void> {
	if (!guideState.currentSurface) return;
	if (guideState.currentStepIndex <= 0) return;
	const step = currentStep();
	if (step?.resetDemo) step.resetDemo();
	guideState.currentStepIndex -= 1;
	// Give the surface a beat to render any state reset.
	await new Promise((r) => setTimeout(r, 80));
	await playCurrentStep();
}

export async function replayCurrentStep(): Promise<void> {
	const step = currentStep();
	if (step?.resetDemo) step.resetDemo();
	await new Promise((r) => setTimeout(r, 80));
	await playCurrentStep();
}

export function closeGuide(): void {
	const step = currentStep();
	if (step?.resetDemo) step.resetDemo();
	guideState.isActive = false;
	guideState.currentSurface = null;
	guideState.currentStepIndex = 0;
	guideState.isPlaying = false;
}
