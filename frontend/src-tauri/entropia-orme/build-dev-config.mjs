// Writes a per-checkout Tauri config overlay carrying the dev devUrl.
// `tauri dev` is invoked with `--config tauri.dev.conf.json --config
// tauri.dev.local.json` so the two overlays merge over the base via
// Tauri's JSON-merge-patch config-extension mechanism.
//
// devUrl resolves in two modes:
//   * When ENTROPIAORME_HOSTNAME is set AND the hostname both resolves
//     through the OS resolver and has a reachable Caddy admin endpoint,
//     the overlay points at the hostname over HTTPS (e.g.
//     `https://entropiaorme.localhost`). Caddy reverse-proxies that
//     hostname to the picked frontend port and serves a cert from its
//     local CA; see the committed `Caddyfile` and the README's
//     `caddy trust` prereq.
//   * Otherwise (hostname unset, or set but a prerequisite is absent) the
//     overlay falls back to `http://localhost:<port>` driven by
//     ENTROPIAORME_FRONTEND_PORT (default 5173). Contributors who skip the
//     optional hostname setup, or who have not completed the one-time
//     OS-resolver step, keep a working `just dev` via this fallback (plain
//     HTTP straight to Vite). When the hostname is set but a prerequisite
//     is missing, a one-line reason is printed so the degrade is visible
//     rather than silent.
//
// The hostname-set path preflights its two prerequisites here, in the
// build step, because tauri-cli resolves the devUrl with getaddrinfo and
// panics ("No such host is known") if the name does not resolve through
// the OS resolver. The preflight converts that hard panic into the working
// plain-HTTP fallback. Both probes target the OS resolver chain and IPv4
// loopback deliberately:
//   * dns.promises.lookup honours NRPT / hosts / the OS resolver order,
//     matching what tauri-cli's getaddrinfo sees; dns.resolve* would
//     bypass the OS resolver and query nameservers directly, so it would
//     not reflect reality.
//   * the Caddy admin probe dials 127.0.0.1 rather than localhost: on a
//     machine where the `.localhost` namespace is routed to the local
//     resolver, bare `localhost` can resolve `::1`-first, and Caddy's admin
//     endpoint binds IPv4 only, so a `localhost` probe would false-negative
//     against a healthy Caddy.
//
// The indirection exists because Tauri 2 does not support `${env:VAR}`
// interpolation inside tauri.conf.json field values, and the only
// dev-URL-related environment variable it reads (TAURI_DEV_HOST) targets
// mobile public-network development rather than overriding devUrl. The
// generated overlay is the smallest portable shim that keeps the env-driven
// devUrl honoured by Tauri's webview-loading side without hardcoding values
// in committed config.
import { lookup } from 'node:dns/promises';
import { writeFileSync } from 'node:fs';
import { request } from 'node:http';
import { dirname, join } from 'node:path';
import { argv, env, exit } from 'node:process';
import { fileURLToPath, pathToFileURL } from 'node:url';

// Caddy's admin endpoint binds IPv4 loopback; probe it there, never via
// `localhost` (see the header note on the `::1`-first hazard).
const CADDY_ADMIN_HOST = '127.0.0.1';
const CADDY_ADMIN_PROBE_TIMEOUT_MS = 1500;

// Pure decision: given the hostname, the validated fallback port, and the
// two preflight results, return the devUrl to emit and an optional one-line
// warning explaining any degrade. Side-effect-free so the whole fallback
// matrix is unit-testable without touching DNS, sockets, or the filesystem.
export function resolveDevUrl({ hostname, port, hostnameResolves, caddyAdminReachable }) {
	const fallbackUrl = `http://localhost:${port}`;
	if (!hostname) {
		return { devUrl: fallbackUrl, warning: null };
	}
	if (!hostnameResolves) {
		return {
			devUrl: fallbackUrl,
			warning: `Hostname ${hostname} did not resolve through the OS resolver; serving the dev stack on ${fallbackUrl} instead. Complete the one-time .localhost resolver step in the README's "Optional dev environment" section to use the HTTPS hostname.`,
		};
	}
	if (!caddyAdminReachable) {
		return {
			devUrl: fallbackUrl,
			warning: `Caddy admin endpoint on ${CADDY_ADMIN_HOST}:2019 was unreachable; serving the dev stack on ${fallbackUrl} instead. Start Caddy with "just proxy-up" to use https://${hostname}.`,
		};
	}
	return { devUrl: `https://${hostname}`, warning: null };
}

// Read and validate ENTROPIAORME_FRONTEND_PORT. Always validated because
// the fallback URL needs it on every degrade path, not only when the
// hostname is unset.
export function readFrontendPort() {
	const rawPort = (env.ENTROPIAORME_FRONTEND_PORT ?? '5173').trim();
	const port = Number(rawPort);
	if (!Number.isInteger(port) || port < 1 || port > 65535) {
		throw new Error('ENTROPIAORME_FRONTEND_PORT must be an integer between 1 and 65535');
	}
	return port;
}

// Does the hostname resolve through the OS resolver chain? Mirrors
// tauri-cli's getaddrinfo so the preflight reflects what the launch will
// actually see.
async function hostnameResolvesViaOs(hostname) {
	try {
		await lookup(hostname);
		return true;
	} catch {
		return false;
	}
}

// Is Caddy's admin endpoint reachable on IPv4 loopback? Any HTTP response
// counts as reachable; a connection error or timeout counts as down. The
// admin port is overridable only to make the probe testable against a stub
// or a known-closed port; production always uses 2019.
function probeCaddyAdmin() {
	const port = Number((env.ENTROPIAORME_CADDY_ADMIN_PORT ?? '2019').trim()) || 2019;
	return new Promise((resolve) => {
		const req = request(
			{
				host: CADDY_ADMIN_HOST,
				port,
				path: '/config/',
				method: 'GET',
				timeout: CADDY_ADMIN_PROBE_TIMEOUT_MS,
			},
			(res) => {
				res.resume();
				resolve(true);
			},
		);
		req.on('timeout', () => {
			req.destroy();
			resolve(false);
		});
		req.on('error', () => resolve(false));
		req.end();
	});
}

async function main() {
	const hostname = (env.ENTROPIAORME_HOSTNAME ?? '').trim();
	const port = readFrontendPort();

	let hostnameResolves = false;
	let caddyAdminReachable = false;
	if (hostname) {
		hostnameResolves = await hostnameResolvesViaOs(hostname);
		// Only probe Caddy when the hostname resolved; an unresolved name
		// already forces the fallback, so the admin probe would be wasted.
		caddyAdminReachable = hostnameResolves ? await probeCaddyAdmin() : false;
	}

	const { devUrl, warning } = resolveDevUrl({
		hostname,
		port,
		hostnameResolves,
		caddyAdminReachable,
	});
	if (warning) {
		console.warn(warning);
	}

	const overlay = { build: { devUrl } };
	const out =
		(env.ENTROPIAORME_DEVCONFIG_OUT ?? '').trim() ||
		join(dirname(fileURLToPath(import.meta.url)), 'tauri.dev.local.json');
	writeFileSync(out, `${JSON.stringify(overlay, null, 2)}\n`);
}

// Run only when executed directly (`node build-dev-config.mjs`), not when
// imported by the unit tests, which exercise resolveDevUrl in isolation.
if (argv[1] && import.meta.url === pathToFileURL(argv[1]).href) {
	main().catch((err) => {
		console.error(err.message);
		exit(1);
	});
}
