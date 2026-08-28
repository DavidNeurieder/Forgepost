# Tracking every headline

Readers scan the title before they read the article. That sentence sounds
obvious, and yet almost every blog publishes exactly one headline per post and
calls the job done. This post shows how to stop guessing: you will create a
headline experiment in Forgepost, watch traffic split between two versions,
and let the report tell you which one earned more reads. The experiment running
on *this* headline is exactly that — a live, real test with the numbers still
accumulating.

## The anatomy of a headline test

A headline test in this system needs four pieces:

1. A **block** to test — here, the `heading` block that is the first line of
   this article.
2. A **goal** — the action that counts as success. In the product model of the
   MVP, a "completion" is a visitor who scrolled to the end of the article.
   This demo install configures its goal as clicks on the *Read next*
   recommendation cards after the article, so the experiment measures real
   onward behavior rather than a synthetic signal.
3. One or more **variants** — replacement content for the block. Control is
   created automatically from whatever is live right now.
4. A **share of traffic** — when you start the experiment, the system decides
   per visitor which version to show. The split is deterministic: the same
   visitor sees the same version on every page load, so your stats don't juggle
   between the two.

Every experiment lives on the article's **Stats** page. Open that page, find
the block table, and click *Create experiment* next to the headline.

## Authoring alternatives that are worth testing

The temptation with A/B testing is to test trivia — "blue text vs. black text".
Headline testing earns its keep when the alternatives are genuinely different
*framings*, not cosmetic tweaks. For this article, the challenger could be:

- **"Testing every headline"** — same subject, framed as a method rather than
  an observation.
- **"How to know when a headline works"** — framed as outcome, aimed at the
  reader's problem.
- **"The case for A/B testing your headlines"** — framed as an argument, aimed
  at the reader's skepticism.

Notice what stays constant. The *claim* of the article does not change between
variants; only the promise in the title does. That discipline is what makes the
result interpretable: if one headline clearly wins, you know the winner's
*framing* resonated, not that one version happened to describe a different
article.

## Variant content lives the same way you write

Variant content is real Markdown parsed into real blocks, not a string in a
database column. When you create a variant for this headline, the editor takes
`# Testing every headline`, parses it into a `heading` block, and writes it to
the shared version pool as an immutable version.

That detail quietly solves the hardest problem in content testing — attribution.
Every time a reader is assigned the challenger, the event records:

- the **variant** they were assigned,
- the **version** that variant pointed at, and
- the **experiment** that produced the assignment.

Because versions are immutable and the assignment is deterministic, you can
reproduce exactly what any given visitor saw on any given day. No ambiguity
about "the headline at the time" — the version pool is the source of truth and
it never rewrites history.

## Starting the experiment

Create the experiment, configure the traffic share and the stop rules, and
click *Start*. From that moment the live block renderer consults the
assignment, not you:

- The server computes a hash of `(experiment, visitor)`, maps it onto the
  control share and the variant weights, and serves the matching version.
- The assignment is recomputed on the server during event ingestion, so a
  client can't claim to have seen a variant it was never assigned.
- The article HTML carries `data-experiment-id` and `data-variant-id`
  attributes, which the tracker reads to report impressions.

Nothing about the article's URL or your editorial flow changes. Publishing,
editing, and un-publishing continue to work as before — the experiment is an
overlay on the block, not a fork of the document.

## Reading the live report

The report updates as events arrive. For each variant it shows:

- **Impressions** — how many assigned visitors actually saw the block.
- **Conversions** — how many of them performed the goal action.
- **Conversion rate** — the simple per-variant fraction.
- **P(beats control)** — the number that actually matters. This is a Bayesian
  posterior probability computed exactly from the observed binary outcomes: how
  confident the system is that this variant is better than control, given what
  it has seen so far.

P(beats control) is not a p-value and it is not a gut feeling. It is a
probability statement about a real quantity (the variant's conversion rate),
continuously updated. The report also shows credible intervals, so you see not
just the point estimate but how wide the uncertainty still is.

## The stop rules: when does the test end?

You can decide manually anytime, but the background auto-decider exists so you
don't have to babysit:

- **Win** — when a variant's posterior clears the spending-bound-corrected
  confidence threshold, the engine promotes it. The live block repoints to the
  winning version immediately.
- **No improvement** — when the engine becomes (near-)certain the variant
  cannot beat control within a meaningful effect, it concludes no-winner and
  keeps the current version.
- **Exhaustion** — the no-winner stopping rule caps how long you wait before
  the conclusion, so a test can never run forever on a trickle of traffic.

The decision, the promoted version, the effect size, and the confidence are
recorded as a *decision row* in the database and flow into exports. Your test
history is a first-class record, queryable and auditable.

## A note on honesty

The quiet risk in any testing tool is false precision. This system fights it
three ways:

- The threshold is spending-bound-corrected — it accounts for the fact that you
  peeked repeatedly, which naive "wait until p<0.05" testing does not.
- "Unique readers" and completion figures are labeled *estimated*, because
  ad-blockers and JS-disabled readers are invisible to any tracker by
  definition.
- The engine refuses to declare a winner at forty impressions. A good report
  will tell you it needs more data before it tells you who won.

That is the point of testing on a self-hosted blog: the conclusions are yours,
repeatable, and honest about their own uncertainty.

## Try it

This article is live proof: the headline you just read is one arm of a running
experiment, and its challenger is sitting in the version pool waiting for its
fortieth assigned reader. Open the Stats page, watch both numbers move, and
then go create a headline test of your own. You already know which block to
pick.