import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Login from './+page.svelte';
import { bootstrap, currentSession } from '$lib/api';
import { goto } from '$app/navigation';
import { session } from '../../test/fixtures';
import { waitForFlush } from '../../test/helpers';

vi.mock('$lib/api', () => ({
	api: vi.fn(),
	bootstrap: vi.fn(),
	currentSession: vi.fn()
}));

const mockedBootstrap = vi.mocked(bootstrap);
const mockedSession = vi.mocked(currentSession);

beforeEach(() => {
	mockedBootstrap.mockClear();
	mockedSession.mockClear();
	vi.mocked(goto).mockClear();
});

describe('login page', () => {
	it('shows the form when there is no active session', async () => {
		mockedSession.mockRejectedValue(new Error('no session'));
		render(Login);

		await waitForFlush(() => expect(screen.getByLabelText('Email')).toBeInTheDocument());
		expect(screen.getByRole('button', { name: 'Log in' })).toBeInTheDocument();
	});

	it('redirects to the dashboard when already signed in', async () => {
		mockedSession.mockResolvedValue(session);
		render(Login);

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/admin'));
	});

	it('logs in and redirects', async () => {
		mockedSession.mockRejectedValue(new Error('no session'));
		mockedBootstrap.mockResolvedValue(session);
		render(Login);
		await waitForFlush(() => expect(screen.getByLabelText('Email')).toBeInTheDocument());

		await fireEvent.input(screen.getByLabelText('Email'), { target: { value: 'a@b.c' } });
		await fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'hunter2' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Log in' }));

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/admin'));
		expect(mockedBootstrap).toHaveBeenCalledWith('/login', {
			email: 'a@b.c',
			password: 'hunter2'
		});
	});

	it('shows the server error when login fails', async () => {
		mockedSession.mockRejectedValue(new Error('no session'));
		mockedBootstrap.mockRejectedValue(new Error('invalid credentials'));
		render(Login);
		await waitForFlush(() => expect(screen.getByLabelText('Email')).toBeInTheDocument());

		await fireEvent.input(screen.getByLabelText('Email'), { target: { value: 'a@b.c' } });
		await fireEvent.input(screen.getByLabelText('Password'), { target: { value: 'hunter2' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Log in' }));

		await waitForFlush(() => expect(screen.getByText('invalid credentials')).toBeInTheDocument());
	});
});
