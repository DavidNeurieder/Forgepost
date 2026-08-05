<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, ApiError, currentSession, logout } from '$lib/api';
	import type { CommentView, DocumentSummary, User } from '$lib/types';

	let user = $state<User | null>(null);
	let docs = $state<DocumentSummary[]>([]);
	let pending = $state<CommentView[]>([]);
	let loading = $state(true);
	let error = $state('');
	let creating = $state(false);

	function formatDate(ms: number | null): string {
		if (!ms) return '';
		return new Date(ms).toLocaleDateString();
	}

	$effect(() => {
		currentSession()
			.then((s) => {
				user = s.user;
				void loadAll();
			})
			.catch((e) => {
				if (e instanceof ApiError && e.status === 401) goto('/login');
				else error = 'Could not load dashboard.';
			});
	});

	async function loadAll() {
		loading = true;
		error = '';
		try {
			const [d, p] = await Promise.all([
				api<DocumentSummary[]>('/documents'),
				api<CommentView[]>('/comments/pending')
			]);
			docs = d;
			pending = p;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not load dashboard.';
		} finally {
			loading = false;
		}
	}

	async function newPost() {
		creating = true;
		error = '';
		try {
			const doc = await api<import('$lib/types').DocumentView>('/documents', {
				method: 'POST',
				body: { title: 'Untitled', tags: [] }
			});
			goto(`/admin/editor/${doc.id}`);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not create post.';
		} finally {
			creating = false;
		}
	}

	async function approve(id: string) {
		await api<void>(`/comments/${id}/approve`, { method: 'POST' });
		pending = pending.filter((c) => c.id !== id);
	}

	async function signOut() {
		await logout();
		goto('/login');
	}
</script>

<svelte:head>
	<title>Admin · OpenPublish</title>
</svelte:head>

<h1>Dashboard</h1>

{#if user}
	<div class="muted" style="margin-bottom:1.5rem;">
		Signed in as {user.display_name}
	</div>
{/if}

<div style="display:flex;gap:0.6rem;margin-bottom:1.5rem;">
	<button onclick={newPost} disabled={creating}>New post</button>
	<a class="button secondary" href="/api/rss" target="_blank" rel="noreferrer">RSS feed</a>
	<button class="secondary" onclick={signOut}>Log out</button>
</div>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if loading}
	<p class="muted">Loading…</p>
{:else}
	<h2>Posts</h2>
	{#if docs.length === 0}
		<p class="muted">No posts yet. Create your first one.</p>
	{:else}
		<table>
			<thead>
				<tr>
					<th>Title</th>
					<th>Status</th>
					<th>Updated</th>
					<th>Analytics</th>
				</tr>
			</thead>
			<tbody>
				{#each docs as d (d.id)}
					<tr>
						<td>
							<a href={`/admin/editor/${d.id}`}>{d.title}</a>
						</td>
						<td>
							<span class="badge {d.status}">{d.status}</span>
						</td>
						<td class="muted">{formatDate(d.updated_at_ms)}</td>
						<td>
							<a class="button small" href={`/admin/stats/${d.id}`}>Stats</a>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}

	<h2 style="margin-top:2rem;">Pending comments</h2>
	{#if pending.length === 0}
		<p class="muted">Nothing awaiting moderation.</p>
	{:else}
		{#each pending as c (c.id)}
			<div class="notice">
				<strong>{c.author_name}</strong>
				<span class="muted"> · on {c.document_id.slice(0, 8)}</span>
				<p style="margin:0.4rem 0 0.6rem;">{c.body}</p>
				<button onclick={() => approve(c.id)}>Approve</button>
			</div>
		{/each}
	{/if}
{/if}
