// Deterministic stub backend for the native-shell e2e suite.
//
// WebDriver (tauri-driver) has no network-interception primitive like
// Playwright's page.route, so the e2e exercises the real shell against a
// fixed, hermetic backend served here rather than the live Python/Rust
// backend. This keeps the suite reproducible and offline: each surface's reads
// return pinned fixture data, and the SSE stream stays open but silent (the
// relay connects; no live frames perturb the rendered state).
//
// Routing is an explicit method+path table (query strings stripped before
// matching) rather than a silent catch-all, so a surface whose endpoint is not
// modelled is reported loudly instead of being handed an empty array and baking
// an empty render into a visual baseline. Any genuinely list-shaped incidental
// read (news, archive, ...) still falls through to a forgiving `[]`, but the
// fall-through is logged so a MISSING fixture is visible in the run output.
//
// This is test scaffolding only; it never ships.

import { readFileSync } from 'node:fs';
import http from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));

function loadFixture(name) {
	return JSON.parse(readFileSync(join(HERE, 'fixtures', name), 'utf8'));
}

const dashboard = loadFixture('dashboard.json');
const analytics = loadFixture('analytics.json');

const PORT = Number(process.env.STUB_PORT || 8424);
const HOST = process.env.STUB_HOST || '127.0.0.1';

// Explicit route table: `${METHOD} ${path}` -> pinned payload. Adding a surface
// is a one-line entry here plus its fixture; anything not listed is logged on
// fall-through so a forgotten fixture surfaces immediately.
const ROUTES = {
	'GET /api/tracking/snapshot': dashboard.snapshot,
	'GET /api/tracking/session/e2e-session': dashboard.sessionDetail,
	'GET /api/quests': dashboard.quests,
	'GET /api/quests/playlists': dashboard.playlists,
	'GET /api/analytics/overview': analytics.overview,
	'GET /api/analytics/activity': analytics.activity,
	'GET /api/analytics/ledger': analytics.ledger,
	'GET /api/analytics/ledger/presets': analytics.presets,
	'GET /api/analytics/inventory': analytics.inventory,
	'GET /api/tracking/sessions': analytics.sessions,
};

function cors(res) {
	res.setHeader('Access-Control-Allow-Origin', '*');
	res.setHeader('Access-Control-Allow-Methods', 'GET,POST,PUT,DELETE,PATCH,OPTIONS');
	res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
}

function json(res, body, status = 200) {
	cors(res);
	res.writeHead(status, { 'Content-Type': 'application/json' });
	res.end(JSON.stringify(body));
}

const server = http.createServer((req, res) => {
	const method = req.method || 'GET';
	const path = (req.url || '').split('?')[0];

	if (method === 'OPTIONS') {
		cors(res);
		res.writeHead(204);
		res.end();
		return;
	}

	// Keep the event stream open but silent: the relay's EventSource connects
	// and each surface hydrates from its snapshot read; no frames arrive to
	// shift the deterministic render.
	if (path === '/api/events') {
		cors(res);
		res.writeHead(200, {
			'Content-Type': 'text/event-stream',
			'Cache-Control': 'no-cache',
			Connection: 'keep-alive',
		});
		res.write(': connected\n\n');
		return; // intentionally left open
	}

	const key = `${method} ${path}`;
	const payload = ROUTES[key];
	if (payload !== undefined) return json(res, payload);

	// Forgiving fall-through: an empty array satisfies the list-shaped reads the
	// app's layer issues incidentally (news, archive) without a 500 reaching the
	// UI. Logged loudly so a missing fixture for a surface under test is visible
	// rather than silently baked into a baseline as an empty render.
	// eslint-disable-next-line no-console
	console.warn(`[stub-backend] UNMATCHED ${key} -> []`);
	return json(res, []);
});

server.listen(PORT, HOST, () => {
	// eslint-disable-next-line no-console
	console.log(`[stub-backend] listening on http://${HOST}:${PORT}`);
});
