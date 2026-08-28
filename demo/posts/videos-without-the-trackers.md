# Videos Without the Trackers

Almost every blog that embeds videos from YouTube or Rumble hands the hosting
platform a slice of the reading experience: a third-party iframe, a stack of
third-party scripts, and a view of your readers that you neither asked for nor
control. This post explains how Forgepost embeds videos without leaking your
readers' data, and why the *click-to-load* pattern matters for privacy, for
speed, and for trust.

## The standard embed is a leak

When you paste a normal YouTube embed into an article, the reader's browser
does roughly this:

1. It loads the provider's iframe on page load.
2. The iframe executes the provider's player script.
3. The provider observes the page's presence of your article (it sees the
   `Referer`), records an impression, and — depending on your reader's
   settings — starts tracking the session even when the video never plays.

The reader is on *your* site, reading *your* words, and a third party is
collecting telemetry about the visit. Many readers are fine with this. Many
others are not, and a growing number of browsers block the worst of it. Either
way, you — the host who made the embedding choice — are the one who decided
that your visitors' page loads should include a third-party relationship they
did not consent to.

## A video block that waits

Forgepost treats a video as a first-class *block* in the document, not as ad
hoc HTML pasted into a paragraph. A line that is exactly one YouTube or Rumble
URL (or a raw `<iframe>` line) becomes a `video` block. Kinds matter, because
the renderer can then do something a paragraph full of raw HTML cannot: render
the block as a *button*.

The video block renders with:

- a lazy-loading thumbnail (derived from the video id for YouTube, or fetched
  once from Rumble's oEmbed endpoint for Rumble),
- a play badge,
- *no iframe*, and
- **zero** third-party network requests.

The reader's browser never contacts the video provider just because your page
loaded. The embed is genuinely inert until the visitor chooses to interact with
it — one click swaps in the player. At that point the request happens, in the
reader's hands, on the reader's terms.

## Privacy, mechanically

Let's be concrete about the difference it makes. With a click-to-load video
block:

- **No embed script on the page.** The provider's SDK is not part of your
  page's critical path, so it cannot read your article's DOM or piggyback on
  your analytics.
- **No third-party cookie on page load.** The reader has not visited the
  provider's domain at all; there is nothing for it to drop or read.
- **Safe embedding domains.** YouTube embeds are served from
  `youtube-nocookie.com`, the privacy mode variant that is explicit about its
  reduced tracking surface. The iframe is created with
  `referrerpolicy="no-referrer"`, so the provider's player does not learn which
  of your articles the video sits in.
- **Attribution stays local.** When a reader plays a video, the event is a
  normal element in your own `analytics_events` — a local record you control,
  not a beacon to a third-party warehouse.

Click-to-load is not a performance trick bolted on afterward; it is the block's
default mode, because it is the mode that respects the reader's choice. A
visitor who never wanted video contact gives the provider nothing. A visitor
who clicks gets the video. That is the entire contract.

## What you can embed

Three forms parse into video blocks, and each is validated for safety:

- **A YouTube URL** — `watch`, `shorts`, `embed`, `live`, or a bare
  `youtu.be` short link.
- **A Rumble URL** — a `watch` or `embed` link; Rumble's title and thumbnail
  are fetched once, best-effort, at save time (with a short timeout, and the
  fetch is non-fatal — saving never depends on the network).
- **A raw `<iframe>` line** — for providers that offer neither, with
  attributes whitelisted to `src`, `title`, `width`, and `height`. The `src`
  must be `http`/`https`, so a hand-crafted tag cannot smuggle
  `javascript:` or `data:` schemes into your articles.

A URL embedded in normal prose, by contrast, stays a plain paragraph — the
block only forms when the URL is the entire line. This keeps conversation and
embeds from colliding in the Markdown.

## What the reader (and search engines) see

Because the block is a real block and not raw HTML, the rendering is
predictable and testable. The article gains proper video metadata:

- **Open Graph** — `og:video`, `og:video:type`, and `og:video:secure_url`, so
  link previews in most social apps can render the video.
- **JSON-LD** — a `VideoObject` node with name, description, thumbnail URL,
  upload date, and the embed/content URLs. Search engines that understand
  structured video data can list your article as a video result.

And readers with JavaScript disabled still see the thumbnail and the title
behind the play badge — the link to the provider has a plain `href`, so the
video is never locked away from a text-mode or cautious reader.

## The edge case that stays an edge case

Video blocks are deliberately **not** experimentable. You can A/B test
headlines, paragraphs, images, and calls-to-action — but a video's content is a
single immutable URL, and "testing" it would mean minting a new version of the
embed whose only real difference is the provider link. That is an attribution
trap with little upside, so the editor simply refuses. The block is immutable,
like every other version in the pool, but only one version will ever exist for
a video. Keeps the mental model simpler, too.

## A reflex worth building

Here is a useful habit once you run your own blog: before embedding any piece
of media, state the transactional cost out loud — "this page load phones home
to X" — and then ask whether that call is happening on the reader's behalf or
on the provider's. Click-to-load upends the default answer. Try the exact
pattern below — one click, and only one click, is all the provider ever gets
from your readers:

https://www.youtube.com/watch?v=dQw4w9WgXcQ

Nothing loaded at page load, nothing tracked in the background, nothing leaked
to a third party. The lazy thumbnail and the play badge are your whole
relationship with the provider until — and unless — the reader reaches out
first.