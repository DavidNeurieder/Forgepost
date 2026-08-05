import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import Home from './+page.svelte';
import { api } from '$lib/api';
import { goto } from '$app/navigation';
import { summary } from '../test/fixtures';
import { waitForFlush } from '../test/helpers';

vi.mock('$lib/api', () => ({
	api: vi.fn(),
	bootstrap: vi.fn(),
	currentSession: vi.fn()
}));

const mockedApi = vi.mocked(api);

function mockHome(setupComplete: boolean, articles: unknown[]) {
	mockedApi.mockImplementation((path) => {
		if (path === '/setup') return Promise.resolve({ setup_complete: setupComplete });
		if (path === '/articles') return Promise.resolve(articles);
		return Promise.resolve(undefined);
	});
}

beforeEach(() => {
	mockedApi.mockClear();
	vi.mocked(goto).mockClear();
});

describe('home page', () => {
	it('redirects to setup when no owner exists yet', async () => {
		mockHome(false, []);
		render(Home);

		await waitForFlush(() => expect(goto).toHaveBeenCalledWith('/setup'));
	});

	it('lists published posts when setup is complete', async () => {
		mockHome(true, [summary]);
		render(Home);

		await waitForFlush(() => expect(screen.getByText('My First Post')).toBeInTheDocument());
	});

	it('shows a placeholder when setup is complete but there are no posts', async () => {
		mockHome(true, []);
		render(Home);

		await waitForFlush(() => expect(screen.getByText('No published posts yet.')).toBeInTheDocument());
	});
});
