import type { CommentView } from '../lib/types';

export const owner = { id: 'u1', email: 'a@b.c', display_name: 'Grace', role: 'owner' };

export const session = {
	user: owner,
	csrf_token: 'csrf-tok'
};

export const summary = {
	id: 'doc-1',
	title: 'My First Post',
	slug: 'my-first-post',
	status: 'published',
	published_at_ms: 1_700_000_000_000,
	updated_at_ms: 1_700_000_000_000
};

export const blockViews = [
	{ id: 'b1', kind: 'Heading { level: 2 }', content: { text: 'Section' } },
	{ id: 'b2', kind: 'Quote', content: { text: 'be safe' } },
	{ id: 'b3', kind: 'Code', content: { language: 'rust', code: 'let x = 1;' } },
	{ id: 'b4', kind: 'Image', content: { src: '/a.png', alt: 'pic' } },
	{ id: 'b5', kind: 'Divider', content: {} },
	{ id: 'b6', kind: 'Paragraph', content: { text: 'hello world' } }
];

export const expectedMarkdown = [
	'## Section',
	'> be safe',
	'```rust\nlet x = 1;\n```',
	'![pic](/a.png)',
	'---',
	'hello world'
].join('\n\n');

export const documentView = {
	id: 'doc-1',
	title: 'My First Post',
	slug: 'my-first-post',
	status: 'draft',
	published_at_ms: null,
	updated_at_ms: 1_700_000_000_000,
	tags: ['tech', 'blog'],
	blocks: blockViews
};

export const pendingComment: CommentView = {
	id: 'c1',
	document_id: 'doc-1',
	author_name: 'Ann',
	body: 'Nice post!',
	status: 'pending',
	created_at_ms: 1_700_000_100_000
};
