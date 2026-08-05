import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Article from './+page.svelte';
import { api } from '$lib/api';
import { trackArticle } from '$lib/tracker';
import { waitForFlush } from '../../../test/helpers';

vi.mock('$lib/api', () => ({ api: vi.fn() }));
vi.mock('$lib/tracker', () => ({ trackArticle: vi.fn(() => ({ dispose: vi.fn() })) }));

const mockedApi = vi.mocked(api);
const mockedTracker = vi.mocked(trackArticle);

const article = {
	id: 'a1',
	title: 'My Post',
	slug: 'my-post',
	published_at_ms: 1_700_000_000_000,
	updated_at_ms: 1_700_000_000_000,
	tags: ['tech', 'blog'],
	blocks: [],
	html: '<h1>My Post</h1>',
	rendered_blocks: [
		{
			id: 'rb1',
			kind: 'Heading { level: 1 }',
			html: '<h1>My Post</h1>',
			experiment_id: null,
			variant_id: null
		},
		{
			id: 'rb2',
			kind: 'Paragraph',
			html: '<p>Body</p>',
			experiment_id: null,
			variant_id: null
		}
	]
};

function mockRoutes(comments: unknown[] = []) {
	mockedApi.mockImplementation((path: string, opts?: { method?: string; body?: unknown }) => {
		if (path === '/articles/my-post') return Promise.resolve(article);
		if (path === '/articles/my-post/comments' && !opts?.method)
			return Promise.resolve(comments);
		if (path === '/articles/my-post/comments' && opts?.method === 'POST')
			return Promise.resolve(opts.body);
		return Promise.reject(new Error('unexpected call: ' + path));
	});
}

function renderArticle() {
	return render(Article, { props: { params: { slug: 'my-post' } } });
}

async function waitForArticle() {
	await waitForFlush(() => expect(screen.getAllByText('My Post').length).toBeGreaterThan(0));
}

beforeEach(() => {
	mockedApi.mockClear();
	mockedTracker.mockClear();
});

describe('article page', () => {
	it('renders the article and starts the tracker', async () => {
		mockRoutes();
		const { container } = renderArticle();

		await waitForArticle();
		expect(screen.getByText('tech')).toBeInTheDocument();
		expect(screen.getByText('blog')).toBeInTheDocument();
		expect(container.querySelector('[data-block-id="rb1"]')?.textContent).toContain('My Post');
		expect(container.querySelector('[data-block-id="rb2"]')?.textContent).toBe('Body');
		expect(screen.getByText('No comments yet.')).toBeInTheDocument();
		expect(mockedTracker).toHaveBeenCalledWith('my-post');
	});

	it('shows an error when the article cannot be fetched', async () => {
		mockedApi.mockRejectedValue(new Error('gone'));
		renderArticle();

		await waitForFlush(() => expect(screen.getByText('gone')).toBeInTheDocument());
		expect(mockedTracker).not.toHaveBeenCalled();
	});

	it('posts a comment awaiting moderation', async () => {
		mockRoutes();
		renderArticle();
		await waitForArticle();

		await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Ann' } });
		await fireEvent.input(screen.getByLabelText('Comment'), { target: { value: 'Nice post!' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Post comment' }));

		await waitForFlush(() =>
			expect(screen.getByText('Thanks! Your comment is awaiting moderation.')).toBeInTheDocument()
		);
		expect(mockedApi).toHaveBeenCalledWith('/articles/my-post/comments', {
			method: 'POST',
			body: { author_name: 'Ann', body: 'Nice post!' }
		});
		expect(screen.getByText('No comments yet.')).toBeInTheDocument();
	});

	it('appends an approved comment to the list', async () => {
		mockRoutes([
			{
				id: 'c1',
				document_id: 'a1',
				author_name: 'Ann',
				body: 'Nice post!',
				status: 'approved',
				created_at_ms: 1
			}
		]);
		renderArticle();

		await waitForFlush(() => expect(screen.getByText('Ann')).toBeInTheDocument());
		expect(screen.getByText('Nice post!')).toBeInTheDocument();
	});

	it('shows a form error when posting fails', async () => {
		mockedApi.mockImplementation((path: string, opts?: { method?: string }) => {
			if (path === '/articles/my-post') return Promise.resolve(article);
			if (path === '/articles/my-post/comments' && !opts?.method) return Promise.resolve([]);
			if (path === '/articles/my-post/comments' && opts?.method === 'POST')
				return Promise.reject(new Error('captcha required'));
			return Promise.reject(new Error('unexpected call: ' + path));
		});
		renderArticle();
		await waitForArticle();

		await fireEvent.input(screen.getByLabelText('Name'), { target: { value: 'Ann' } });
		await fireEvent.input(screen.getByLabelText('Comment'), { target: { value: 'Nice' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Post comment' }));

		await waitForFlush(() => expect(screen.getByText('captcha required')).toBeInTheDocument());
	});
});
