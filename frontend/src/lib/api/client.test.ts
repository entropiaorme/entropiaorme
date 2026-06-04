import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The generated client and the legacy request() helper share one error
// contract: any non-2xx throws ApiError(status, message), message preferring
// the FastAPI `detail` string. Both run against a stubbed global fetch; the
// vitest config inlines ENTROPIAORME_BACKEND_PORT=8421, so URLs are stable.
//
// openapi-fetch captures globalThis.fetch when createClient() runs at module
// load (the same captured-at-import wrinkle as preferences.ts' inTauri), so
// the stub must be in place BEFORE the module is imported: fresh import per
// test via vi.resetModules() + dynamic import().
const fetchMock = vi.fn();

type Mod = typeof import('./client');

async function loadModule(): Promise<Mod> {
	vi.resetModules();
	return import('./client');
}

beforeEach(() => {
	fetchMock.mockReset();
	vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
	vi.unstubAllGlobals();
});

function jsonResponse(body: unknown, init?: ResponseInit): Response {
	return new Response(JSON.stringify(body), {
		headers: { 'Content-Type': 'application/json' },
		...init,
	});
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
	it('builds the events stream URL on the configured loopback origin', async () => {
		const { EVENTS_STREAM_URL } = await loadModule();
		expect(EVENTS_STREAM_URL).toBe('http://127.0.0.1:8421/api/events');
	});

	it('builds the capture preview PNG URL per page', async () => {
		const { manualSkillScanCapturePngUrl } = await loadModule();
		expect(manualSkillScanCapturePngUrl(3)).toBe('http://127.0.0.1:8421/api/scan/skills/capture/3');
	});
});

describe('generated client error middleware', () => {
	it('targets the loopback origin and sends Content-Type on every call', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({}));
		await client.GET('/api/settings');

		expect(fetchMock).toHaveBeenCalledTimes(1);
		const requestArg = fetchMock.mock.calls[0][0] as Request;
		expect(requestArg.url).toBe('http://127.0.0.1:8421/api/settings');
		expect(requestArg.headers.get('content-type')).toBe('application/json');
	});

	it('prefers the FastAPI detail string on a non-2xx JSON body', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ detail: 'Settings unavailable' }, { status: 404 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			name: 'ApiError',
			status: 404,
			message: 'Settings unavailable',
		});
	});

	it('falls back to the raw body when detail is not a string', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ detail: 42 }, { status: 500 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 500,
			message: '{"detail":42}',
		});
	});

	it('falls back to the raw body when detail is blank', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ detail: '   ' }, { status: 400 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 400,
			message: '{"detail":"   "}',
		});
	});

	it('uses a plain-text error body as the message', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(new Response('upstream down', { status: 503 }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 503,
			message: 'upstream down',
		});
	});

	it('falls back to statusText on an empty error body', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(new Response('', { status: 502, statusText: 'Bad Gateway' }));
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 502,
			message: 'Bad Gateway',
		});
	});

	it('falls back to statusText when the body cannot be read', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue({
			ok: false,
			status: 500,
			statusText: 'Internal Server Error',
			text: () => Promise.reject(new Error('stream aborted')),
		});
		await expect(client.GET('/api/settings')).rejects.toMatchObject({
			status: 500,
			message: 'Internal Server Error',
		});
	});

	it('passes a 2xx response through untouched', async () => {
		const { client } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ player_name: 'Mikel' }));
		const { data } = await client.GET('/api/settings');
		expect(data).toEqual({ player_name: 'Mikel' });
	});
});

describe('unwrap', () => {
	it('resolves to the payload as the declared type', async () => {
		const { client, unwrap } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ player_name: 'Mikel' }));
		const settings = await unwrap<{ player_name: string }>(client.GET('/api/settings'));
		expect(settings).toEqual({ player_name: 'Mikel' });
	});

	it('propagates the middleware ApiError', async () => {
		const { ApiError, client, unwrap } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ detail: 'nope' }, { status: 403 }));
		await expect(unwrap(client.GET('/api/settings'))).rejects.toBeInstanceOf(ApiError);
	});
});

describe('legacy request()', () => {
	it('fetches the api-prefixed path with a JSON Content-Type and returns the body', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ ok: true }));
		const result = await request<{ ok: boolean }>('/ping');

		expect(result).toEqual({ ok: true });
		expect(fetchMock).toHaveBeenCalledWith('http://127.0.0.1:8421/api/ping', {
			headers: { 'Content-Type': 'application/json' },
		});
	});

	it('spreads caller options into the fetch init', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ ok: true }));
		await request('/ping', { method: 'POST', body: '{}' });

		expect(fetchMock.mock.calls[0][1]).toMatchObject({ method: 'POST', body: '{}' });
	});

	it('throws ApiError with the detail string on a non-2xx JSON body', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue(jsonResponse({ detail: 'gone' }, { status: 410 }));
		await expect(request('/ping')).rejects.toMatchObject({
			name: 'ApiError',
			status: 410,
			message: 'gone',
		});
	});

	it('throws ApiError with the raw text on a non-JSON error body', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue(new Response('boom', { status: 500 }));
		await expect(request('/ping')).rejects.toMatchObject({ status: 500, message: 'boom' });
	});

	it('falls back to statusText when the error body cannot be read', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue({
			ok: false,
			status: 500,
			statusText: 'Internal Server Error',
			text: () => Promise.reject(new Error('stream aborted')),
		});
		await expect(request('/ping')).rejects.toMatchObject({
			status: 500,
			message: 'Internal Server Error',
		});
	});

	it('falls back to statusText on an empty error body', async () => {
		const { request } = await loadModule();
		fetchMock.mockResolvedValue(new Response('', { status: 502, statusText: 'Bad Gateway' }));
		await expect(request('/ping')).rejects.toMatchObject({
			status: 502,
			message: 'Bad Gateway',
		});
	});
});
