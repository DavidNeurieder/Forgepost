import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { trackArticle } from './tracker';

// Tracker needs no browser APIs beyond jsdom, but IntersectionObserver is not
// provided, so we install a fake that records observed targets and lets each
// test fire intersections deterministically.
class FakeIntersectionObserver {
	static instances: FakeIntersectionObserver[] = [];
	callback: IntersectionObserverCallback;
	targets = new Set<Element>();

	constructor(callback: IntersectionObserverCallback) {
		this.callback = callback;
		FakeIntersectionObserver.instances.push(this);
	}

	observe(target: Element) {
		this.targets.add(target);
	}

	unobserve(target: Element) {
		this.targets.delete(target);
	}

	disconnect() {
		this.targets.clear();
	}

	takeRecords() {
		return [];
	}

	intersect(target: Element, isIntersecting = true) {
		this.callback(
			[{ isIntersecting, target } as unknown as IntersectionObserverEntry],
			this as unknown as IntersectionObserver
		);
	}
}

type Event = Record<string, unknown>;

let sent: Event[];
let scrollRatio: number;

const MAX_READ_MS = 2 * 60 * 60 * 1000;

function setScroll(ratio: number) {
	scrollRatio = ratio;
	Object.defineProperty(window, 'innerHeight', { value: 500, configurable: true });
	Object.defineProperty(document.documentElement, 'scrollHeight', { value: 2000, configurable: true });
	const max = 1500;
	Object.defineProperty(window, 'scrollY', { value: max * ratio, configurable: true });
}

function scrollTo(ratio: number) {
	setScroll(ratio);
	window.dispatchEvent(new Event('scroll'));
}

function kinds() {
	return sent.map((e) => e.kind);
}

function addBlock(id: string, experimentId?: string, variantId?: string) {
	const el = document.createElement('div');
	el.dataset.blockId = id;
	if (experimentId) el.dataset.experimentId = experimentId;
	if (variantId) el.dataset.variantId = variantId;
	document.body.appendChild(el);
	return el;
}

beforeEach(() => {
	vi.useRealTimers();
	sent = [];
	scrollRatio = 0;
	setScroll(0);
	Object.defineProperty(globalThis.navigator, 'sendBeacon', {
		value: undefined,
		configurable: true
	});
	globalThis.fetch = vi.fn(async (_url: unknown, init?: RequestInit) => {
		sent.push(JSON.parse(String(init?.body ?? '{}')));
		return new Response(null, { status: 200 });
	});
	FakeIntersectionObserver.instances = [];
	(globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver =
		FakeIntersectionObserver;
	document.body.innerHTML = '';
});

afterEach(() => {
	document.body.innerHTML = '';
	vi.restoreAllMocks();
});

describe('trackArticle', () => {
	it('reports a view on mount with a session id', () => {
		const tracker = trackArticle('my-post');
		tracker.dispose();

		expect(sent.length).toBe(1);
		expect(sent[0].kind).toBe('view');
		expect(sent[0].slug).toBe('my-post');
		expect(sent[0].session_id).toBeTruthy();
		expect(sent[0].payload).toEqual({});
	});

	it('fires banded scroll depth once per band as the reader scrolls', () => {
		const tracker = trackArticle('my-post');
		scrollTo(0.3);
		scrollTo(0.55);
		scrollTo(0.76);
		scrollTo(1);
		scrollTo(1);
		scrollTo(0.2);
		tracker.dispose();

		expect(kinds()).toEqual(['view', 'banded_scroll', 'banded_scroll', 'banded_scroll', 'banded_scroll']);
		const bands = sent.filter((e) => e.kind === 'banded_scroll').map((e) => e.payload);
		expect(bands).toEqual([{ band: 25 }, { band: 50 }, { band: 75 }, { band: 100 }]);
	});

	it('records a read only after the reader reaches the end and dwells', () => {
		const now = vi.spyOn(performance, 'now');
		now.mockReturnValue(0);
		const tracker = trackArticle('my-post');

		now.mockReturnValue(100);
		scrollTo(1); // reaches 100% but dwelled too briefly
		expect(sent.some((e) => e.kind === 'article_read')).toBe(false);

		now.mockReturnValue(10_000);
		document.dispatchEvent(new Event('visibilitychange'));
		tracker.dispose();

		const read = sent.find((e) => e.kind === 'article_read');
		expect(read).toBeDefined();
		expect((read!.payload as Record<string, unknown>).read_time_ms).toBe(10_000);
	});

	it('clamps the read time to the maximum', () => {
		const now = vi.spyOn(performance, 'now');
		now.mockReturnValue(0);
		const tracker = trackArticle('my-post');

		now.mockReturnValue(Number.MAX_SAFE_INTEGER);
		scrollTo(1);
		document.dispatchEvent(new Event('visibilitychange'));
		tracker.dispose();

		const read = sent.find((e) => e.kind === 'article_read');
		expect((read!.payload as Record<string, unknown>).read_time_ms).toBe(MAX_READ_MS);
	});

	it('reports a single read even if the reader leaves repeatedly', () => {
		const now = vi.spyOn(performance, 'now');
		now.mockReturnValue(0);
		const tracker = trackArticle('my-post');

		now.mockReturnValue(5000);
		scrollTo(1);
		document.dispatchEvent(new Event('visibilitychange'));
		document.dispatchEvent(new Event('visibilitychange'));
		tracker.dispose();

		expect(sent.filter((e) => e.kind === 'article_read').length).toBe(1);
	});

	it('reports block impressions from the observer, once per block', () => {
		const tracker = trackArticle('my-post');
		const observer = FakeIntersectionObserver.instances[0];
		const el = addBlock('block-1');
		observer.intersect(el);
		observer.intersect(el);
		tracker.dispose();

		const impressions = sent.filter((e) => e.kind === 'block_impression');
		expect(impressions.length).toBe(1);
		expect(impressions[0].block_id).toBe('block-1');
	});

	it('reports experiment impressions and converts at 100% scroll', () => {
		const tracker = trackArticle('my-post');
		const observer = FakeIntersectionObserver.instances[0];
		const el = addBlock('block-exp', 'exp-1', 'variant-2');
		observer.intersect(el);
		scrollTo(1);
		tracker.dispose();

		const impressions = sent.filter((e) => e.kind === 'experiment_impression');
		expect(impressions).toHaveLength(1);
		expect(impressions[0].experiment_id).toBe('exp-1');
		expect(impressions[0].variant_id).toBe('variant-2');

		const conversions = sent.filter((e) => e.kind === 'experiment_conversion');
		expect(conversions).toHaveLength(1);
		expect(conversions[0].experiment_id).toBe('exp-1');
		expect(conversions[0].variant_id).toBe('variant-2');
	});

	it('does not convert the same experiment twice', () => {
		const tracker = trackArticle('my-post');
		const observer = FakeIntersectionObserver.instances[0];
		const el = addBlock('block-exp', 'exp-1', 'variant-2');
		observer.intersect(el);
		scrollTo(1);
		scrollTo(1);
		observer.intersect(el);
		tracker.dispose();

		expect(sent.filter((e) => e.kind === 'experiment_conversion')).toHaveLength(1);
	});

	it('dispose stops listening to scroll', () => {
		const tracker = trackArticle('my-post');
		tracker.dispose();
		scrollTo(0.5);
		expect(kinds()).toEqual(['view']);
	});

	it('uses sendBeacon when available', () => {
		const beacon = vi.fn(() => true);
		Object.defineProperty(globalThis.navigator, 'sendBeacon', { value: beacon, configurable: true });
		const tracker = trackArticle('my-post');
		tracker.dispose();

		expect(beacon).toHaveBeenCalled();
		expect(globalThis.fetch).not.toHaveBeenCalled();
	});
});
