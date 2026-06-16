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
				const isChecked = () =>
					browser.execute(() => !!document.querySelector('input[type="checkbox"]:checked'));
				const accept = await browser.$('input[type="checkbox"]');
				if (await accept.isExisting()) {
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
				const start = await browser.$('button*=Get started');
				await start.click();
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
