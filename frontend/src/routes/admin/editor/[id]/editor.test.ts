import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Editor from './+page.svelte';
import { api, renderMarkdown } from '$lib/api';
import { documentView, expectedMarkdown } from '../../../../test/fixtures';
import { waitForFlush } from '../../../../test/helpers';

vi.mock('$lib/api', () => ({
	api: vi.fn(),
	renderMarkdown: vi.fn()
}));

const mockedApi = vi.mocked(api);
const mockedRender = vi.mocked(renderMarkdown);

function mockLoad() {
	mockedApi.mockImplementation(
		(path: string, opts?: { method?: string }) => {
			if (path === '/documents/doc-1' && !opts?.method) return Promise.resolve(documentView);
			if (path === '/documents/doc-1' && opts?.method === 'PUT') return Promise.resolve({});
			if (path === '/documents/doc-1/publish') return Promise.resolve({});
			return Promise.reject(new Error('unexpected call: ' + path));
		}
	);
}

function renderEditor() {
	return render(Editor, { props: { params: { id: 'doc-1' } } });
}

describe('editor page', () => {
	it('loads the document and converts blocks back to markdown', async () => {
		mockLoad();
		renderEditor();

		await waitForFlush(() =>
			expect(screen.getByLabelText('Markdown')).toHaveValue(expectedMarkdown)
		);
		expect(screen.getByLabelText('Title')).toHaveValue('My First Post');
		expect(screen.getByLabelText('Tags (comma separated)')).toHaveValue('tech, blog');
		expect(screen.getByText('draft')).toBeInTheDocument();
	});

	it('shows the load error when the document cannot be fetched', async () => {
		mockedApi.mockRejectedValue(new Error('gone'));
		renderEditor();

		await waitForFlush(() => expect(screen.getByText('gone')).toBeInTheDocument());
	});

	it('saves the document via PUT', async () => {
		mockLoad();
		renderEditor();
		await waitForFlush(() => expect(screen.getByLabelText('Markdown')).toHaveValue(expectedMarkdown));

		await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

		await waitForFlush(() => expect(screen.getByText('Saved')).toBeInTheDocument());
		expect(mockedApi).toHaveBeenCalledWith('/documents/doc-1', {
			method: 'PUT',
			body: { title: 'My First Post', markdown: expectedMarkdown, tags: ['tech', 'blog'] }
		});
	});

	it('publishes the document and hides the publish button', async () => {
		mockLoad();
		renderEditor();
		await waitForFlush(() => expect(screen.getByLabelText('Markdown')).toHaveValue(expectedMarkdown));

		await fireEvent.click(screen.getByRole('button', { name: 'Publish' }));

		await waitForFlush(() => expect(screen.getByText('Published')).toBeInTheDocument());
		expect(mockedApi).toHaveBeenCalledWith('/documents/doc-1/publish', { method: 'POST' });
		await waitForFlush(() =>
			expect(screen.queryByRole('button', { name: 'Publish' })).not.toBeInTheDocument()
		);
	});

	it('debounces and renders a live preview as the markdown changes', async () => {
		mockLoad();
		mockedRender.mockResolvedValue({
			html: '<h2>New</h2>',
			blocks: [{ kind: 'Heading { level: 2 }', content: { text: 'New' } }]
		});
		renderEditor();
		await waitForFlush(() => expect(screen.getByLabelText('Markdown')).toHaveValue(expectedMarkdown));

		const textarea = screen.getByLabelText('Markdown');
		await fireEvent.input(textarea, { target: { value: '## New' } });

		await waitForFlush(() => expect(mockedRender).toHaveBeenCalledWith('## New'));
		await waitForFlush(() => expect(screen.getByText('1 block parsed')).toBeInTheDocument());
		expect(screen.getByText('New')).toBeInTheDocument();
	});

	it('shows a preview error when rendering fails', async () => {
		mockLoad();
		mockedRender.mockRejectedValue(new Error('preview broke'));
		renderEditor();
		await waitForFlush(() => expect(screen.getByLabelText('Markdown')).toHaveValue(expectedMarkdown));

		await fireEvent.input(screen.getByLabelText('Markdown'), {
			target: { value: '## New' }
		});

		await waitForFlush(() => expect(screen.getByText('preview broke')).toBeInTheDocument());
	});
});
