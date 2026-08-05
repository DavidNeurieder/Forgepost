import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { goto } from '$app/navigation';
import { ApiError, api, bootstrap, getCsrf, logout, renderMarkdown, setCsrf } from './api';

type FetchMock = typeof fetch;

function mockFetch(status: number, body: unknown, headers?: Record<string, string>): FetchMock {
	const text = typeof body === 'string' ? body : JSON.stringify(body);
	return vi.fn(
		async () =>
			new Response(status === 204 ? null : text, {
				status,
				headers: { 'Content-Type': 'application/json', ...headers }
			})
	) as unknown as FetchMock;
}

function lastRequest(): { url: string; init: RequestInit } {
	const calls = (globalThis.fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls;
	expect(calls.length).toBeGreaterThan(0);
	const [url, init] = calls[calls.length - 1];
	return { url: String(url), init: (init ?? {}) as RequestInit };
}

beforeEach(() => {
	localStorage.clear();
	vi.mocked(goto).mockClear();
});

afterEach(() => {
	vi.restoreAllMocks();
});

describe('api', () => {
	it('prefixes the path with /api and uses GET by default', async () => {
		globalThis.fetch = mockFetch(200, [{ id: 1 }]);
		const out = await api('/documents');
		const { url, init } = lastRequest();
		expect(url).toBe('/api/documents');
		expect(init.method ?? 'GET').toBe('GET');
		expect(out).toEqual([{ id: 1 }]);
	});

	it('sends no CSRF header on GET', async () => {
		setCsrf('tok');
		globalThis.fetch = mockFetch(200, []);
		await api('/documents');
		const { init } = lastRequest();
		const headers = (init.headers ?? {}) as Record<string, string>;
		expect(headers['x-csrf-token']).toBeUndefined();
	});

	it('sends the CSRF header and JSON content type on mutating requests', async () => {
		setCsrf('tok');
		globalThis.fetch = mockFetch(200, {});
		await api('/documents', { method: 'POST', body: { title: 'x' } });
		const { url, init } = lastRequest();
		expect(url).toBe('/api/documents');
		expect(init.method).toBe('POST');
		const headers = init.headers as Record<string, string>;
		expect(headers['Content-Type']).toBe('application/json');
		expect(headers['x-csrf-token']).toBe('tok');
		expect(init.body).toBe(JSON.stringify({ title: 'x' }));
	});

	it('omits the CSRF header when no token is stored', async () => {
		globalThis.fetch = mockFetch(200, {});
		await api('/documents', { method: 'POST', body: {} });
		const { init } = lastRequest();
		const headers = (init.headers ?? {}) as Record<string, string>;
		expect(headers['x-csrf-token']).toBeUndefined();
	});

	it('clears the session and redirects on 401', async () => {
		setCsrf('tok');
		globalThis.fetch = mockFetch(401, { error: 'no session' });
		await expect(api('/documents', { method: 'GET' })).rejects.toThrow('no session');
		expect(getCsrf()).toBeNull();
		expect(goto).toHaveBeenCalledWith('/login');
	});

	it('does not redirect when /me returns 401', async () => {
		setCsrf('tok');
		globalThis.fetch = mockFetch(401, { error: 'no session' });
		await expect(api('/me')).rejects.toThrow('no session');
		expect(getCsrf()).toBe('tok');
		expect(goto).not.toHaveBeenCalled();
	});

	it('throws an ApiError carrying the server error message', async () => {
		globalThis.fetch = mockFetch(400, { error: 'bad payload' });
		const err = await api('/documents', { method: 'POST', body: {} }).catch((e) => e);
		expect(err).toBeInstanceOf(ApiError);
		expect((err as ApiError).status).toBe(400);
		expect((err as ApiError).message).toBe('bad payload');
	});

	it('falls back to the status text for non-JSON errors', async () => {
		globalThis.fetch = mockFetch(500, 'boom', { 'Content-Type': 'text/plain' });
		const err = await api('/documents').catch((e) => e);
		expect((err as ApiError).message).toBe('HTTP 500');
	});

	it('resolves undefined for 204 responses', async () => {
		globalThis.fetch = mockFetch(204, '', { 'Content-Type': 'text/plain' });
		const out = await api('/logout', { method: 'POST' });
		expect(out).toBeUndefined();
	});
});

describe('bootstrap', () => {
	it('stores the session CSRF token', async () => {
		globalThis.fetch = mockFetch(200, {
			user: { id: 'u1', email: 'a@b.c', display_name: 'A', role: 'owner' },
			csrf_token: 'csrf-1'
		});
		const session = await bootstrap('/login', { email: 'a@b.c', password: 'pw' });
		expect(session.csrf_token).toBe('csrf-1');
		expect(getCsrf()).toBe('csrf-1');
		const { url, init } = lastRequest();
		expect(url).toBe('/api/login');
		expect(init.method).toBe('POST');
		expect(init.body).toBe(JSON.stringify({ email: 'a@b.c', password: 'pw' }));
	});
});

describe('logout', () => {
	it('clears the CSRF token even when the request fails', async () => {
		setCsrf('tok');
		globalThis.fetch = vi.fn(async () => {
			throw new Error('offline');
		}) as unknown as typeof fetch;
		await logout();
		expect(getCsrf()).toBeNull();
	});
});

describe('renderMarkdown', () => {
	it('posts markdown to /render and returns the rendered view', async () => {
		globalThis.fetch = mockFetch(200, { html: '<p>x</p>', blocks: [] });
		const out = await renderMarkdown('# Hi');
		expect(out.html).toBe('<p>x</p>');
		const { url, init } = lastRequest();
		expect(url).toBe('/api/render');
		expect(init.method).toBe('POST');
		expect(init.body).toBe(JSON.stringify({ markdown: '# Hi' }));
	});
});
