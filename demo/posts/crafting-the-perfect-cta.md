# Crafting the Perfect CTA

The call-to-action is the one block on the page that asks for something. Every
other block is a promise; the CTA is the request. And because it is a request,
it is the block most worth measuring and most worth testing. This post walks
through what makes a CTA work and how to use Forgepost's experiment tooling to
stop guessing about the words that ask.

{{img:chart:A/B results}}

## The CTA is a block

In Forgepost, a call-to-action is its own block kind, not a hyperlink dropped
at the bottom of a paragraph. That one structural decision matters more than
any copywriting tip in this post, because it makes the CTA a *first-class
measurable object*. The stats page treats it like any other block — you see
where it sits in the scroll-depth funnel, how many readers got within view of
it, and how that view rate compares with the blocks around it.

A CTA nobody ever reaches still costs nothing to write, but it earns nothing
either. The block's real job starts after the article has earned the right to
ask.

## Test the words, not your gut

The fastest way to write better CTAs is to admit you have no idea which words
work for *your* readers. Not "which words work" in general — the general advice
is where this post could stop and still be correct. Across years of testing,
the same few patterns keep surfacing:

- **Imperative beats declarative.** *Subscribe* out-performs *You can
  subscribe* almost every time.
- **A specific subject beats an abstract one.** *Get the weekly notes* beats
  *Join the newsletter*, because readers can picture the first.
- **First person beats second person at the last step.** *Give me your
  email* — the button — reads strangely; *Keep me posted* reads like the
  reader talking.
- **One ask beats a menu.** A page asking you to subscribe, follow, *and*
  share asks you to make a decision; one clear ask lets you make a yes-or-no.

But "almost every time" and "across years of testing" are exactly the phrases
that should make you suspicious. Your audience, your topic, your cadence — the
local optimum lives somewhere specific. The only way to find it is measurement.

## What to change in a CTA test

A CTA has three variables, and an experiment should change one at a time.

**The message.** The words that promise the value — *Subscribe*, *Get the
notes*, *Keep me posted*, *Read the archive*. This is the block's content and
the natural thing to test. Rewriting it costs nothing and isolates the copy.

**The placement.** Where in the article the ask sits — after the intro (bold),
mid-article (earned), or at the end (classic). Placement is a render-time
property and, in the current model, part of how you structure blocks. Test a
version inserted at a different scroll depth to see whether the ask comes too
early or too late.

**The framing.** What surrounds it — a bare button versus a one-line push with
the promise. The CTA's neighbors are separate blocks; a test that changes the
*pair* is legal but muddies attribution. For clean learning, keep the
surroundings fixed and change the CTA itself.

## Running a CTA experiment

The mechanics are the same as a headline test, and the block kind is
explicitly experimentable:

1. On the article's Stats page, find the CTA in the block table.
2. Create an experiment, set a goal — in the MVP model the goal is a
   "completion" (scroll to the end), so a CTA near the bottom is measuring the
   reader's closing decision, which is exactly the job.
3. Author one or two variants of the message. Keep the same promise, change the
   speech.
4. Start it, set the traffic share, and let the deterministic split do its
   work.

Control is the version currently live; each variant is written as a new
immutable version in the pool. The report shows impressions, conversions, and
conversion rate per variant plus P(beats control) — the Bayesian probability
that this wording is genuinely better than the one you shipped.

## Reading the verdict honestly

A CTA test settles into three kinds of outcomes, and each is a win if you read
it correctly:

- **A clear winner.** The challenger crossed the confidence bar; the engine
  promotes it and the block repoints. Ship it, and move on to the *next*
  variable.
- **A near-tie.** P(beats control) sits around 50% with a wide interval.
  This is real information: your readers shrugged at both wordings. Save the
  version, change something bolder, retest.
- **Control wins.** The new words lost to the old ones — the most common and
  most valuable outcome, because it is why you test instead of trusting the
  general advice. The loser stays in the pool; nothing is deleted.

Notice the framing. There is no losing outcome in testing words, because every
outcome sharpens what you know about your specific readers. The cost of a lost
test is the traffic it took; the cost of an untested hunch is that you never
find out the old CTA was costing you a third of your asks.

## Counting a conversion

One subtlety worth internalizing: a "conversion" under the MVP goal model is a
visitor who scrolled to the end — not a click. For a bottom-of-article CTA that
is a coarse but defensible proxy: the ask was reached, the article was actually
read, and the reader made whatever decision they made in full context. As the
goal model grows (clicks, shares, subscriptions), the same experiment rows will
re-aggregate against the finer signal without re-running the test — the
assignment and the version provenance are already recorded per event.

## A starter split

If you are trying this today, here is a three-variant starting split that
checks your baseline against two opposing speech patterns:

- **Control:** *Subscribe*
- **Variant A:** *Get the weekly notes*
- **Variant B:** *Keep me posted*

Same promise, three registers. Give it enough traffic to let the posterior
move, and read the report with the honest rules above. What you learn about
*your* readers will beat any list of best practices — because the list ends
where your audience begins.