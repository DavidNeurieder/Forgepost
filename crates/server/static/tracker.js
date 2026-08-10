// Client-side analytics tracker (M2).
//
// Reports a view, banded scroll depth (25/50/75/100), one read event per page
// load, and per-block impressions from an IntersectionObserver. Events are
// sent to the public `/api/events` endpoint; the server mints the anonymous
// visitor cookie. All writes use `sendBeacon`/`keepalive` so they survive
// navigation.
(function () {
	'use strict';

	var BANDS = [25, 50, 75, 100];
	var MAX_READ_MS = 2 * 60 * 60 * 1000;
	var MIN_DWELL_MS = 3000;

	function postEvent(payload) {
		var body = JSON.stringify(payload);
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
			body: body,
			keepalive: true
		}).catch(function () {
			// Tracking is best-effort; never surface analytics errors to readers.
		});
	}

	function randomId() {
		if (typeof crypto !== 'undefined' && crypto.randomUUID) {
			return crypto.randomUUID();
		}
		return Date.now() + '-' + Math.random().toString(36).slice(2);
	}

	// Start tracking a published article. Call from the article page.
	function trackArticle(slug) {
		var sessionId = randomId();
		var startedAt = performance.now();
		var firedBands = new Set();
		var readFired = false;
		var reached100 = false;
		// Experiments this session has seen (block intersected) and which ones
		// have already converted for the `completion` goal (reached 100%).
		var experimentsSeen = new Map();
		var convertedExperiments = new Set();

		postEvent({ slug: slug, session_id: sessionId, kind: 'view', payload: {} });

		function scrollDepth() {
			var doc = document.documentElement;
			var max = doc.scrollHeight - window.innerHeight;
			if (max <= 0) return 1;
			return Math.min(1, Math.max(0, window.scrollY / max));
		}

		function fireConversions() {
			experimentsSeen.forEach(function (variantId, experimentId) {
				if (convertedExperiments.has(experimentId)) return;
				convertedExperiments.add(experimentId);
				postEvent({
					slug: slug,
					session_id: sessionId,
					kind: 'experiment_conversion',
					experiment_id: experimentId,
					variant_id: variantId,
					payload: {}
				});
			});
		}

		function fireRead() {
			if (readFired) return;
			readFired = true;
			var readTimeMs = Math.min(
				Math.max(0, performance.now() - startedAt),
				MAX_READ_MS
			);
			postEvent({
				slug: slug,
				session_id: sessionId,
				kind: 'article_read',
				payload: { read_time_ms: Math.round(readTimeMs) }
			});
		}

		function onScroll() {
			var depth = scrollDepth();
			for (var i = 0; i < BANDS.length; i++) {
				var band = BANDS[i];
				if (firedBands.has(band)) continue;
				if (depth >= band / 100) {
					firedBands.add(band);
					postEvent({
						slug: slug,
						session_id: sessionId,
						kind: 'banded_scroll',
						payload: { band: band }
					});
					if (band === 100) {
						reached100 = true;
						fireConversions();
					}
				}
			}
			if (
				reached100 &&
				!readFired &&
				performance.now() - startedAt >= MIN_DWELL_MS
			) {
				fireRead();
			}
		}

		// Impressions: fire once per block when a meaningful part is on screen.
		// Blocks that are experiment variants also report their impression (with
		// the assigned experiment/variant ids) so the engine can count samples.
		var observed = new Set();
		var observer = null;
		if (typeof IntersectionObserver !== 'undefined') {
			observer = new IntersectionObserver(
				function (entries) {
					for (var i = 0; i < entries.length; i++) {
						var entry = entries[i];
						if (!entry.isIntersecting) continue;
						var el = entry.target;
						var id = el.dataset.blockId;
						if (!id || observed.has(id)) continue;
						observed.add(id);
						postEvent({
							slug: slug,
							session_id: sessionId,
							kind: 'block_impression',
							block_id: id,
							payload: {}
						});
						var expId = el.dataset.experimentId;
						var variantId = el.dataset.variantId;
						if (expId && variantId && !experimentsSeen.has(expId)) {
							experimentsSeen.set(expId, variantId);
							postEvent({
								slug: slug,
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
			document.querySelectorAll('[data-block-id]').forEach(function (el) {
				observer.observe(el);
			});
		}

		// Fire scroll-depth bands as the reader scrolls, and ensure a read is
		// recorded even if the reader leaves after reaching the end.
		window.addEventListener('scroll', onScroll, { passive: true });
		function onLeave() {
			if (reached100 && performance.now() - startedAt >= MIN_DWELL_MS) {
				fireRead();
			}
		}
		document.addEventListener('visibilitychange', onLeave);
		window.addEventListener('pagehide', onLeave);

		onScroll();

		return {
			dispose: function () {
				if (observer) observer.disconnect();
				window.removeEventListener('scroll', onScroll);
				document.removeEventListener('visibilitychange', onLeave);
				window.removeEventListener('pagehide', onLeave);
			}
		};
	}

	window.ForgepostTracker = { trackArticle: trackArticle };
})();
