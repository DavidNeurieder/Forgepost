// Client-side analytics tracker (M2).
//
// Reports a view, banded scroll depth (25/50/75/100), one read event per page
// load, and per-block impressions from an IntersectionObserver. Events are
// sent to the public `/api/events` endpoint; the server mints the anonymous
// visitor cookie. All writes use `sendBeacon`/`keepalive` so they survive
// navigation.

export interface Tracker {
	dispose(): void;
}

const BANDS = [25, 50, 75, 100] as const;
const MAX_READ_MS = 2 * 60 * 60 * 1000;
const MIN_DWELL_MS = 3_000;

function postEvent(payload: Record<string, unknown>): void {
	const body = JSON.stringify(payload);
	if (typeof navigator !== 'undefined' && navigator.sendBeacon) {
		if (
			navigator.sendBeacon(
				'/api/events',
				new Blob([body], { type: 'application/json' })
			)
		) {
			return;
		}
	}
	fetch('/api/events', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body,
		keepalive: true
	}).catch(() => {
		// Tracking is best-effort; never surface analytics errors to readers.
	});
}

/** Start tracking a published article. Call from the article page on mount. */
export function trackArticle(slug: string): Tracker {
	const sessionId =
		typeof crypto !== 'undefined' && 'randomUUID' in crypto
			? crypto.randomUUID()
			: `${Date.now()}-${Math.random().toString(36).slice(2)}`;

	const startedAt = performance.now();
	const firedBands = new Set<number>();
	let readFired = false;
	let reached100 = false;
	// Experiments this session has seen (block intersected) and which ones have
	// already converted for the `completion` goal (reached 100%).
	const experimentsSeen = new Map<string, string>();
	const convertedExperiments = new Set<string>();

	postEvent({ slug, session_id: sessionId, kind: 'view', payload: {} });

	function scrollDepth(): number {
		const doc = document.documentElement;
		const max = doc.scrollHeight - window.innerHeight;
		if (max <= 0) return 1;
		return Math.min(1, Math.max(0, window.scrollY / max));
	}

	function fireConversions(): void {
		for (const [experimentId, variantId] of experimentsSeen) {
			if (convertedExperiments.has(experimentId)) continue;
			convertedExperiments.add(experimentId);
			postEvent({
				slug,
				session_id: sessionId,
				kind: 'experiment_conversion',
				experiment_id: experimentId,
				variant_id: variantId,
				payload: {}
			});
		}
	}

	function fireRead(): void {
		if (readFired) return;
		readFired = true;
		const readTimeMs = Math.min(
			Math.max(0, performance.now() - startedAt),
			MAX_READ_MS
		);
		postEvent({
			slug,
			session_id: sessionId,
			kind: 'article_read',
			payload: { read_time_ms: Math.round(readTimeMs) }
		});
	}

	function onScroll(): void {
		const depth = scrollDepth();
		for (const band of BANDS) {
			if (firedBands.has(band)) continue;
			if (depth >= band / 100) {
				firedBands.add(band);
				postEvent({
					slug,
					session_id: sessionId,
					kind: 'banded_scroll',
					payload: { band }
				});
				if (band === 100) {
					reached100 = true;
					fireConversions();
				}
			}
		}
		if (reached100 && !readFired && performance.now() - startedAt >= MIN_DWELL_MS) {
			fireRead();
		}
	}

	// Impressions: fire once per block when a meaningful part is on screen.
	// Blocks that are experiment variants also report their impression (with the
	// assigned experiment/variant ids) so the engine can count sample sizes.
	const observed = new Set<string>();
	const observer =
		typeof IntersectionObserver === 'undefined'
			? null
			: new IntersectionObserver(
					(entries) => {
						for (const entry of entries) {
							if (!entry.isIntersecting) continue;
							const el = entry.target as HTMLElement;
							const id = el.dataset.blockId;
							if (!id || observed.has(id)) continue;
							observed.add(id);
							postEvent({
								slug,
								session_id: sessionId,
								kind: 'block_impression',
								block_id: id,
								payload: {}
							});
							const expId = el.dataset.experimentId;
							const variantId = el.dataset.variantId;
							if (expId && variantId && !experimentsSeen.has(expId)) {
								experimentsSeen.set(expId, variantId);
								postEvent({
									slug,
									session_id: sessionId,
									kind: 'experiment_impression',
									experiment_id: expId,
									variant_id: variantId,
									payload: {}
								});
							}
						}
					},
					{ threshold: 0.25 }
				);

	if (observer) {
		for (const el of document.querySelectorAll<HTMLElement>('[data-block-id]')) {
			observer.observe(el);
		}
	}

	// Fire scroll-depth bands as the reader scrolls, and ensure a read is
	// recorded even if the reader leaves after reaching the end.
	window.addEventListener('scroll', onScroll, { passive: true });
	const onLeave = (): void => {
		if (reached100 && performance.now() - startedAt >= MIN_DWELL_MS) fireRead();
	};
	document.addEventListener('visibilitychange', onLeave);
	window.addEventListener('pagehide', onLeave);

	onScroll();

	return {
		dispose(): void {
			observer?.disconnect();
			window.removeEventListener('scroll', onScroll);
			document.removeEventListener('visibilitychange', onLeave);
			window.removeEventListener('pagehide', onLeave);
		}
	};
}
