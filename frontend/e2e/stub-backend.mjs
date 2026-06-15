// Deterministic stub backend for the native-shell e2e suite.
//
// WebDriver (tauri-driver) has no network-interception primitive like
// Playwright's page.route, so the e2e exercises the real shell against a
// fixed, hermetic backend served here rather than the live Python/Rust
// backend. This keeps the suite reproducible and offline: the dashboard's
// reads return pinned fixture data, and the SSE stream stays open but silent
// (the relay connects; no live frames perturb the rendered state).
//
// Scope: the dashboard surface (tracking snapshot + quests + playlists) plus
// a forgiving catch-all so no unrelated `/api/*` read 500s the page. This is
// test scaffolding only; it never ships.

import { readFileSync } from 'node:fs';
import http from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const FIX = JSON.parse(readFileSync(join(HERE, 'fixtures', 'dashboard.json'), 'utf8'));

const PORT = Number(process.env.STUB_PORT || 8424);
const HOST = process.env.STUB_HOST || '127.0.0.1';

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
	const path = (req.url || '').split('?')[0];

	if (req.method === 'OPTIONS') {
		cors(res);
		res.writeHead(204);
		res.end();
		return;
	}

	// Keep the event stream open but silent: the relay's EventSource connects
	// and the dashboard hydrates from the snapshot read; no frames arrive to
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

	if (req.method === 'GET' && path === '/api/tracking/snapshot') return json(res, FIX.snapshot);
	if (req.method === 'GET' && path === '/api/quests') return json(res, FIX.quests);
	if (req.method === 'GET' && path === '/api/quests/playlists') return json(res, FIX.playlists);

	// Forgiving catch-all: an empty array satisfies the list-shaped reads the
	// dashboard's layer may issue (news, archive) without a 500 reaching the UI.
	return json(res, []);
});

server.listen(PORT, HOST, () => {
	// eslint-disable-next-line no-console
	console.log(`[stub-backend] listening on http://${HOST}:${PORT}`);
});
