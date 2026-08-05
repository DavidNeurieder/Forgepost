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
