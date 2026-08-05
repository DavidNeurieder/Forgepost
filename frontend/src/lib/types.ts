export interface User {
	id: string;
	email: string;
	display_name: string;
	role: string;
}

export interface SessionResponse {
	user: User;
	csrf_token: string;
}

export interface BlockView {
	id: string;
	kind: string;
	content: Record<string, unknown>;
}

export interface DocumentView {
	id: string;
	title: string;
	slug: string;
	status: 'draft' | 'published';
	published_at_ms: number | null;
	updated_at_ms: number;
	tags: string[];
	blocks: BlockView[];
}

export interface ArticleView {
	id: string;
	title: string;
	slug: string;
	published_at_ms: number | null;
	updated_at_ms: number;
	tags: string[];
	blocks: BlockView[];
	html: string;
	rendered_blocks: RenderedBlock[];
}

export interface RenderedBlock {
	id: string;
	kind: string;
	html: string;
	experiment_id: string | null;
	variant_id: string | null;
}

export interface BandReach {
	band: number;
	pageviews: number;
}

export interface ArticleStats {
	views: number;
	unique_readers: number;
	avg_read_time_ms: number | null;
	read_events: number;
	completion: number | null;
	band_reach: BandReach[];
}

export interface BlockStat {
	block_id: string;
	position: number;
	kind: string;
	preview: string;
	impressions: number;
	estimated_reach: number;
	estimated_dropoff: number;
	is_estimate: boolean;
}

export interface DocumentStatsView {
	article: ArticleStats;
	blocks: BlockStat[];
}

export interface DocumentSummary {
	id: string;
	title: string;
	slug: string;
	status: string;
	published_at_ms: number | null;
	updated_at_ms: number;
}

export interface CommentView {
	id: string;
	document_id: string;
	author_name: string;
	body: string;
	status: string;
	created_at_ms: number;
}

export interface RenderView {
	html: string;
	blocks: { kind: string; content: Record<string, unknown> }[];
}

export interface ExperimentVariantView {
	id: string;
	block_id: string;
	version_id: string;
	weight: number;
	is_control: boolean;
}

export interface ExperimentDecisionView {
	id: string;
	decided_at_ms: number;
	decision: string;
	winner_variant_id: string | null;
	promoted_version_id: string | null;
	effect_size: number | null;
	confidence: number | null;
}

export interface VariantReport {
	variant_id: string;
	is_control: boolean;
	impressions: number;
	conversions: number;
	conversion_rate: number;
	posterior_mean: number;
	credible_interval: [number, number];
	prob_beats_control: number | null;
}

export type Recommendation =
	| { type: 'continue' }
	| { type: 'promote'; variant_id: string; confidence: number }
	| { type: 'no_winner' };

export interface ExperimentReport {
	variants: VariantReport[];
	recommendation: Recommendation;
	n_looks: number;
	adjusted_confidence_threshold: number;
	elapsed_ms: number;
}

export interface ExperimentView {
	id: string;
	document_id: string;
	block_id: string;
	name: string;
	status: 'draft' | 'running' | 'decided' | 'stopped';
	goal: string;
	traffic_weight: number;
	confidence_threshold: number;
	min_sample_per_variant: number;
	no_winner_prob: number;
	max_duration_ms: number;
	started_at_ms: number | null;
	decided_at_ms: number | null;
	decision: string | null;
	winning_variant_id: string | null;
	created_at_ms: number;
	variants: ExperimentVariantView[];
	report: ExperimentReport | null;
	decisions: ExperimentDecisionView[];
}
