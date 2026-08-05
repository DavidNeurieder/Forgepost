<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, ApiError, currentSession } from '$lib/api';
	import type {
		BlockStat,
		DocumentStatsView,
		ExperimentDecisionView,
		ExperimentView
	} from '$lib/types';

	let { params } = $props();

	let stats = $state<DocumentStatsView | null>(null);
	let loading = $state(true);
	let error = $state('');

	let experiments = $state<ExperimentView[]>([]);
	let experimentsLoaded = $state(false);

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

	function loadExperiments(id: string): void {
		void api<ExperimentView[]>(`/documents/${id}/experiments`)
			.then((list) => {
				experiments = list;
			})
			.catch((e) => {
				if (e instanceof ApiError && e.status === 401) goto('/login');
				else error = e instanceof Error ? e.message : 'Could not load experiments.';
			})
			.finally(() => (experimentsLoaded = true));
	}

	function blockLabel(blockId: string): string {
		const b = stats?.blocks.find((x) => x.block_id === blockId);
		return b ? `${b.kind} — ${b.preview}` : blockId.slice(0, 8);
	}

	function experimentableBlocks(): BlockStat[] {
		return (stats?.blocks ?? []).filter((b) =>
			['Paragraph', 'Heading', 'CallToAction'].some((k) => b.kind.startsWith(k))
		);
	}

	async function act(id: string, action: string): Promise<void> {
		await api(`/experiments/${id}/${action}`, { method: 'POST' });
		loadExperiments(params.id);
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
				loadExperiments(id);
			})
			.catch((e) => {
				if (e instanceof ApiError && e.status === 401) goto('/login');
			});
	});

	let creating = $state(false);
	let newName = $state('');
	let newBlockId = $state('');
	let newTrafficWeight = $state(100);
	let newVariants = $state<{ content: string; weight: number }[]>([{ content: '', weight: 50 }]);

	$effect(() => {
		if (!newBlockId && experimentableBlocks().length > 0) {
			newBlockId = experimentableBlocks()[0].block_id;
		}
	});

	function addVariant(): void {
		newVariants = [...newVariants, { content: '', weight: 50 }];
	}

	function removeVariant(i: number): void {
		newVariants = newVariants.filter((_, idx) => idx !== i);
	}

	async function createExperiment(): Promise<void> {
		const variants = newVariants
			.filter((v) => v.content.trim() !== '')
			.map((v) => ({ content: { text: v.content.trim() }, weight: v.weight }));
		if (variants.length === 0) return;
		creating = true;
		try {
			await api('/experiments', {
				method: 'POST',
				body: {
					document_id: params.id,
					block_id: newBlockId,
					name: newName.trim(),
					traffic_weight: newTrafficWeight,
					variants
				}
			});
			newName = '';
			newVariants = [{ content: '', weight: 50 }];
			loadExperiments(params.id);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not create experiment.';
		} finally {
			creating = false;
		}
	}

	function statusClass(status: string): string {
		switch (status) {
			case 'running':
				return 'badge ok';
			case 'decided':
				return 'badge info';
			default:
				return 'badge';
		}
	}

	function recommendationText(exp: ExperimentView): string {
		const report = exp.report;
		const r = report?.recommendation;
		if (!r) return '';
		if (r.type === 'continue')
			return `Collecting data… ${report.n_looks} look(s) so far, threshold ${formatPct(
				report.adjusted_confidence_threshold
			)}.`;
		if (r.type === 'no_winner') return 'Variant is (near-)certain not to beat control.';
		return `Variant ${r.variant_id.slice(0, 8)} likely better (${formatPct(r.confidence)}).`;
	}

	function variantLabel(exp: ExperimentView, variantId: string): string {
		const v = exp.variants.find((x) => x.id === variantId);
		if (!v) return variantId.slice(0, 8);
		if (v.is_control) return 'Control (current)';
		return `Variant ${exp.variants.filter((x) => !x.is_control).findIndex((x) => x.id === variantId) + 1}`;
	}

	function decisionSummary(d: ExperimentDecisionView): string {
		switch (d.decision) {
			case 'promote':
				return `Promoted ${d.winner_variant_id ? d.winner_variant_id.slice(0, 8) : 'winner'}${
					d.confidence != null ? ` at ${formatPct(d.confidence)} confidence` : ''
				}${d.effect_size != null ? `, +${(d.effect_size * 100).toFixed(1)}%` : ''}`;
			case 'no_winner':
				return 'No improvement';
			case 'stop':
				return 'Stopped manually';
			default:
				return d.decision;
		}
	}

	function decisionDate(ms: number): string {
		return new Date(ms).toLocaleString();
	}
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

	<h2 style="margin-top:2rem;">Experiments</h2>
	<p class="muted">
		A/B test alternative content on a single block. Visitors are split
		between control (the current block) and your variants; the goal is
		reaching the end of the article. Decisions use a Bayesian sequential
		test that watches for early evidence of a winner.
	</p>

	{#if !experimentsLoaded}
		<p class="muted">Loading experiments…</p>
	{:else if experiments.length === 0}
		<p class="muted">No experiments yet — create one below.</p>
	{:else}
		{#each experiments as exp (exp.id)}
			<div class="exp-card">
				<div style="display:flex;align-items:baseline;gap:0.75rem;flex-wrap:wrap;">
					<h3 style="margin:0;">{exp.name || 'Untitled experiment'}</h3>
					<span class={statusClass(exp.status)}>{exp.status}</span>
					<span class="muted">Goal: {exp.goal}</span>
					<span class="muted">{exp.traffic_weight}% of visitors</span>
				</div>
				<p class="muted" style="margin-top:0.4rem;">{blockLabel(exp.block_id)}</p>

				{#if exp.status === 'draft'}
					<div style="display:flex;gap:0.5rem;">
						<button class="button small" onclick={() => act(exp.id, 'start')}>
							Start experiment
						</button>
						<button
							class="button secondary small"
							onclick={() => act(exp.id, 'stop')}
						>
							Delete
						</button>
					</div>
				{:else}
					<table>
						<thead>
							<tr>
								<th>Variant</th>
								<th>Impressions</th>
								<th>Conversions</th>
								<th>Conv. rate</th>
								<th>P(beats control)</th>
							</tr>
						</thead>
						<tbody>
							{#each exp.variants as v (v.id)}
								{@const report = exp.report?.variants.find((x) => x.variant_id === v.id)}
								{@const beats = report?.prob_beats_control ?? null}
								<tr>
									<td>
										{variantLabel(exp, v.id)}
										{#if v.is_control}
											<span class="muted" style="display:block;">{v.weight}% traffic</span>
										{:else}
											<span class="muted" style="display:block;">{v.weight}% of tested traffic</span>
										{/if}
									</td>
									<td>{report?.impressions ?? '—'}</td>
									<td>{report?.conversions ?? '—'}</td>
									<td>{formatPct(report?.conversion_rate ?? null)}</td>
									<td>
										{#if beats == null}
											—
										{:else}
											<div class="prob-bar">
												<div
													class="prob-fill"
													style:width={`${Math.round(beats * 100)}%`}
												></div>
											</div>
											{formatPct(beats)}
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>

					{#if exp.report && exp.status === 'running'}
						<p class="muted" style="margin:0.75rem 0;">
							Running for {formatDuration(exp.report.elapsed_ms)} ·
							{recommendationText(exp)}
						</p>
					{:else if exp.decision}
						<p class="muted" style="margin:0.75rem 0;">Decision: {exp.decision}</p>
					{/if}

					{#if exp.status === 'running'}
						<div style="display:flex;gap:0.5rem;flex-wrap:wrap;">
							<button class="button small" onclick={() => act(exp.id, 'decide')}>
								Decide now
							</button>
							<button
								class="button secondary small"
								onclick={() => act(exp.id, 'promote')}
							>
								Promote best
							</button>
							<button
								class="button secondary small"
								onclick={() => act(exp.id, 'no-winner')}
							>
								No improvement
							</button>
							<button
								class="button secondary small"
								onclick={() => act(exp.id, 'stop')}
							>
								Stop
							</button>
						</div>
					{/if}

					{#if exp.decisions.length > 0}
						<h4 style="margin:1rem 0 0.25rem;">Decision history</h4>
						<ul class="muted" style="margin:0;">
							{#each exp.decisions as d (d.id)}
								<li>
									{decisionDate(d.decided_at_ms)} — {decisionSummary(d)}
								</li>
							{/each}
						</ul>
					{/if}
				{/if}
			</div>
		{/each}
	{/if}

	{#if experimentableBlocks().length > 0}
		<details class="exp-card" style="margin-top:1.5rem;">
			<summary style="cursor:pointer;font-weight:600;">Create experiment</summary>
			<div style="margin-top:0.75rem;display:grid;gap:0.75rem;max-width:34rem;">
				<label>
					Block to test
					<select bind:value={newBlockId}>
						{#each experimentableBlocks() as b (b.block_id)}
							<option value={b.block_id}>{b.kind} — {b.preview}</option>
						{/each}
					</select>
				</label>
				<label>
					Name <span class="muted">(optional)</span>
					<input bind:value={newName} placeholder="e.g. New headline" />
				</label>
				<label>
					Visitors to test
					<input type="number" min="1" max="100" bind:value={newTrafficWeight} />
					<span class="muted">% — the rest always see control.</span>
				</label>
				{#each newVariants as variant, i (i)}
					<label>
						Variant {i + 1} content
						<textarea
							bind:value={variant.content}
							rows="2"
							placeholder="Replacement text for this block"
						></textarea>
						<div style="display:flex;gap:1rem;align-items:center;">
							<span class="muted" style="white-space:nowrap;">
								Traffic share
							</span>
							<input type="number" min="1" bind:value={variant.weight} style="max-width:5rem;" />
							<button
								class="button secondary small"
								onclick={() => removeVariant(i)}
								disabled={newVariants.length === 1}
							>
								Remove
							</button>
						</div>
					</label>
				{/each}
				<div style="display:flex;gap:0.5rem;flex-wrap:wrap;">
					<button class="button secondary small" onclick={addVariant}>Add variant</button>
					<button
						class="button"
						onclick={createExperiment}
						disabled={creating || newVariants.every((v) => v.content.trim() === '')}
					>
						{creating ? 'Creating…' : 'Create experiment'}
					</button>
				</div>
			</div>
		</details>
	{:else}
		<p class="muted">Add a heading, paragraph, or CTA block to run experiments.</p>
	{/if}
	<p class="muted" style="margin-top:0.5rem;">
		Note: promotion replaces the live block with the winning variant, so your
		article changes immediately. Stopping after “no improvement” keeps the
		current content.
	</p>
{/if}
