import { expect, test, type Browser } from '@playwright/test';

// Full creator journey through the real UI against the real `forgepost`
// server: setup -> write/publish -> external read + comment -> moderation ->
// analytics -> experiments -> logout/login. Runs serially because later steps
// depend on earlier ones mutating shared state, and shares the owner session
// (saved to .auth.json after setup) since Playwright gives each test a fresh
// browser context.

const AUTH_FILE = '.auth.json';
const EMAIL = 'e2e@example.com';
const PASSWORD = 'correct horse battery staple';
const DISPLAY = 'E2E Owner';
const TITLE = 'Hello from E2E';
const MARKDOWN = [
	'# Hello from E2E',
	'',
	'This is the first paragraph of the post.',
	'',
	'## Section two',
	'',
	'More content lives here.',
	'',
	'- **Fast:** built with Rust',
	'- **Simple:** one binary',
	''
].join('\n');

// A post with a standalone YouTube URL line -> a click-to-load video block.
const VIDEO_MARKDOWN = [
	'# Video E2E Post',
	'',
	'Some words before the embed.',
	'',
	'https://www.youtube.com/watch?v=dQw4w9WgXcQ',
	'',
	'Some words after the embed.',
	''
].join('\n');

test.describe.configure({ mode: 'serial' });

let slug = '';
let docId = '';

async function adminPage(browser: Browser) {
	const context = await browser.newContext({ storageState: AUTH_FILE });
	return context.newPage();
}

async function gotoDashboard(page: import('@playwright/test').Page) {
	await page.goto('/admin');
	await expect(page.getByText(`Signed in as ${DISPLAY}`)).toBeVisible();
}

test('first-run setup creates the owner account', async ({ page }) => {
	await page.goto('/setup');
	await expect(page.getByLabel('Email', { exact: true })).toBeVisible();

	await page.locator('#email').fill(EMAIL);
	await page.locator('#display').fill(DISPLAY);
	await page.locator('#password').fill(PASSWORD);
	await page.locator('#confirm').fill(PASSWORD);
	await page.getByRole('button', { name: 'Create account' }).click();

	await expect(page).toHaveURL(/\/admin$/);
	await expect(page.getByText(`Signed in as ${DISPLAY}`)).toBeVisible();

	await page.context().storageState({ path: AUTH_FILE });
});

test('the owner enables comments in settings', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.goto('/admin/settings');
	await page.locator('#comments_enabled').check();
	await page.getByRole('button', { name: 'Save settings' }).click();
	await expect(page.getByText('Settings saved.')).toBeVisible();
	await expect(page.locator('#comments_enabled')).toBeChecked();

	await page.context().close();
});

test('the owner creates, saves, and publishes a post', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.getByRole('button', { name: 'New post' }).click();
	await page.waitForURL(/\/admin\/editor\//);
	const editorUrl = new URL(page.url());

	// Save happens as a form POST, then the redirect re-renders the editor.
	await page.getByLabel('Title', { exact: true }).fill(TITLE);
	await page.getByLabel('Markdown').fill(MARKDOWN);
	await page.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(page.getByText('Saved')).toBeVisible();

	// The editor is still a draft until published.
	await expect(page.locator('.badge')).toHaveText('draft');

	// Publish, then the public URL appears as a "View post" link.
	await page.getByRole('button', { name: 'Publish', exact: true }).click();
	await expect(page.getByText('Published', { exact: true })).toBeVisible();
	await expect(page.locator('.badge')).toHaveText('published');

	const viewPost = page.getByRole('link', { name: 'View post' });
	await expect(viewPost).toBeVisible();
	slug = (await viewPost.getAttribute('href'))!.replace(/^\/articles\//, '');
	docId = editorUrl.pathname.split('/').pop() ?? '';
	expect(slug).not.toBe('');
	expect(slug).not.toBe('untitled');
	expect(docId).not.toBe('');
	await page.context().close();
});

test('the owner uploads an image and it renders in the article', async ({ browser }) => {
	expect(docId).not.toBe('');
	const fs = await import('fs');
	const os = await import('os');
	const path = await import('path');

	// A real PNG on disk (magic bytes only; the server never decodes it).
	const pngPath = path.join(os.tmpdir(), 'e2e-upload.png');
	fs.writeFileSync(pngPath, Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));

	const page = await adminPage(browser);
	await gotoDashboard(page);
	await page.goto(`/admin/editor/${docId}`);
	await expect(page.locator('#markdown')).toBeVisible();

	await page.locator('#media-input').setInputFiles(pngPath);

	// The upload inserts `![alt](/media/<uuid>.png)` at the cursor.
	await expect.poll(() => page.locator('#markdown').inputValue()).toContain('![alt](/media/');
	const markdown = await page.locator('#markdown').inputValue();
	const match = markdown.match(/!\[alt\]\((\/media\/[a-f0-9-]+\.png)\)/);
	expect(match).not.toBeNull();
	const mediaUrl = match![1];

	// The media endpoint serves the uploaded bytes as a PNG, without auth.
	const resp = await page.request.get(mediaUrl);
	expect(resp.status()).toBe(200);
	expect(resp.headers()['content-type']).toBe('image/png');

	// The live preview renders the image block.
	await expect(page.locator('#preview-body img')).toHaveAttribute('src', mediaUrl);

	// Save, then the published article shows the image too.
	await page.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(page.getByText('Saved')).toBeVisible();
	await page.goto(`/articles/${slug}`);
	await expect(page.locator('.article-body img')).toHaveAttribute('src', mediaUrl);
	await page.context().close();
});

// CRC-32 and a minimal stored-only zip writer, so the import test can hand a
// real .zip (post.md + images/) to the dashboard without external tooling.
function crc32(buf: Buffer): number {
	let c = 0xffffffff;
	for (let i = 0; i < buf.length; i++) {
		c ^= buf[i];
		for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
	}
	return (c ^ 0xffffffff) >>> 0;
}

function buildZip(entries: { name: string; data: Buffer }[]): Buffer {
	const parts: Buffer[] = [];
	const central: Buffer[] = [];
	let offset = 0;
	for (const { name, data } of entries) {
		const nameBuf = Buffer.from(name, 'utf8');
		const crc = crc32(data);
		const local = Buffer.alloc(30);
		local.writeUInt32LE(0x04034b50, 0);
		local.writeUInt16LE(20, 4);
		local.writeUInt16LE(0, 6);
		local.writeUInt16LE(0, 8);
		local.writeUInt16LE(0, 10);
		local.writeUInt16LE(0x21, 12);
		local.writeUInt32LE(crc, 14);
		local.writeUInt32LE(data.length, 18);
		local.writeUInt32LE(data.length, 22);
		local.writeUInt16LE(nameBuf.length, 26);
		local.writeUInt16LE(0, 28);
		parts.push(local, nameBuf, data);
		const cd = Buffer.alloc(46);
		cd.writeUInt32LE(0x02014b50, 0);
		cd.writeUInt16LE(20, 4);
		cd.writeUInt16LE(20, 6);
		cd.writeUInt16LE(0, 8);
		cd.writeUInt16LE(0, 10);
		cd.writeUInt16LE(0x21, 12);
		cd.writeUInt16LE(0x21, 14);
		cd.writeUInt32LE(crc, 16);
		cd.writeUInt32LE(data.length, 20);
		cd.writeUInt32LE(data.length, 24);
		cd.writeUInt16LE(nameBuf.length, 28);
		cd.writeUInt16LE(0, 30);
		cd.writeUInt16LE(0, 32);
		cd.writeUInt16LE(0, 34);
		cd.writeUInt16LE(0, 36);
		cd.writeUInt32LE(0, 38);
		cd.writeUInt32LE(offset, 42);
		central.push(cd, nameBuf);
		offset += 30 + nameBuf.length + data.length;
	}
	const cdBuf = Buffer.concat(central);
	const eocd = Buffer.alloc(22);
	eocd.writeUInt32LE(0x06054b50, 0);
	eocd.writeUInt16LE(0, 4);
	eocd.writeUInt16LE(0, 6);
	eocd.writeUInt16LE(entries.length, 8);
	eocd.writeUInt16LE(entries.length, 10);
	eocd.writeUInt32LE(cdBuf.length, 12);
	eocd.writeUInt32LE(offset, 16);
	eocd.writeUInt16LE(0, 20);
	return Buffer.concat([...parts, cdBuf, eocd]);
}

test('the owner imports a zip post with images as a draft', async ({ browser }) => {
	const fs = await import('fs');
	const os = await import('os');
	const path = await import('path');

	const zipPath = path.join(os.tmpdir(), 'e2e-import.zip');
	const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
	fs.writeFileSync(
		zipPath,
		buildZip([
			{ name: 'post.md', data: Buffer.from('---\ntitle: Imported by E2E\ntags: imported\n---\n\n# Imported by E2E\n\n![E2E image](images/photo.png)\n') },
			{ name: 'images/photo.png', data: png }
		])
	);

	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.locator('#import-input').setInputFiles(zipPath);
	await page.getByRole('button', { name: 'Import post' }).click();
	await page.waitForURL(/\/admin\/editor\/.+flash=imported/);

	await expect(page.getByText('Imported draft created — review it before publishing.')).toBeVisible();
	await expect(page.locator('.badge')).toHaveText('draft');
	await expect(page.getByLabel('Title', { exact: true })).toHaveValue('Imported by E2E');
	await expect(page.getByLabel('Tags')).toHaveValue('imported');

	const markdown = await page.locator('#markdown').inputValue();
	const match = markdown.match(/!\[E2E image\]\((\/media\/[a-f0-9-]+\.png)\)/);
	expect(match).not.toBeNull();
	const mediaUrl = match![1];

	const resp = await page.request.get(mediaUrl);
	expect(resp.status()).toBe(200);
	expect(resp.headers()['content-type']).toBe('image/png');
	await page.context().close();
});

test('external readers can view the article and leave a comment', async ({ browser }) => {
	expect(slug).not.toBe('');
	const context = await browser.newContext();
	const page = await context.newPage();

	await page.goto(`/articles/${slug}`);
	await expect(page.locator('h1').first()).toHaveText(TITLE);
	await expect(page.locator('[data-block-id]').first()).toBeVisible();

	// Markdown lists render as real `<li>` bullets with inline bold.
	await expect(page.locator('.article-body li').first()).toContainText('built with Rust');
	await expect(page.locator('.article-body li strong').first()).toHaveText('Fast:');

	await page.getByLabel('Name', { exact: true }).fill('Reader');
	await page.getByLabel('Comment', { exact: true }).fill('Nice post!');
	await page.getByRole('button', { name: 'Post comment' }).click();
	await expect(page.getByText('Thanks! Your comment is awaiting moderation.')).toBeVisible();

	await context.close();
});

test('readers can search and open a result', async ({ browser }) => {
	expect(slug).not.toBe('');
	const context = await browser.newContext();
	const page = await context.newPage();

	await page.goto('/');
	await page.getByRole('searchbox', { name: 'Search posts' }).fill('paragraph');
	await page.getByRole('searchbox', { name: 'Search posts' }).press('Enter');
	await expect(page).toHaveURL(/\/search\?q=paragraph/);
	await expect(page.getByRole('link', { name: TITLE })).toBeVisible();
	await expect(page.locator('.search-hit .snippet')).toContainText('first paragraph');

	await page.getByRole('link', { name: TITLE }).click();
	await expect(page.locator('h1').first()).toHaveText(TITLE);
	await context.close();
});

test('the owner approves the comment', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);
	await expect(page.getByText('Reader')).toBeVisible();
	await page.getByRole('button', { name: 'Approve' }).click();
	await expect(page.getByText('Nothing awaiting moderation.')).toBeVisible();

	await page.goto(`/articles/${slug}`);
	await expect(page.getByText('Nice post!')).toBeVisible();
	await page.context().close();
});

test('analytics show the real tracker view events', async ({ browser }) => {
	expect(docId).not.toBe('');
	const page = await adminPage(browser);
	await page.goto(`/admin/stats/${docId}`);
	await expect(page.getByText('Views (estimated)')).toBeVisible();
	await expect(page.locator('.stat-value').first()).not.toHaveText('0');
	await page.context().close();
});

test('an experiment can be created, started, and concluded', async ({ browser }) => {
	expect(docId).not.toBe('');
	const page = await adminPage(browser);
	await page.goto(`/admin/stats/${docId}`);
	await expect(page.getByText('No experiments yet — create one below.')).toBeVisible();

	await page.locator('details.exp-card summary').click();
	await page.getByPlaceholder('e.g. New headline').fill('Headline test');
	await page.getByLabel('Variant 1 content').fill('A bolder headline');
	await page.getByRole('button', { name: 'Create experiment' }).click();
	await expect(page.getByText('Headline test')).toBeVisible();

	await page.getByRole('button', { name: 'Start experiment' }).click();
	await expect(page.getByText('running', { exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'No improvement' }).click();
	await expect(page.getByText('Decision: no_improvement')).toBeVisible();
	await page.context().close();
});

// A module-scope slug for the video post so the two video tests can share it.
let videoSlug = '';

test('a video plays through click-to-load', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.getByRole('button', { name: 'New post' }).click();
	await page.waitForURL(/\/admin\/editor\//);
	await page.getByLabel('Title', { exact: true }).fill('Video E2E Post');
	await page.getByLabel('Markdown').fill(VIDEO_MARKDOWN);
	await page.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(page.getByText('Saved')).toBeVisible();
	await page.getByRole('button', { name: 'Publish', exact: true }).click();
	await expect(page.getByText('Published', { exact: true })).toBeVisible();

	videoSlug = (await page.getByRole('link', { name: 'View post' }).getAttribute('href'))!.replace(/^\/articles\//, '');
	expect(videoSlug).not.toBe('');
	await page.context().close();

	// Public article: the click-to-load box is rendered and there is no iframe
	// before the reader interacts.
	const reader = await browser.newContext();
	const rpage = await reader.newPage();
	await rpage.goto(`/articles/${videoSlug}`);
	const box = rpage.locator('button.video-box');
	await expect(box).toBeVisible();
	await expect(box).toHaveAttribute(
		'data-src',
		'https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ'
	);
	await expect(rpage.locator('.article-body iframe')).toHaveCount(0);

	// One click swaps in the player iframe with the privacy-host embed and the
	// referrerpolicy YouTube requires — no-referrer would trigger Error 153.
	await box.click();
	const frame = rpage.locator('.article-body iframe.video-frame');
	await expect(frame).toBeVisible();
	await expect(frame).toHaveAttribute(
		'src',
		'https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ'
	);
	await expect(frame).toHaveAttribute('referrerpolicy', 'strict-origin-when-cross-origin');
	expect(await frame.getAttribute('allowfullscreen')).not.toBeNull();
	await reader.close();
});

test('YouTube embeds boot and (on unblocked networks) really play', async ({ browser }) => {
	test.skip(!process.env.E2E_NETWORK, 'set E2E_NETWORK=1 to contact YouTube');
	expect(videoSlug).not.toBe('');

	const context = await browser.newContext({ locale: 'en-US' });
	const page = await context.newPage();
	await page.goto(`/articles/${videoSlug}`);
	await page.locator('button.video-box').click();

	const frameLocator = page.locator('.article-body iframe.video-frame');
	await expect(frameLocator).toBeVisible();
	const handle = await frameLocator.elementHandle();
	const yt = await handle!.contentFrame();
	expect(yt).not.toBeNull();

	// Drive the embed for up to ~60s. Outcomes:
	//  - Error 153 / "configuration error": OUR regression, always fail.
	//  - "Video unavailable": YouTube is bot-blocking this network/browser, so
	//    the test is inconclusive here and skips rather than turning red.
	//  - Player boots and the <video> clock advances: real YouTube playback.
	const errorOverlay = yt!.locator('.ytp-error-content, .ytp-error').first();
	let clickedPlay = false;
	for (let i = 0; i < 60; i++) {
		if ((await errorOverlay.isVisible().catch(() => false)) && (await errorOverlay.textContent().catch(() => ''))) {
			const text = (await errorOverlay.textContent()).trim();
			if (/153|configuration error/i.test(text)) {
				throw new Error(`YouTube reported player configuration error 153: "${text}"`);
			}
			if (/unavailable|verfügbar|not available/i.test(text)) {
				test.skip(true, `YouTube bot-blocks this environment ("${text}"); nothing to play back`);
				return;
			}
		}
		const played = await yt!
			.evaluate(() => {
				const v = document.querySelector('video');
				return v !== null && !v.paused && v.currentTime > 0.5;
			})
			.catch(() => false);
		if (played) {
			await context.close();
			return;
		}
		if (!clickedPlay) {
			const big = yt!.locator('.ytp-large-play-button');
			if (await big.isVisible().catch(() => false)) {
				await big.click().catch(() => {});
				clickedPlay = true;
			}
		}
		await page.waitForTimeout(1000);
	}
	await context.close();
	throw new Error('YouTube player booted but the timeline never advanced; playback did not start');
});

test('a video actually plays (self-hosted mp4 via click-to-load)', async ({ browser }) => {
	const fs = await import('node:fs');
	const http = await import('node:http');
	const path = await import('node:path');

	// Serve a tiny real mp4 (H.264 + AAC, committed fixture) from a throwaway
	// HTTP server so the assertion below proves *playback*, not just "an iframe
	// appeared". YouTube can't drive this: it refuses embeds in automated and
	// restricted networks ("This video is unavailable").
	const mp4 = fs.readFileSync(path.join(process.cwd(), 'fixtures', 'sample.mp4'));
	const media = http.createServer((req, res) => {
		const range = req.headers.range;
		const m = range ? /^bytes=(\d+)-(\d*)/.exec(range) : null;
		if (m) {
			const start = Number(m[1]);
			const end = m[2] ? Number(m[2]) : mp4.length - 1;
			res.writeHead(206, {
				'Content-Type': 'video/mp4',
				'Accept-Ranges': 'bytes',
				'Content-Range': `bytes ${start}-${end}/${mp4.length}`,
				'Content-Length': end - start + 1
			});
			res.end(mp4.subarray(start, end + 1));
			return;
		}
		res.writeHead(200, {
			'Content-Type': 'video/mp4',
			'Accept-Ranges': 'bytes',
			'Content-Length': mp4.length
		});
		res.end(mp4);
	});
	await new Promise<void>((resolve) => media.listen(0, '127.0.0.1', resolve));
	const mediaPort = (media.address() as { port: number }).port;

	// The standalone <iframe> line becomes a click-to-load video block whose
	// data-src is the local file, i.e. the exact same pipeline the demo uses.
	const src = `http://127.0.0.1:${mediaPort}/sample.mp4`;
	const admin = await adminPage(browser);
	await gotoDashboard(admin);
	await admin.getByRole('button', { name: 'New post' }).click();
	await admin.waitForURL(/\/admin\/editor\//);
	await admin.getByLabel('Title', { exact: true }).fill('Real Play E2E Post');
	await admin.getByLabel('Markdown').fill(
		['# Real Play E2E Post', '', 'Some words before the embed.', '', `<iframe src="${src}" title="E2E sample"></iframe>`, ''].join('\n')
	);
	await admin.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(admin.getByText('Saved')).toBeVisible();
	await admin.getByRole('button', { name: 'Publish', exact: true }).click();
	await expect(admin.getByText('Published', { exact: true })).toBeVisible();
	const realSlug = (await admin.getByRole('link', { name: 'View post' }).getAttribute('href'))!.replace(/^\/articles\//, '');
	expect(realSlug).not.toBe('');
	await admin.context().close();

	// Click the play button, then inspect the real <video> inside the frame.
	const reader = await browser.newContext();
	const page = await reader.newPage();
	await page.goto(`/articles/${realSlug}`);
	const box = page.locator('button.video-box');
	await expect(box).toBeVisible();
	await expect(box).toHaveAttribute('data-src', src);
	await box.click();

	const frame = page.locator('iframe.video-frame');
	await expect(frame).toBeVisible();
	await expect(frame).toHaveAttribute('src', src);
	const handle = await frame.elementHandle();
	const mediaDoc = await handle!.contentFrame();
	expect(mediaDoc).not.toBeNull();

	// The video element appears and the stream actually decodes (readyState
	// HAVE_CURRENT_DATA or better) — a real playable file, not a wood-frame
	// iframe shell.
	const decoded = () =>
		mediaDoc!.evaluate(() => {
			const v = document.querySelector('video');
			return v !== null && v.readyState >= 2;
		});
	await expect
		.poll(decoded, { timeout: 15_000, message: 'video element must appear and decode' })
		.toBe(true);

	// If the autoplay policy left the media paused (it normally doesn't here,
	// because the click that injected the iframe is a user gesture), start it.
	const initialPaused = await mediaDoc!.evaluate(() => {
		const v = document.querySelector('video');
		if (v && v.paused) {
			v.play().catch(() => {});
		}
		return v !== null && v.paused;
	});
	if (initialPaused) {
		await expect
			.poll(decoded, { timeout: 5_000, message: 'play() should keep the media loaded' })
			.toBe(true);
	}

	// REAL playback: the playback clock must advance past the first frame, not
	// just decode into a stalled buffer.
	const clock = () =>
		mediaDoc!.evaluate(() => {
			const v = document.querySelector('video');
			return v ? v.currentTime : 0;
		});
	await expect
		.poll(clock, { timeout: 15_000, message: 'playback must advance past the first frame' })
		.toBeGreaterThan(0.4);

	await new Promise<void>((resolve) => media.close(() => resolve()));
	await reader.close();
});

test('the owner can delete a post', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	// A throwaway post, so the shared post the earlier tests rely on survives.
	await page.getByRole('button', { name: 'New post' }).click();
	await page.waitForURL(/\/admin\/editor\//);
	await page.getByLabel('Title', { exact: true }).fill('Deletable E2E Post');
	await page.getByLabel('Markdown').fill('# Deletable E2E Post\n\nDelete me.');
	await page.getByRole('button', { name: 'Save', exact: true }).click();
	await expect(page.getByText('Saved')).toBeVisible();
	await page.getByRole('button', { name: 'Publish', exact: true }).click();
	await expect(page.getByText('Published', { exact: true })).toBeVisible();

	const slug = (await page.getByRole('link', { name: 'View post' }).getAttribute('href'))!.replace(/^\/articles\//, '');
	expect(slug).not.toBe('');

	// The public article is live.
	const live = await page.request.get(`/articles/${slug}`);
	expect(live.status()).toBe(200);

	// Delete from the dashboard row (the shared post row keeps its own button).
	await page.goto('/admin');
	const row = page.locator('tr', { hasText: 'Deletable E2E Post' });
	page.on('dialog', (d) => d.accept());
	await row.getByRole('button', { name: 'Delete' }).click();
	await expect(page.getByText('Post deleted.')).toBeVisible();
	await expect(page.getByText('Deletable E2E Post')).toHaveCount(0);

	// The shared post still shows in the posts table (the widget and the nudge
	// also render its title, so scope the assertion to the table row link).
	await expect(page.getByRole('table').getByRole('link', { name: TITLE })).toBeVisible();

	// The URL now 404s and the dashboard still shows the shared post.
	const gone = await page.request.get(`/articles/${slug}`);
	expect(gone.status()).toBe(404);
	await page.context().close();
});

test('the owner can log out and log back in', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.getByRole('button', { name: 'Log out' }).click();
	await page.waitForURL(/\/login$/);

	await page.getByLabel('Email', { exact: true }).fill(EMAIL);
	await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
	await expect(page.locator('#password')).toHaveAttribute('type', 'password');
	await page.getByRole('button', { name: 'Show password' }).click();
	await expect(page.locator('#password')).toHaveAttribute('type', 'text');
	await page.getByRole('button', { name: 'Hide password' }).click();
	await expect(page.locator('#password')).toHaveAttribute('type', 'password');
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page).toHaveURL(/\/admin$/);
	await expect(page.getByRole('table').getByRole('link', { name: TITLE })).toBeVisible();
	await page.context().close();
});
