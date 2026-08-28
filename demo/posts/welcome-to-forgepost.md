# Welcome to Forgepost

Forgepost is a self-hosted blogging engine with A/B testing built in at the
*block* level. Not the article level, not the headline-plus-first-paragraph
level — every heading, paragraph, image, and call-to-action is a measurable,
testable object. This post walks through what that actually means, why it
matters for the way we write on the web, and how to get the most out of your own
install.

{{img:header:Welcome banner}}

## Why another blogging engine?

Most blog platforms have settled into a comfortable pattern: you write a post,
apply a theme, publish it, and hope the headline lands. If it doesn't, you
manually change it and try again, with no idea whether the second version was
an improvement or a lateral move. The analytics that *would* tell you — bounce
rate, time on page, scroll depth — live in a separate tab, keyed to some other
company's dashboard, and only rarely find their way back into the words you
write.

Forgepost is built on a different premise. The reader's behavior is already
being recorded by your blog, so the natural next step is to let the blog itself
make writing decisions visible. Every block you publish can carry a little
experiment: two versions of a headline, two phrasings of a paragraph, two
images in a card. Readers are split deterministically, both versions are shown,
and the numbers come back to *you* — not to some recommendation engine in the
cloud.

## What "self-hosted" really buys you

Running your own copy means the data pipeline is a closed loop:

- No third-party script on your pages. The tracker is one small file served
  from your own domain alongside the article.
- No warehoused reader logs sitting in someone else's compliance regime. The
  events live in your SQLite file, next to your posts.
- No guessing about whether your stats are sampled or filtered. You can query
  the same tables the dashboard reads.

There is a real cost. You are the operations department: updates, backups, and
the occasional `systemctl restart` are yours. The trade is honest — in exchange
for a little care you get full ownership of your words and your numbers.

## The block is the unit of thinking

Open any article here and you will notice the structure: a headline, then a
short intro, then a series of sections. Each of those pieces is a *block*, and
blocks are the smallest thing the editor lets you work with.

Why does granularity matter? Because reader attention is granular. A headline
is scanned long before the body is read. The first paragraph decides whether
the second paragraph exists. A call-to-action at the end of a good article
outperforms the same call-to-action at the end of a mediocre one. When you can
attach numbers to each of those pieces instead of averaging the whole page into
one "views" figure, the writing process changes:

1. You write the draft as usual.
2. You identify the block most likely to be the bottleneck — usually the
   headline.
3. You author one or two alternatives.
4. You switch on the test and let traffic decide.

The rest of this blog is a tour of exactly that loop.

## The publish → measure → experiment → improve loop

The workflow is short enough to internalize:

- **Publish.** Write in Markdown, save, publish. The public URL is fixed from
  the moment you save the slug, so you can share a draft link before the post
  goes live.
- **Measure.** Open the stats page for a document. You see views, unique
  readers (honestly labeled as estimates), average reading time, completion,
  and a scroll-depth funnel. The per-block table shows where readers leave —
  this is your map of the piece's weak spots.
- **Experiment.** Pick a weak block and give it alternatives. Control is
  whatever is live right now; each challenger is written to the version pool
  and shown to a share of visitors.
- **Improve.** The Bayesian engine watches impressions and conversions. When a
  variant clears the confidence bar, it is promoted automatically — the block
  now points at the winning version and the experiment closes.

Every promotion is a decision recorded in the database, so you can look back at
what you tested, with what effect size, and at what confidence. That history is
the quiet superpower of the whole system: over months, you learn which kind of
fake promises your own readers respond to.

## What this install includes

This demo blog ships with six long-form articles, a handful of bundled images,
seeded traffic, and — if you look at *Tracking Every Headline* — a live
experiment in progress with real counts in its report. Log in with the admin
credentials printed when you started the server and browse the dashboard:

- **Dashboard** — this week's most-read post, per-post seven-day views, and a
  nudge at the post with the worst read-through.
- **Stats** — the full per-article analytics surface, including the per-block
  drop-off table where the experiment controls live.
- **Editor** — Markdown in, block preview out, publish from the same screen.
- **Settings** — blog name, theme, the site-wide default image used as the
  social-card fallback, and comment moderation.

Nothing here is a teaser. Delete the demo posts, write your own, and the same
machinery that produced these figures serves yours.

## Who is this for?

You, probably, if you have ever changed a headline and wondered whether it
helped. Forgepost is a deliberate compromise for people who want the editorial
tooling of a large platform without the platform. It is not a CMS for a
ten-publisher magazine; there is one editor account and no role matrix. It is
not a host; you bring your own server and your own storage. What it gives back
is a writing environment where measurement and experimentation are not
afterthoughts but the primary shape of the editor.

## Getting the most out of it

A few habits make the difference between a blog that merely has A/B testing and
one that improves steadily:

- **Test one block at a time.** A headline test and a CTA test running
  simultaneously on the same article make the causality muddy. The engine
  handles both, but you will learn more from one clear retest at a time.
- **Change the word, not the world.** The best first experiment is nearly
  always the headline: the same claim, differently spoken. That isolates the
  variable you actually care about.
- **Let small data accumulate.** Forty impressions is a fun start; the engine
  is explicit about confidence precisely because it does not want you
  concluding anything at forty. Check back when the report has something to
  say.
- **Keep the losers.** A competitor version that lost last month is often the
  runner-up next month. The version pool is append-only, so nothing you test
  ever disappears.

## Closing

You are reading this article on our own dogfood: this very site is a Forgepost
installation, its headline is a control version in a running experiment, and
the paragraph you are on right now is one block of many. Explore the archive
article behind it, dig into the content-model post when you want the internals,
and then — most importantly — write something and test it. That is the whole
point.