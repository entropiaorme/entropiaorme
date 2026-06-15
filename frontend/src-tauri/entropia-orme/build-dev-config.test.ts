import { execFile } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import { type AddressInfo, createServer as createNetServer } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { env, execPath } from 'node:process';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { afterEach, describe, expect, it } from 'vitest';
import { resolveDevUrl } from './build-dev-config.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const scriptPath = join(here, 'build-dev-config.mjs');
const execFileAsync = promisify(execFile);
// Built from its code point so no literal em dash sits in the source.
const EM_DASH = String.fromCharCode(0x2014);

describe('resolveDevUrl (pure fallback matrix)', () => {
	const ok = { port: 5173, hostnameResolves: true, caddyAdminReachable: true };

	it('hostname unset: plain-HTTP fallback, no warning', () => {
		expect(resolveDevUrl({ ...ok, hostname: '' })).toEqual({
			devUrl: 'http://localhost:5173',
			warning: null,
		});
	});

	it('hostname set, resolves, admin reachable: HTTPS hostname, no warning', () => {
		const r = resolveDevUrl({ ...ok, hostname: 'entropiaorme.localhost' });
		expect(r.devUrl).toBe('https://entropiaorme.localhost');
		expect(r.warning).toBeNull();
	});

	it('hostname set but does not resolve: fallback, reason names resolution', () => {
		const r = resolveDevUrl({ ...ok, hostname: 'entropiaorme.localhost', hostnameResolves: false });
		expect(r.devUrl).toBe('http://localhost:5173');
		expect(r.warning).toMatch(/resolve/i);
		expect(r.warning).toContain('http://localhost:5173');
	});

	it('hostname resolves but Caddy admin unreachable: fallback, reason names Caddy', () => {
		const r = resolveDevUrl({
			...ok,
			hostname: 'entropiaorme.localhost',
			caddyAdminReachable: false,
		});
		expect(r.devUrl).toBe('http://localhost:5173');
		expect(r.warning).toMatch(/Caddy/);
		expect(r.warning).toContain('http://localhost:5173');
	});

	it('the fallback URL honours the frontend port', () => {
		const r = resolveDevUrl({
			...ok,
			hostname: 'h.localhost',
			hostnameResolves: false,
			port: 5199,
		});
		expect(r.devUrl).toBe('http://localhost:5199');
	});

	it('warnings carry no em dash (authoring discipline)', () => {
		const warnings = [
			resolveDevUrl({ ...ok, hostname: 'h.localhost', hostnameResolves: false }).warning,
			resolveDevUrl({ ...ok, hostname: 'h.localhost', caddyAdminReachable: false }).warning,
		];
		for (const w of warnings) {
			expect(w).not.toContain(EM_DASH);
		}
	});
});

// Integration: drive the real script end-to-end (its actual dns.promises
// lookup and 127.0.0.1 admin probe), reading back the overlay it writes.
// The hostname/admin states are made deterministic on any OS:
//   * `.invalid` is reserved by RFC 6761 to never resolve;
//   * `localhost` always resolves, so the admin probe decides the outcome;
//   * ENTROPIAORME_CADDY_ADMIN_PORT redirects the probe at a known-closed
//     port (down) or a local stub (up), so the result never depends on
//     whether a real Caddy happens to be running on this machine.
// The child is run with async execFile rather than a *Sync spawn so the
// parent event loop stays free to answer the stub admin server below.
async function runScript(extraEnv: Record<string, string>) {
	const outDir = mkdtempSync(join(tmpdir(), 'eo-devconfig-'));
	const out = join(outDir, 'tauri.dev.local.json');
	let status = 0;
	let stdout = '';
	let stderr = '';
	try {
		const r = await execFileAsync(execPath, [scriptPath], {
			encoding: 'utf8',
			env: { ...env, ENTROPIAORME_DEVCONFIG_OUT: out, ...extraEnv },
		});
		stdout = r.stdout;
		stderr = r.stderr;
	} catch (err) {
		const e = err as { code?: number; stdout?: string; stderr?: string };
		status = e.code ?? 1;
		stdout = e.stdout ?? '';
		stderr = e.stderr ?? '';
	}
	let overlay: { build: { devUrl: string } } | null = null;
	try {
		overlay = JSON.parse(readFileSync(out, 'utf8'));
	} catch {
		overlay = null;
	}
	rmSync(outDir, { recursive: true, force: true });
	return { status, stdout, stderr, overlay };
}

function freePort(): Promise<number> {
	return new Promise((resolve, reject) => {
		const srv = createNetServer();
		srv.on('error', reject);
		srv.listen(0, '127.0.0.1', () => {
			const { port } = srv.address() as AddressInfo;
			srv.close(() => resolve(port));
		});
	});
}

describe('build-dev-config.mjs (subprocess integration)', () => {
	let admin: Server | null = null;

	afterEach(async () => {
		if (admin) {
			await new Promise<void>((resolve) => admin?.close(() => resolve()));
			admin = null;
		}
	});

	it('hostname does not resolve: writes plain-HTTP overlay and warns', async () => {
		const { status, stdout, stderr, overlay } = await runScript({
			ENTROPIAORME_HOSTNAME: 'eo-nonexistent.invalid',
			ENTROPIAORME_FRONTEND_PORT: '5173',
		});
		expect(status).toBe(0);
		expect(overlay?.build.devUrl).toBe('http://localhost:5173');
		expect(`${stdout}${stderr}`).toMatch(/resolve/i);
	});

	it('hostname resolves but Caddy admin down: writes plain-HTTP overlay and warns', async () => {
		const closed = await freePort();
		const { status, stdout, stderr, overlay } = await runScript({
			ENTROPIAORME_HOSTNAME: 'localhost',
			ENTROPIAORME_FRONTEND_PORT: '5173',
			ENTROPIAORME_CADDY_ADMIN_PORT: String(closed),
		});
		expect(status).toBe(0);
		expect(overlay?.build.devUrl).toBe('http://localhost:5173');
		expect(`${stdout}${stderr}`).toMatch(/Caddy/);
	});

	it('hostname resolves and Caddy admin reachable: writes HTTPS overlay', async () => {
		const port = await freePort();
		admin = createServer((_req, response) => {
			response.writeHead(200, { 'content-type': 'application/json' });
			response.end('{}');
		});
		await new Promise<void>((resolve, reject) => {
			admin?.on('error', reject);
			admin?.listen(port, '127.0.0.1', () => resolve());
		});
		const { status, overlay } = await runScript({
			ENTROPIAORME_HOSTNAME: 'localhost',
			ENTROPIAORME_FRONTEND_PORT: '5173',
			ENTROPIAORME_CADDY_ADMIN_PORT: String(port),
		});
		expect(status).toBe(0);
		expect(overlay?.build.devUrl).toBe('https://localhost');
	});
});
