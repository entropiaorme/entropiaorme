// Writes a per-checkout Tauri config overlay carrying the dev devUrl.
// `tauri dev` is invoked with `--config tauri.dev.conf.json --config
// tauri.dev.local.json` so the two overlays merge over the base via
// Tauri's JSON-merge-patch config-extension mechanism.
//
// devUrl resolves in two modes:
//   * When ENTROPIAORME_HOSTNAME is set, the overlay points at the
//     hostname over HTTPS (e.g. `https://entropiaorme.localhost`).
//     Caddy reverse-proxies that hostname to the picked frontend
//     port and serves a cert from its local CA; see the committed
//     `Caddyfile` and the README's `caddy trust` prereq.
//   * When ENTROPIAORME_HOSTNAME is unset, the overlay falls back to
//     `http://localhost:<port>` driven by ENTROPIAORME_FRONTEND_PORT
//     (default 5173). Contributors who skip the Caddy install keep a
//     working `just dev` via this fallback (plain HTTP straight to
//     Vite).
//
// The indirection exists because Tauri 2 does not support `${env:VAR}`
// interpolation inside tauri.conf.json field values, and the only
// dev-URL-related environment variable it reads (TAURI_DEV_HOST) targets
// mobile public-network development rather than overriding devUrl. The
// generated overlay is the smallest portable shim that keeps the
// env-driven devUrl honoured by Tauri's webview-loading side without
// hardcoding values in committed config.
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const hostname = (process.env.ENTROPIAORME_HOSTNAME ?? '').trim();
let devUrl;
if (hostname) {
	devUrl = `https://${hostname}`;
} else {
	const rawPort = (process.env.ENTROPIAORME_FRONTEND_PORT ?? '5173').trim();
	const port = Number(rawPort);
	if (!Number.isInteger(port) || port < 1 || port > 65535) {
		throw new Error('ENTROPIAORME_FRONTEND_PORT must be an integer between 1 and 65535');
	}
	devUrl = `http://localhost:${port}`;
}
const overlay = { build: { devUrl } };
const out = join(dirname(fileURLToPath(import.meta.url)), 'tauri.dev.local.json');
writeFileSync(out, `${JSON.stringify(overlay, null, 2)}\n`);
