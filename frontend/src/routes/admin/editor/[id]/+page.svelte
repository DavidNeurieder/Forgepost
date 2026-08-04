<script lang="ts">
	import { api, renderMarkdown } from '$lib/api';
	import type { BlockView, DocumentView } from '$lib/types';

	let { params } = $props();

	let loading = $state(true);
	let loadError = $state('');
	let title = $state('');
	let tagsInput = $state('');
	let markdown = $state('');
	let status = $state('draft');

	let previewHtml = $state('');
	let previewBlocks = $state<{ kind: string; content: Record<string, unknown> }[]>([]);
	let previewError = $state('');

	let busy = $state(false);
	let message = $state('');
	let error = $state('');

	function textOf(content: Record<string, unknown>): string {
		return String(content.text ?? '');
	}

	function blocksToMarkdown(blocks: BlockView[]): string {
		const parts: string[] = [];
		for (const b of blocks) {
			const c = b.content;
			if (b.kind.startsWith('Heading')) {
				const m = b.kind.match(/level: (\d+)/);
				const level = m ? Number(m[1]) : 1;
				parts.push(`${'#'.repeat(level)} ${textOf(c)}`);
			} else if (b.kind === 'Quote') {
				parts.push(
					textOf(c)
						.split('\n')
						.map((l) => `> ${l}`)
						.join('\n')
				);
			} else if (b.kind === 'Code') {
				parts.push(`\`\`\`${String(c.language ?? '')}\n${String(c.code ?? '')}\n\`\`\``);
			} else if (b.kind === 'Image') {
				parts.push(`![${String(c.alt ?? '')}](${String(c.src ?? '')})`);
			} else if (b.kind === 'Divider') {
				parts.push('---');
			} else {
				parts.push(textOf(c));
			}
		}
		return parts.join('\n\n');
	}

	let timer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		const id = params.id;
		loading = true;
		api<DocumentView>(`/documents/${id}`)
			.then((d) => {
				title = d.title;
				tagsInput = d.tags.join(', ');
				status = d.status;
				markdown = blocksToMarkdown(d.blocks);
			})
			.catch((e) => (loadError = e instanceof Error ? e.message : 'Could not load post.'))
			.finally(() => (loading = false));
	});

	$effect(() => {
		const md = markdown;
		clearTimeout(timer);
		timer = setTimeout(async () => {
			if (!md.trim()) {
				previewHtml = '';
				previewBlocks = [];
				previewError = '';
				return;
			}
			try {
				const r = await renderMarkdown(md);
				previewHtml = r.html;
				previewBlocks = r.blocks;
				previewError = '';
			} catch (e) {
				previewError = e instanceof Error ? e.message : 'Preview failed.';
			}
		}, 400);
	});

	async function save() {
		busy = true;
		message = '';
		error = '';
		try {
			await api(`/documents/${params.id}`, {
				method: 'PUT',
				body: {
					title: title.trim(),
					markdown,
					tags: tagsInput
						.split(',')
						.map((s) => s.trim())
						.filter(Boolean)
				}
			});
			message = 'Saved';
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not save.';
		} finally {
			busy = false;
		}
	}

	async function publish() {
		busy = true;
		message = '';
		error = '';
		try {
			await api(`/documents/${params.id}/publish`, { method: 'POST' });
			status = 'published';
			message = 'Published';
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not publish.';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>Editor · OpenPublish</title>
</svelte:head>

{#if loading}
	<p class="muted">Loading…</p>
{:else if loadError}
	<p class="error">{loadError}</p>
{:else}
	<h1>Editor</h1>
	<p class="muted">
		Status: <span class="badge {status}">{status}</span>
	</p>

	{#if message}
		<p class="muted">{message}</p>
	{/if}
	{#if error}
		<p class="error">{error}</p>
	{/if}

	<field>
		<label for="title">Title</label>
		<input id="title" bind:value={title} />
	</field>
	<field>
		<label for="tags">Tags (comma separated)</label>
		<input id="tags" bind:value={tagsInput} placeholder="tech, blog" />
	</field>

	<field>
		<label for="markdown">Markdown</label>
		<textarea id="markdown" bind:value={markdown} rows="16"></textarea>
	</field>

	<div style="display:flex;gap:0.6rem;margin-bottom:1.5rem;">
		<button onclick={save} disabled={busy}>Save</button>
		{#if status !== 'published'}
			<button onclick={publish} disabled={busy}>Publish</button>
		{/if}
		<a class="button secondary" href="/admin">Back to dashboard</a>
	</div>

	<h2>Preview</h2>
	{#if previewError}
		<p class="error">{previewError}</p>
	{:else if previewHtml}
		{#if previewBlocks.length > 0}
			<p class="muted">
				{previewBlocks.length} block{previewBlocks.length === 1 ? '' : 's'} parsed
			</p>
		{/if}
		<div class="notice article-body">{@html previewHtml}</div>
	{:else}
		<p class="muted">Start typing to preview the rendered post.</p>
	{/if}
{/if}
