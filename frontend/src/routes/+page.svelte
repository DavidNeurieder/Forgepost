<script lang="ts">
	import { api } from '$lib/api';
	import type { DocumentSummary } from '$lib/types';

	let articles = $state<DocumentSummary[]>([]);
	let loading = $state(true);
	let error = $state('');

	function formatDate(ms: number | null): string {
		if (!ms) return '';
		return new Date(ms).toLocaleDateString();
	}

	$effect(() => {
		api<DocumentSummary[]>('/articles')
			.then((a) => (articles = a))
			.catch((e) => (error = e instanceof Error ? e.message : 'Failed to load posts.'))
			.finally(() => (loading = false));
	});
</script>

<h1>OpenPublish</h1>
{#if loading}
	<p class="muted">Loading…</p>
{:else if error}
	<p class="error">{error}</p>
{:else if articles.length === 0}
	<p class="muted">No published posts yet.</p>
{:else}
	<ul class="posts">
		{#each articles as a (a.id)}
			<li>
				<a class="title" href={`/articles/${a.slug}`}>{a.title}</a>
				<div class="muted">{formatDate(a.published_at_ms)}</div>
			</li>
		{/each}
	</ul>
{/if}
