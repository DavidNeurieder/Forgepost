import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Setup from './+page.svelte';
import { api, bootstrap } from '$lib/api';
import { goto } from '$app/navigation';
import { session } from '../../test/fixtures';
import { waitForFlush } from '../../test/helpers';

vi.mock('$lib/api', () => ({
	api: vi.fn(),
	bootstrap: vi.fn(),
	currentSession: vi.fn()
}));

const mockedApi = vi.mocked(api);
const mockedBootstrap = vi.mocked(bootstrap);

function mockNotSetup() {
	mockedApi.mockResolvedValue({ setup_complete: false });
}

async function renderForm() {
	render(Setup);
	await waitForFlush(() => expect(screen.getByLabelText('Email')).toBeInTheDocument());
}

async function fillAndSubmit(overrides: Partial<Record<string, string>> = {}) {
	const values = {
		Email: 'a@b.c',
		'Display name': 'Grace',
		Password: 'correct-horse',
		'Confirm password': 'correct-horse',
		...overrides
	};
	for (const [label, value] of Object.entries(values)) {
		await fireEvent.input(screen.getByLabelText(label), { target: { value } });
	}
	// Dispatch submit on the form directly: jsdom applies type="email" constraint
	// validation to submit-button clicks and would swallow the app's own checks.
	await fireEvent.submit(document.querySelector('form')!);
}

beforeEach(() => {
	mockedApi.mockClear();
	mockedBootstrap.mockClear();
	vi.mocked(goto).mockClear();
});

describe('setup page', () => {
	it('shows the form when setup is incomplete', async () => {
		mockNotSetup();
		await renderForm();
		expect(screen.getByRole('button', { name: 'Create account' })).toBeInTheDocument();
	});

	it('redirects to login when setup is complete', async () => {
		mockedApi.mockResolvedValue({ setup_complete: true });
		render(Setup);

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/login'));
	});

	it('creates the owner account and redirects to the dashboard', async () => {
		mockNotSetup();
		mockedBootstrap.mockResolvedValue(session);
		await renderForm();

		await fillAndSubmit();

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/admin'));
		expect(mockedBootstrap).toHaveBeenCalledWith('/setup', {
			email: 'a@b.c',
			password: 'correct-horse',
			display_name: 'Grace'
		});
	});

	it('rejects an invalid email address', async () => {
		mockNotSetup();
		await renderForm();

		await fillAndSubmit({ Email: 'not-an-email' });

		await waitForFlush(() => expect(screen.getByText('Enter a valid email address.')).toBeInTheDocument());
		expect(mockedBootstrap).not.toHaveBeenCalled();
	});

	it('rejects a short password', async () => {
		mockNotSetup();
		await renderForm();

		await fillAndSubmit({ Password: 'short' });

		await waitForFlush(() =>
			expect(screen.getByText('Password must be at least 8 characters.')).toBeInTheDocument()
		);
		expect(mockedBootstrap).not.toHaveBeenCalled();
	});

	it('rejects mismatched confirmation', async () => {
		mockNotSetup();
		await renderForm();

		await fillAndSubmit({ 'Confirm password': 'different' });

		await waitForFlush(() => expect(screen.getByText('Passwords do not match.')).toBeInTheDocument());
		expect(mockedBootstrap).not.toHaveBeenCalled();
	});

	it('rejects an empty display name', async () => {
		mockNotSetup();
		await renderForm();

		await fillAndSubmit({ 'Display name': '  ' });

		await waitForFlush(() => expect(screen.getByText('Enter a display name.')).toBeInTheDocument());
		expect(mockedBootstrap).not.toHaveBeenCalled();
	});
});
