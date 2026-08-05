import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import Stats from './+page.svelte';
import { api, currentSession } from '$lib/api';
import { session } from '../../../../test/fixtures';
import { waitForFlush } from '../../../../test/helpers';

vi.mock('$lib/api', async (importOriginal) => {
	const mod = await importOriginal<typeof import('$lib/api')>();
	return { ...mod, api: vi.fn(), currentSession: vi.fn() };
});

const mockedApi = vi.mocked(api);
const mockedSession = vi.mocked(currentSession);

const stats = {
	article: {
		views: 42,
		unique_readers: 17,
		avg_read_time_ms: 90_000,
		read_events: 5,
		completion: 0.5,
		band_reach: [
			{ band: 25, pageviews: 30 },
			{ band: 50, pageviews: 20 },
			{ band: 75, pageviews: 10 },
			{ band: 100, pageviews: 5 }
		]
	},
	blocks: [
		{
			block_id: 'bl1',
			position: 0,
			kind: 'Paragraph',
			preview: 'hello',
			impressions: 10,
			estimated_reach: 40,
			estimated_dropoff: 2,
			estimates: true
		},
		{
			block_id: 'bl2',
			position: 1,
			kind: 'Code',
			preview: 'let x',
			impressions: 3,
			estimated_reach: 20,
			estimated_dropoff: 20,
			estimates: true
		}
	]
};

function experiment(overrides: Record<string, unknown> = {}) {
	return {
		id: 'e1',
		document_id: 'doc-1',
		block_id: 'bl1',
		name: 'Headline test',
		status: 'running',
		goal: 'completion',
		traffic_weight: 50,
		confidence_threshold: 0.95,
		min_sample_per_variant: 5,
		no_winner_prob: 0.05,
		max_duration_ms: 2_592_000_000,
		started_at_ms: 1_000,
		decided_at_ms: null,
		decision: null,
		winning_variant_id: null,
		created_at_ms: 1,
		variants: [
			{ id: 'vc', block_id: 'bl1', version_id: 'vx1', weight: 50, is_control: true },
			{ id: 'v1', block_id: 'bl1', version_id: 'vx2', weight: 50, is_control: false }
		],
		report: {
			variants: [
				{
					variant_id: 'vc',
					is_control: true,
					impressions: 4,
					conversions: 2,
					conversion_rate: 0.5,
					posterior_mean: 0.5,
					credible_interval: [0.1, 0.9],
					prob_beats_control: null
				},
				{
					variant_id: 'v1',
					is_control: false,
					impressions: 4,
					conversions: 2,
					conversion_rate: 0.5,
					posterior_mean: 0.5,
					credible_interval: [0.1, 0.9],
					prob_beats_control: 0.6
				}
			],
			recommendation: { type: 'continue' },
			n_looks: 1,
			adjusted_confidence_threshold: 0.95,
			elapsed_ms: 60_000
		},
		decisions: [],
		...overrides
	};
}

function mockRoutes(experiments: unknown[]) {
	mockedSession.mockResolvedValue(session);
	mockedApi.mockImplementation((path: string, opts?: { method?: string }) => {
		if (path === '/documents/doc-1/stats') return Promise.resolve(stats);
		if (path === '/documents/doc-1/experiments') return Promise.resolve(experiments);
		if (path?.startsWith('/experiments/') && opts?.method === 'POST') return Promise.resolve({});
		return Promise.reject(new Error('unexpected call: ' + path));
	});
}

function renderStats() {
	return render(Stats, { props: { params: { id: 'doc-1' } } });
}

beforeEach(() => {
	mockedApi.mockClear();
	mockedSession.mockClear();
});

describe('stats page', () => {
	it('renders the article analytics cards', async () => {
		mockRoutes([experiment()]);
		renderStats();

		await waitForFlush(() => expect(screen.getByText('42')).toBeInTheDocument());
		expect(screen.getByText('17')).toBeInTheDocument();
		expect(screen.getByText('1m 30s')).toBeInTheDocument();
		expect(screen.getAllByText('50%').length).toBeGreaterThan(0);
		expect(screen.getByText('Views (estimated)')).toBeInTheDocument();
	});

	it('renders block-level drop-off rows', async () => {
		mockRoutes([]);
		renderStats();

		await waitForFlush(() => expect(screen.getByText('hello')).toBeInTheDocument());
		expect(screen.getByText('Paragraph')).toBeInTheDocument();
		expect(screen.getByText('Code')).toBeInTheDocument();
	});

	it('renders a running experiment with its live report', async () => {
		mockRoutes([experiment()]);
		renderStats();

		await waitForFlush(() => expect(screen.getByText('Headline test')).toBeInTheDocument());
		expect(screen.getByText('running')).toBeInTheDocument();
		expect(screen.getByText('Variant 1')).toBeInTheDocument();
		expect(screen.getByText('Control (current)')).toBeInTheDocument();
		expect(screen.getByText('60%')).toBeInTheDocument();
		expect(screen.getByText(/Collecting data… 1 look/)).toBeInTheDocument();
	});

	it('acts on a running experiment', async () => {
		mockRoutes([experiment()]);
		renderStats();
		await waitForFlush(() => expect(screen.getByText('Headline test')).toBeInTheDocument());

		await fireEvent.click(screen.getByRole('button', { name: 'Decide now' }));

		await waitForFlush(() =>
			expect(mockedApi).toHaveBeenCalledWith('/experiments/e1/decide', { method: 'POST' })
		);
	});

	it('starts a draft experiment', async () => {
		mockRoutes([
			experiment({
				id: 'e2',
				name: 'Draft test',
				status: 'draft',
				started_at_ms: null,
				report: null
			})
		]);
		renderStats();
		await waitForFlush(() => expect(screen.getByText('Draft test')).toBeInTheDocument());

		await fireEvent.click(screen.getByRole('button', { name: 'Start experiment' }));

		await waitForFlush(() =>
			expect(mockedApi).toHaveBeenCalledWith('/experiments/e2/start', { method: 'POST' })
		);
	});

	it('creates an experiment with the configured block, traffic and variant', async () => {
		mockRoutes([]);
		renderStats();
		await waitForFlush(() =>
			expect(screen.getByText('No experiments yet — create one below.')).toBeInTheDocument()
		);

		const summary = screen.getByText('Create experiment', { selector: 'summary' });
		const details = summary.closest('details');
		expect(details).not.toBeNull();
		details!.open = true;

		await fireEvent.input(screen.getByPlaceholderText('e.g. New headline'), {
			target: { value: 'Bold headline' }
		});
		await fireEvent.input(screen.getByLabelText(/Variant 1 content/), {
			target: { value: 'Bold new text' }
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Create experiment' }));

		await waitForFlush(() =>
			expect(mockedApi).toHaveBeenCalledWith('/experiments', {
				method: 'POST',
				body: {
					document_id: 'doc-1',
					block_id: 'bl1',
					name: 'Bold headline',
					traffic_weight: 100,
					variants: [{ content: { text: 'Bold new text' }, weight: 50 }]
				}
			})
		);
	});
});
