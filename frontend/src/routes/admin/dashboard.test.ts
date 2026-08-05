import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Dashboard from './+page.svelte';
import { api, currentSession, logout } from '$lib/api';
import { goto } from '$app/navigation';
import { pendingComment, session, summary } from '../../test/fixtures';
import { waitForFlush } from '../../test/helpers';

vi.mock('$lib/api', async (importOriginal) => {
	const mod = await importOriginal<typeof import('$lib/api')>();
	return { ...mod, api: vi.fn(), currentSession: vi.fn(), logout: vi.fn() };
});

const mockedApi = vi.mocked(api);
const mockedSession = vi.mocked(currentSession);
const mockedLogout = vi.mocked(logout);

function mockLoaded() {
	mockedSession.mockResolvedValue(session);
	mockedApi.mockImplementation((path: string, opts?: { method?: string }) => {
		if (path === '/documents') return Promise.resolve([summary]);
		if (path === '/comments/pending') return Promise.resolve([pendingComment]);
		if (path === '/comments/c1/approve' && opts?.method === 'POST') return Promise.resolve(undefined);
		return Promise.reject(new Error('unexpected call: ' + path));
	});
}

beforeEach(() => {
	mockedApi.mockClear();
	mockedSession.mockClear();
	mockedLogout.mockClear();
	vi.mocked(goto).mockClear();
});

describe('dashboard page', () => {
	it('lists posts and pending comments', async () => {
		mockLoaded();
		render(Dashboard);

		await waitForFlush(() => expect(screen.getByText('My First Post')).toBeInTheDocument());
		expect(screen.getByText('Signed in as Grace')).toBeInTheDocument();
		expect(screen.getByText('Ann')).toBeInTheDocument();
		expect(screen.getByText('Nice post!')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Approve' })).toBeInTheDocument();
	});

	it('approving a comment removes it from the pending queue', async () => {
		mockLoaded();
		render(Dashboard);
		await waitForFlush(() => expect(screen.getByText('Ann')).toBeInTheDocument());

		await fireEvent.click(screen.getByRole('button', { name: 'Approve' }));

		await waitForFlush(() =>
			expect(screen.getByText('Nothing awaiting moderation.')).toBeInTheDocument()
		);
		expect(mockedApi).toHaveBeenCalledWith('/comments/c1/approve', { method: 'POST' });
	});

	it('logs out and redirects to the login page', async () => {
		mockLoaded();
		mockedLogout.mockResolvedValue();
		render(Dashboard);
		await waitForFlush(() => expect(screen.getByText('Signed in as Grace')).toBeInTheDocument());

		await fireEvent.click(screen.getByRole('button', { name: 'Log out' }));

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/login'));
		expect(mockedLogout).toHaveBeenCalled();
	});

	it('redirects to login when there is no session', async () => {
		const { ApiError } = await import('$lib/api');
		mockedSession.mockRejectedValue(new ApiError(401, 'no session'));
		render(Dashboard);

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/login'));
	});
});
