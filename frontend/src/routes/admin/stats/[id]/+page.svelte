<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, ApiError, currentSession } from '$lib/api';
	import type { BlockStat, DocumentStatsView } from '$lib/types';

	let { params } = $props();

	let stats = $state<DocumentStatsView | null>(null);
	let loading = $state(true);
	let error = $state('');

	function formatDuration(ms: number | null): string {
		if (ms == null) return '—';
		if (ms < 1000) return `${ms} ms`;
		const s = Math.round(ms / 1000);
		return `${Math.floor(s / 60)}m ${s % 60}s`;
	}

	function formatPct(v: number | null): string {
		if (v == null) return '—';
		return `${(v * 100).toFixed(0)}%`;
	}

	function maxDropoff(): number {
		const drops = stats?.blocks.map((b) => b.estimated_dropoff) ?? [];
		return Math.max(1, ...drops);
	}

	$effect(() => {
		currentSession()
			.then(() => {
				const id = params.id;
				void api<DocumentStatsView>(`/documents/${id}/stats`)
					.then((s) => {
						stats = s;
					})
					.catch((e) => {
						if (e instanceof ApiError && e.status === 401) goto('/login');
						else error = e instanceof Error ? e.message : 'Could not load stats.';
					})
					.finally(() => (loading = false));
			})
			.catch((e) => {
				if (e instanceof ApiError && e.status === 401) goto('/login');
			});
	});
</script>

<svelte:head>
	<title>Analytics · OpenPublish</title>
</svelte:head>

<div style="display:flex;align-items:baseline;gap:1rem;flex-wrap:wrap;">
	<h1 style="margin-bottom:0.25rem;">Analytics</h1>
	<a class="button secondary small" href="/admin">← Dashboard</a>
</div>
<p class="muted" style="margin-top:0.4rem;">
	Numbers marked “estimated” are inferred from scroll depth and undercount
	readers with ad-blockers or JavaScript disabled.
</p>

{#if loading}
	<p class="muted">Loading…</p>
{:else if error}
	<p class="error">{error}</p>
{:else if stats}
	<h2>Article</h2>
	<div class="stat-grid">
		<div class="stat-card">
			<div class="stat-value">{stats.article.views}</div>
			<div class="stat-label">Views (estimated)</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{stats.article.unique_readers}</div>
			<div class="stat-label">Unique readers (estimated)</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{formatDuration(stats.article.avg_read_time_ms)}</div>
			<div class="stat-label">Average reading time</div>
		</div>
		<div class="stat-card">
			<div class="stat-value">{formatPct(stats.article.completion)}</div>
			<div class="stat-label">Completed (scrolled to end)</div>
		</div>
	</div>

	<h2 style="margin-top:2rem;">Scroll depth</h2>
	<div class="funnel">
		{#each stats.article.band_reach as band (band.band)}
			<div class="funnel-row">
				<span class="funnel-label">Past {band.band}%</span>
				<div class="funnel-bar">
					<div
						class="funnel-fill"
						style:width={stats.article.views > 0
							? `${Math.round((band.pageviews / stats.article.views) * 100)}%`
							: '0%'}
					></div>
				</div>
				<span class="funnel-count">{band.pageviews}</span>
			</div>
		{/each}
	</div>

	<h2 style="margin-top:2rem;">Where readers leave</h2>
	{#if stats.blocks.length === 0}
		<p class="muted">No blocks in this article.</p>
	{:else}
		<table>
			<thead>
				<tr>
					<th>Block</th>
					<th>Reached (estimated)</th>
					<th>Drop-off</th>
					<th>Impressions</th>
				</tr>
			</thead>
			<tbody>
				{#each stats.blocks as b (b.block_id)}
					<tr>
						<td>
							<span class="badge">{b.kind}</span>
							<span class="muted" style="display:block;max-width:22rem;">
								{b.preview}
							</span>
						</td>
						<td>{b.estimated_reach}</td>
						<td>
							<div class="dropoff">
								<div
									class="dropoff-fill"
									style:width={`${Math.round((b.estimated_dropoff / maxDropoff()) * 100)}%`}
								></div>
								<span class="dropoff-count">{b.estimated_dropoff}</span>
							</div>
						</td>
						<td class="muted">{b.impressions}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
{/if}
