import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The generated client and the legacy request() helper share one error
// contract: any non-2xx throws ApiError(status, message), message preferring
// the FastAPI `detail` string. Both now run over the Tauri-IPC transport:
// `tauriFetch` in client.ts routes every call through the `api_request`
// command rather than a loopback `fetch`, so these tests stub `invoke` (the
// command boundary) and return the command's wire response shape. The vitest
// config inlines ENTROPIAORME_BACKEND_PORT=8421, so the URL builders stay
// stable.
//
// `invoke` is captured at module load (client.ts imports it), so the mock must
// be in place BEFORE the module is imported: a hoisted mock plus a fresh import
// per test via vi.resetModules() + dynamic import().
const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));

type Mod = typeof import('./client');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./client');
}

beforeEach(() => {
	invokeMock.mockReset();
});

afterEach(() => {
	vi.unstubAllGlobals();
});

/** The `api_request` command's wire response (what `invoke` resolves to);
 * `tauriFetch` rebuilds a `Response` from it. The status line's reason phrase
 * rides `statusText` so the empty-body error fallback still has it. */
function wire(
	body: string,
	init?: { status?: number; statusText?: string; headers?: [string, string][] },
): { status: number; statusText: string; headers: [string, string][]; body: string } {
	return {
		status: init?.status ?? 200,
		statusText: init?.statusText ?? '',
		headers: init?.headers ?? [],
		body,
	};
}

describe('ApiError', () => {
	it('carries status, message, and a distinguishing name', async () => {
		const { ApiError } = await loadModule();
		const err = new ApiError(418, 'teapot');
		expect(err).toBeInstanceOf(Error);
		expect(err.status).toBe(418);
		expect(err.message).toBe('teapot');
		expect(err.name).toBe('ApiError');
	});
});

describe('URL builders', () => {
	it('fetches the capture preview PNG over the capture_png command as a base64 data URL', async () => {
		const { manualSkillScanCapturePng } = await loadModule();
		invokeMock.mockResolvedValue('aGVsbG8=');
		const url = await manualSkillScanCapturePng(3);
		expect(invokeMock).toHaveBeenCalledWith('capture_png', { page: 3 });
		expect(url).toBe('data:image/png;base64,aGVsbG8=');
	});
});

describe('generated client over the IPC transport', () => {
	it('routes the call through the api_request command with method, path, and Content-Type', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire('{}'));
		await client.GET('/api/settings');

		expect(invokeMock).toHaveBeenCalledTimes(1);
		const [command, args] = invokeMock.mock.calls[0];
		expect(command).toBe('api_request');
		expect(args.request.method).toBe('GET');
		expect(args.request.path).toBe('/api/settings');
		expect(args.request.headers).toContainEqual(['content-type', 'application/json']);
	});

	it('prefers the FastAPI detail string on a non-2xx JSON body', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(
			wire(JSON.stringify({ detail: 'Settings unavailable' }), { status: 404 }),
		);
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			name: 'ApiError',
			status: 404,
			message: 'Settings unavailable',
		});
	});

	it('falls back to the raw body when detail is not a string', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ detail: 42 }), { status: 500 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 500,
			message: '{"detail":42}',
		});
	});

	it('falls back to the raw body when detail is blank', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ detail: '   ' }), { status: 400 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 400,
			message: '{"detail":"   "}',
		});
	});

	it('uses a plain-text error body as the message', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire('upstream down', { status: 503 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 503,
			message: 'upstream down',
		});
	});

	it('falls back to statusText on an empty error body', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire('', { status: 502, statusText: 'Bad Gateway' }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 502,
			message: 'Bad Gateway',
		});
	});

	it('passes a 2xx response through untouched', async () => {
		const { client } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ player_name: 'Mikel' })));
		const { data } = await client.GET('/api/settings');
		expect(data).toEqual({ player_name: 'Mikel' });
	});
});

describe('unwrap', () => {
	it('resolves to the payload as the declared type', async () => {
		const { client, unwrap } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ player_name: 'Mikel' })));
		const settings = await unwrap<{ player_name: string }>(client.GET('/api/settings'));
		expect(settings).toEqual({ player_name: 'Mikel' });
	});

	it('propagates the middleware ApiError', async () => {
		const { ApiError, client, unwrap } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ detail: 'nope' }), { status: 403 }));
		await expect(unwrap(client.GET('/api/settings'))).rejects.toBeInstanceOf(ApiError);
	});
});

describe('legacy request()', () => {
	it('routes the api-prefixed path through the command with a JSON Content-Type and returns the body', async () => {
		const { request } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ ok: true })));
		const result = await request<{ ok: boolean }>('/ping');

		expect(result).toEqual({ ok: true });
		const [command, args] = invokeMock.mock.calls[0];
		expect(command).toBe('api_request');
		expect(args.request.method).toBe('GET');
		expect(args.request.path).toBe('/api/ping');
		expect(args.request.headers).toContainEqual(['content-type', 'application/json']);
	});

	it('spreads caller options into the request', async () => {
		const { request } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ ok: true })));
		await request('/ping', { method: 'POST', body: '{}' });

		const args = invokeMock.mock.calls[0][1];
		expect(args.request.method).toBe('POST');
		expect(args.request.body).toBe('{}');
		expect(args.request.path).toBe('/api/ping');
	});

	it('throws ApiError with the detail string on a non-2xx JSON body', async () => {
		const { request } = await loadModule();
		invokeMock.mockResolvedValue(wire(JSON.stringify({ detail: 'gone' }), { status: 410 }));
		await expect(request('/ping')).rejects.toMatchObject({
			name: 'ApiError',
			status: 410,
			message: 'gone',
		});
	});

	it('throws ApiError with the raw text on a non-JSON error body', async () => {
		const { request } = await loadModule();
		invokeMock.mockResolvedValue(wire('boom', { status: 500 }));
		await expect(request('/ping')).rejects.toMatchObject({ status: 500, message: 'boom' });
	});

	it('falls back to statusText on an empty error body', async () => {
		const { request } = await loadModule();
		invokeMock.mockResolvedValue(wire('', { status: 502, statusText: 'Bad Gateway' }));
		await expect(request('/ping')).rejects.toMatchObject({
			status: 502,
			message: 'Bad Gateway',
		});
	});
});
