import { goto } from '$app/navigation';
import type { RenderView, SessionResponse } from './types';

const CSRF_KEY = 'openpublish.csrf';

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.status = status;
	}
}

export function getCsrf(): string | null {
	return typeof localStorage === 'undefined' ? null : localStorage.getItem(CSRF_KEY);
}

export function setCsrf(token: string | null): void {
	if (typeof localStorage === 'undefined') return;
	if (token) {
		localStorage.setItem(CSRF_KEY, token);
	} else {
		localStorage.removeItem(CSRF_KEY);
	}
}

interface ApiOptions {
	method?: string;
	body?: unknown;
}

export async function api<T>(path: string, opts: ApiOptions = {}): Promise<T> {
	const method = opts.method ?? 'GET';
	const headers: Record<string, string> = {};
	if (opts.body !== undefined) headers['Content-Type'] = 'application/json';
	if (method !== 'GET' && method !== 'HEAD') {
		const csrf = getCsrf();
		if (csrf) headers['x-csrf-token'] = csrf;
	}
	const res = await fetch(`/api${path}`, {
		method,
		headers,
		body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined
	});
	if (res.status === 401 && typeof window !== 'undefined' && !path.startsWith('/me')) {
		setCsrf(null);
		goto('/login');
	}
	if (!res.ok) {
		let message = `HTTP ${res.status}`;
		try {
			const data = await res.json();
			message = (data as { error?: string }).error ?? message;
		} catch {
			// not JSON; keep the status-based message
		}
		throw new ApiError(res.status, message);
	}
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

export async function bootstrap(
	path: '/setup' | '/login',
	body: { email: string; password: string; display_name?: string }
): Promise<SessionResponse> {
	const session = await api<SessionResponse>(path, { method: 'POST', body });
	setCsrf(session.csrf_token);
	return session;
}

export function currentSession(): Promise<SessionResponse> {
	return api<SessionResponse>('/me');
}

export async function logout(): Promise<void> {
	try {
		await api<void>('/logout', { method: 'POST' });
	} finally {
		setCsrf(null);
	}
}

export function renderMarkdown(markdown: string): Promise<RenderView> {
	return api('/render', { method: 'POST', body: { markdown } });
}
