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

export async function ensureDashboard(browser, devUrl) {
	// Pin a large, fixed window before anything renders. Two reasons: (1) the
	// dashboard's flex-height layout needs a definite viewport height or its
	// `flex-1 min-h-0` panels (e.g. the loot-composition list) collapse to zero
	// height; (2) a fixed size makes every visual baseline deterministic instead
	// of varying with whatever size the shell happened to launch at.
	try {
		await browser.setWindowSize(1600, 1000);
	} catch {
		// Some driver/shell combinations reject Set Window Rect; fall back to the
		// launch size rather than failing the whole suite.
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
