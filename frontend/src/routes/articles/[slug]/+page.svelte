<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { ArticleView, CommentView } from '$lib/types';
	import { trackArticle } from '$lib/tracker';

	let { params } = $props();

	let article = $state<ArticleView | null>(null);
	let comments = $state<CommentView[]>([]);
	let loading = $state(true);
	let error = $state('');

	let author = $state('');
	let commentBody = $state('');
	let submitting = $state(false);
	let posted = $state(false);
	let formError = $state('');

	function formatDate(ms: number | null): string {
		if (!ms) return '';
		return new Date(ms).toLocaleDateString();
	}

	$effect(() => {
		const slug = params.slug;
		loading = true;
		error = '';
		api<ArticleView>(`/articles/${slug}`)
			.then((a) => (article = a))
			.catch((e) => (error = e instanceof Error ? e.message : 'Article not found.'))
			.finally(() => (loading = false));
		api<CommentView[]>(`/articles/${slug}/comments`)
			.then((c) => (comments = c))
			.catch(() => {});
	});

	let tracker: ReturnType<typeof trackArticle> | null = null;
	$effect(() => {
		if (article && !tracker) {
			tracker = trackArticle(article.slug);
		}
	});
	onMount(() => () => tracker?.dispose());

	async function submitComment() {
		submitting = true;
		formError = '';
		posted = false;
		try {
			const c = await api<CommentView>(`/articles/${params.slug}/comments`, {
				method: 'POST',
				body: { author_name: author.trim(), body: commentBody.trim() }
			});
			posted = true;
			commentBody = '';
			if (c.status === 'approved') comments = [...comments, c];
		} catch (e) {
			formError = e instanceof Error ? e.message : 'Could not post comment.';
		} finally {
			submitting = false;
		}
	}
</script>

<svelte:head>
	<title>{article?.title ? `${article.title} · OpenPublish` : 'OpenPublish'}</title>
</svelte:head>

{#if loading}
	<p class="muted">Loading…</p>
{:else if error}
	<p class="error">{error}</p>
{:else if article}
	<article>
		<h1>{article.title}</h1>
		<p class="muted">{formatDate(article.published_at_ms)}</p>
		{#if article.tags.length > 0}
			<p>
				{#each article.tags as t (t)}
					<span class="badge">{t}</span>
				{/each}
			</p>
		{/if}
		<div class="article-body">
			{#each article.rendered_blocks as block (block.id)}
				<div class="tracked-block" data-block-id={block.id}>
					{@html block.html}
				</div>
			{/each}
		</div>
	</article>

	<hr style="margin:2rem 0;border:none;border-top:1px solid var(--border);" />

	<section>
		<h2>Comments</h2>
		{#if comments.length === 0}
			<p class="muted">No comments yet.</p>
		{:else}
			{#each comments as c (c.id)}
				<div class="notice">
					<strong>{c.author_name}</strong>
					<span class="muted"> · {formatDate(c.created_at_ms)}</span>
					<p style="margin:0.4rem 0 0;">{c.body}</p>
				</div>
			{/each}
		{/if}

		<form
			onsubmit={(e) => {
				e.preventDefault();
				submitComment();
			}}
		>
			<field>
				<label for="author">Name</label>
				<input id="author" bind:value={author} maxlength="80" placeholder="Your name" />
			</field>
			<field>
				<label for="body">Comment</label>
				<textarea
					id="body"
					bind:value={commentBody}
					maxlength="2000"
					rows="4"
					placeholder="Leave a comment"
				></textarea>
			</field>
			{#if formError}
				<p class="error">{formError}</p>
			{/if}
			{#if posted}
				<p class="muted">Thanks! Your comment is awaiting moderation.</p>
			{/if}
			<button type="submit" disabled={submitting || !author.trim() || !commentBody.trim()}>
				Post comment
			</button>
		</form>
	</section>
{/if}
