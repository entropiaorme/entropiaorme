// Drive the app to the dashboard, completing onboarding when present.
//
// The plugin-store prefs file lives outside the repo tree, so it cannot be
// seeded directly in the test environment; onboarding state is established the
// only portable way: by walking the real onboarding flow through the UI,
// which persists the prefs via the app's own runtime. Idempotent across runs
// and machines: a fresh profile lands on /welcome and gets driven through; an
// already-onboarded profile lands on the dashboard and skips straight past.

const DASH = '[data-guide-anchor="dashboard-area"]';

function probe(browser) {
	return browser.execute((dashSel) => {
		const pb = document.querySelector('[role="progressbar"]');
		return {
			path: location.pathname,
			onDashboard: !!document.querySelector(dashSel),
			welcomeStep: pb ? Number(pb.getAttribute('aria-valuenow')) : -1,
		};
	}, DASH);
}

// Wait for an element to stop moving before interacting with it. Each welcome
// step flies in via a `svelte/transition` (a JS-driven transform, so
// disableCSSAnimations does not reach it): a click dispatched while the target
// is still translating is rejected as "element not interactable" because the
// click point shifts between the driver computing it and dispatching it. The
// progressbar lives outside the keyed step, so its value advances the instant
// the step changes, well before the new step has settled. Poll the element's
// box until two consecutive reads agree.
async function settle(browser, el, timeout = 8000) {
	let prev = null;
	await browser.waitUntil(
		async () => {
			const loc = await el.getLocation();
			const stable = prev !== null && Math.abs(loc.x - prev.x) < 1 && Math.abs(loc.y - prev.y) < 1;
			prev = loc;
			return stable;
		},
		{ timeout, interval: 120, timeoutMsg: 'welcome control never settled (still animating)' },
	);
}

// Wait for an element to reach a STABLE size at or above a minimum height. The
// dashboard's flex-height panels can momentarily collapse during the async
// hydrate/reflow, so a capture taken then is a sliver; polling getSize() until
// the box is stable across two reads AND tall enough makes a collapsed frame
// fail loudly with a clear message rather than being baked into a baseline.
export async function settleSize(browser, el, { timeout = 10000, min = 1 } = {}) {
	let prev = null;
	await browser.waitUntil(
		async () => {
			const s = await el.getSize();
			const stable =
				prev !== null && Math.abs(s.height - prev.height) < 1 && Math.abs(s.width - prev.width) < 1;
			prev = s;
			return stable && s.height >= min;
		},
		{
			timeout,
			interval: 120,
			timeoutMsg: `element never reached a stable height >= ${min}px (collapsed or still reflowing)`,
		},
	);
}

// Await all FINITE in-flight Web Animations (e.g. the stat-grid FLIP), then a
// double rAF so layout has committed, before a screenshot. disableCSSAnimations
// cannot reach the WAAPI-driven FLIP, so this is the only gate that settles it
// in the suite. Infinite animations (a pulse / spinner) are skipped so the
// await can never hang.
export async function animationsFinished(browser) {
	await browser.executeAsync((done) => {
		const finite = document.getAnimations().filter((a) => {
			try {
				return a.effect && a.effect.getComputedTiming().iterations !== Number.POSITIVE_INFINITY;
			} catch {
				return false;
			}
		});
		Promise.all(finite.map((a) => a.finished.catch(() => {}))).then(() =>
			requestAnimationFrame(() => requestAnimationFrame(done)),
		);
	});
}

export async function ensureDashboard(browser, devUrl) {
	// Pin a large, fixed window before anything renders. Two reasons: (1) the
	// dashboard's flex-height layout needs a definite viewport height or its
	// `flex-1 min-h-0` panels (e.g. the loot-composition list) collapse to zero
	// height; (2) a fixed size makes every visual baseline deterministic instead
	// of varying with whatever size the shell happened to launch at.
	// tauri-driver / WebView2 can intermittently reject or under-apply Set
	// Window Rect, leaving the shell at its launch size. Every visual baseline
	// is captured at 1600x1000, so a capture at any other size reflows the whole
	// layout and swings the diff: set it, verify via getWindowSize, and retry
	// rather than silently proceeding at the wrong size. Each attempt is logged
	// (outer + inner viewport) so a residual size problem is visible in the run
	// output. After the retries we proceed regardless (no worse than the prior
	// fall-back), but a successful retry is what removes the size flake.
	const TARGET_W = 1600;
	const TARGET_H = 1000;
	for (let attempt = 0; attempt < 6; attempt += 1) {
		try {
			await browser.setWindowSize(TARGET_W, TARGET_H);
		} catch (e) {
			console.log(`[onboarding] setWindowSize attempt ${attempt} rejected: ${e.message}`);
		}
		let outer = { width: -1, height: -1 };
		try {
			outer = await browser.getWindowSize();
		} catch {
			// getWindowSize unsupported on some drivers; the inner viewport read
			// below still gates the layout.
		}
		const inner = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
		console.log(
			`[onboarding] window attempt ${attempt}: outer=${outer.width}x${outer.height} inner=${inner.w}x${inner.h}`,
		);
		// getWindowSize reads back the outer size we set; accept once it is close
		// to target (a gross mismatch means the resize did not take).
		if (Math.abs(outer.width - TARGET_W) <= 32 && Math.abs(outer.height - TARGET_H) <= 32) {
			break;
		}
		await browser.pause(250);
	}

	// The debug shell launches at about:blank (the dev URL is injected by the
	// `tauri dev` CLI, absent when tauri-driver launches the binary directly),
	// so navigate the real webview to the dev origin ourselves.
	await browser.url(devUrl);

	// Let the app settle: the layout's onMount runs an async init and only THEN
	// redirects to /welcome (fresh profile) or stays on the dashboard. Waiting
	// for a stable state avoids racing that redirect.
	await browser.waitUntil(
		async () => {
			const s = await probe(browser);
			return s.path.startsWith('/welcome') ? s.welcomeStep > 0 : s.onDashboard;
		},
		{ timeout: 45000, timeoutMsg: 'app never settled into welcome or dashboard' },
	);

	if (await browser.execute(() => location.pathname.startsWith('/welcome'))) {
		// Six-step flow: advance to the Terms step (6), accept, then "Get started"
		// persists the prefs and routes to the dashboard. Step-aware off the
		// progressbar rather than blind clicks.
		// Every interaction waits for the control to be CLICKABLE (not merely
		// present) before clicking, and each step gates on the progressbar
		// actually advancing rather than a fixed pause: a click that lands mid
		// fly/fade transition is rejected as "element not interactable" or
		// silently no-ops, which is the flake that left the app half-onboarded
		// when the visual shots were taken.
		for (let guard = 0; guard < 12; guard += 1) {
			const s = await probe(browser);
			console.log(`[onboarding] step=${s.welcomeStep} path=${s.path}`);
			if (s.welcomeStep < 0 || !s.path.startsWith('/welcome')) break;
			if (s.welcomeStep >= 6) {
				// The primary button is disabled until the Terms checkbox is accepted
				// (`canAdvance = step !== totalSteps || tosAccepted`), so accept first,
				// then click through. A WebDriver click on the checkbox fires the
				// change event the Svelte binding listens to (a raw DOM click does
				// not reliably); the surrounding label is the fallback. Confirm the
				// box registered as checked before clicking the now-enabled button.
				//
				// Scope every selector to the Terms label: the prior step (News
				// opt-in) also renders an `input[type="checkbox"]`, and during the
				// step transition the out-fading prior step and the in-flying Terms
				// step briefly coexist in the DOM, so an unscoped selector can toggle
				// (and falsely read as accepted) the wrong box. Wait for the Terms
				// step to settle before interacting, so the click never lands mid
				// fly-in.
				const isChecked = () =>
					browser.execute(
						() => !!document.querySelector('label.accept input[type="checkbox"]:checked'),
					);
				const accept = await browser.$('label.accept input[type="checkbox"]');
				await accept.waitForExist({ timeout: 10000 });
				await settle(browser, accept);
				await accept.waitForClickable({ timeout: 10000 });
				if (!(await isChecked())) {
					try {
						await accept.click();
					} catch {
						const label = await browser.$('label.accept');
						if (await label.isExisting()) await label.click();
					}
				}
				await browser.waitUntil(isChecked, {
					timeout: 5000,
					timeoutMsg: 'Terms checkbox never registered as accepted',
				});
				// The button enables reactively once `tosAccepted` flips;
				// waitForClickable covers enabled + visible + not-obscured. Confirm
				// the click actually left /welcome, retrying once if a stray
				// transition swallowed it.
				const start = await browser.$('button*=Get started');
				await start.waitForClickable({ timeout: 10000 });
				for (let attempt = 0; attempt < 2; attempt += 1) {
					await start.click();
					try {
						await browser.waitUntil(
							async () => !(await browser.execute(() => location.pathname.startsWith('/welcome'))),
							{ timeout: 4000 },
						);
						break;
					} catch {
						if (attempt === 1) throw new Error('"Get started" never advanced past /welcome');
					}
				}
				break;
			}
			const cont = await browser.$('button*=Continue');
			let clicked = false;
			if (await cont.isExisting()) {
				try {
					await cont.waitForClickable({ timeout: 3000 });
					await cont.click();
					clicked = true;
				} catch {
					// Fall through to the keyboard path below.
				}
			}
			if (!clicked) await browser.keys(['ArrowRight']); // keyboard fallback (handleKey -> next())
			// Gate the next iteration on the step actually changing, so a missed
			// click is retried rather than racing ahead into a wrong state.
			await browser.waitUntil(
				async () => {
					const n = await probe(browser);
					return (
						!n.path.startsWith('/welcome') || n.welcomeStep < 0 || n.welcomeStep > s.welcomeStep
					);
				},
				{ timeout: 6000, timeoutMsg: `onboarding step ${s.welcomeStep} never advanced` },
			);
		}
	}

	await browser.waitUntil(
		async () => {
			const s = await probe(browser);
			return s.path === '/' && s.onDashboard;
		},
		{ timeout: 25000, timeoutMsg: 'never reached the dashboard after onboarding' },
	);
	await browser.$(DASH).waitForExist({ timeout: 10000 });
}
