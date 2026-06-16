// WebdriverIO config for the native-shell e2e suite.
//
// Drives the REAL Tauri shell (WebView2) through tauri-driver, which proxies
// to a version-matched Microsoft Edge WebDriver. The shell is the production
// IPC surface the Phase-8 fetch->invoke collapse will rewrite, so this net is
// that collapse's entry hedge: a browser-only harness cannot see window.__TAURI__.
//
// onPrepare brings up the full hermetic stack and onComplete tears it down:
//   * Vite dev server at the shell's dev origin (the debug shell loads its
//     frontend from the dev URL; the suite navigates the webview there).
//   * the deterministic stub backend (WebDriver has no request interception),
//   * tauri-driver bridging WebdriverIO to the matched msedgedriver.
import { execSync, spawn } from 'node:child_process';
import http from 'node:http';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const E2E_DIR = dirname(fileURLToPath(import.meta.url));
const FRONTEND_DIR = dirname(E2E_DIR);

// Ports: kept off the project anchor pair so a stray app/dev server does not
// collide with the suite. FRONTEND_PORT is the shell's dev origin (the suite
// navigates the webview here); BACKEND_PORT is where the stub backend serves.
const FRONTEND_PORT = process.env.E2E_FRONTEND_PORT || '5173';
const BACKEND_PORT = process.env.E2E_BACKEND_PORT || '8424';
const TAURI_DRIVER_PORT = 4444;

export const DEV_URL = `http://localhost:${FRONTEND_PORT}/`;

const APP_BINARY = join(FRONTEND_DIR, 'src-tauri', 'target', 'debug', 'entropia-orme.exe');
const MSEDGEDRIVER = join(E2E_DIR, '.drivers', 'msedgedriver.exe');
const TAURI_DRIVER =
	process.env.TAURI_DRIVER_PATH || join(homedir(), '.cargo', 'bin', 'tauri-driver.exe');

/** Child processes spawned by onPrepare, torn down in onComplete. */
const procs = [];

function spawnProc(label, command, args, opts = {}) {
	const child = spawn(command, args, {
		stdio: 'ignore',
		shell: false,
		windowsHide: true,
		...opts,
	});
	child.on('error', (err) => console.error(`[e2e:${label}] spawn error:`, err.message));
	procs.push({ label, child });
	return child;
}

function get(url) {
	return new Promise((resolve) => {
		const req = http.get(url, (res) => {
			res.resume();
			resolve(res.statusCode || 0);
		});
		req.on('error', () => resolve(0));
		req.setTimeout(2000, () => {
			req.destroy();
			resolve(0);
		});
	});
}

async function waitForHttp(label, url, { timeoutMs = 90000, ok = (s) => s > 0 && s < 500 } = {}) {
	const deadline = Date.now() + timeoutMs;
	while (Date.now() < deadline) {
		const status = await get(url);
		if (ok(status)) return;
		await sleep(1000);
	}
	throw new Error(`[e2e:${label}] not ready at ${url} within ${timeoutMs}ms`);
}

function killTree(pid) {
	try {
		execSync(`taskkill /F /T /PID ${pid}`, { stdio: 'ignore' });
	} catch {
		// already gone
	}
}

function killImage(image) {
	try {
		execSync(`taskkill /F /IM ${image}`, { stdio: 'ignore' });
	} catch {
		// not running
	}
}

export const config = {
	runner: 'local',
	specs: [join(E2E_DIR, 'specs', '**', '*.e2e.mjs')],
	maxInstances: 1,
	hostname: '127.0.0.1',
	port: TAURI_DRIVER_PORT,
	capabilities: [
		{
			browserName: 'wry',
			'tauri:options': { application: APP_BINARY },
		},
	],
	logLevel: 'error',
	bail: 0,
	waitforTimeout: 15000,
	connectionRetryTimeout: 120000,
	connectionRetryCount: 2,
	framework: 'mocha',
	reporters: ['spec'],
	mochaOpts: { ui: 'bdd', timeout: 180000 },

	// The visual service backs deliverable 2 (dashboard visual-regression):
	// baselines committed in-repo, diffs tolerant to sub-pixel AA noise.
	services: [
		[
			'visual',
			{
				baselineFolder: join(E2E_DIR, 'baselines'),
				screenshotPath: join(E2E_DIR, '.visual-output'),
				formatImageName: '{tag}',
				autoSaveBaseline: true,
				savePerInstance: false,
			},
		],
	],

	onPrepare: async () => {
		// 1. Vite dev server at the shell's dev origin. The frontend bakes the
		//    backend port from the env, so it dials the stub below.
		spawnProc('vite', 'npm', ['run', 'dev'], {
			cwd: FRONTEND_DIR,
			shell: true,
			env: {
				...process.env,
				ENTROPIAORME_FRONTEND_PORT: FRONTEND_PORT,
				ENTROPIAORME_BACKEND_PORT: BACKEND_PORT,
			},
		});
		// 2. Deterministic stub backend (hermetic data; WebDriver cannot mock).
		spawnProc('stub', 'node', [join(E2E_DIR, 'stub-backend.mjs')], {
			cwd: FRONTEND_DIR,
			env: { ...process.env, STUB_PORT: BACKEND_PORT, STUB_HOST: '127.0.0.1' },
		});
		// 3. tauri-driver bridging to the matched msedgedriver.
		spawnProc('tauri-driver', TAURI_DRIVER, ['--native-driver', MSEDGEDRIVER]);

		await Promise.all([
			waitForHttp('vite', `http://localhost:${FRONTEND_PORT}/`),
			waitForHttp('stub', `http://127.0.0.1:${BACKEND_PORT}/api/tracking/snapshot`),
			waitForHttp('tauri-driver', `http://127.0.0.1:${TAURI_DRIVER_PORT}/status`),
		]);
	},

	onComplete: async () => {
		for (const { child } of procs) {
			if (child?.pid) killTree(child.pid);
		}
		// Belt-and-braces: tauri-driver spawns the app + msedgedriver as its own
		// children, which a parent-tree kill can miss if they reparented.
		killImage('entropia-orme.exe');
		killImage('msedgedriver.exe');
		killImage('tauri-driver.exe');
	},
};
