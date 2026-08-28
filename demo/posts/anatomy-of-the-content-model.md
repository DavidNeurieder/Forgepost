# Anatomy of the Content Model

Under the editor's Markdown surface, every article in Forgepost is a *block
tree* with an unusual property: blocks are immutable and versioned, and
experiments are just new versions layered on top. This post walks through the
model from the bottom up — why blocks, why immutability, and how the append-only
version pool is the single idea everything else — experiments, promotion, and
attribution — hangs off of.

{{img:cards:Content cards}}

## Documents are block sequences, not blobs

When you save a post, the Markdown is parsed once into a sequence of typed
blocks. Each block has a kind and a JSON payload:

```
kind        content                  example
heading     { "text": … }            "# Tracking every headline"
paragraph   { "text": … }            "Readers scan the title…"
image       { "url": … }             "![…](/media/<uuid>.png)"
video       { "url": … }             "https://www.youtube.com/watch?v=…"
quote       { "text": … }            "> A backup you cannot restore…"
code        { "text": … }            "```rust\nfn main() {}\n```"
call_to_action { "text": … }         "Subscribe →"
```

Storing content as blocks instead of a raw blob is what makes the rest of the
product possible. The editor can show a live block preview. The stats page can
report *per block* where readers drop off, because blocks are the reference
unit of measure. The experiment engine can replace one block out of twenty
without re-rendering (or re-storing) the other nineteen.

If you want to see the tree for real, open any article here and count the
blocks: the headline is block one, each paragraph is its own block, the image
is a block, the video is a block. The thing you are reading is a sequence of
small, replaceable objects.

## Blocks are immutable versions

Here is where the model stops being a normal document store. Every block is
identified by a version, and *versions never change once written*. Editing a
paragraph that is already published does not mutate it — the editor writes a
new version:

```
paragraph block
  version v1: "Forgepost is a self-hosted blog…"   ← published
  version v2: "Forgepost is a self-hosted blog with A/B testing…"
```

The block simply points at the version it currently shows. The version pool is
append-only: `v1` stays in the database, quoted by every visitor who was ever
served it, quoted by every experiment that measured it.

Two consequences fall out of this, and both matter.

**History is real.** "What did this look like in March?" has a concrete answer:
look at the version that was current in March. There is no silent rewinding of
published history, because there is no code path that deletes or edits an old
version — only ones that write new versions and move the pointer.

**Attribution is reproducible.** Every analytics event that touches a tested
block records the exact `version_id` the visitor was served. Because versions
are immutable, "which version did this conversion come from?" is a look-up, not
an archaeology problem. The data cannot drift from the content, because the
content cannot drift, period.

## Experiments are overlays on the tree

An experiment is a *branch* in the version pool for exactly one block. Control
is whatever the block points at today; each variant writes a new immutable
version and registers itself as an alternative. Creating an experiment:

```
block: heading "Tracking every headline" (current_version = v9)
  control → version v9   weight 50
  variant → version v99  "Testing every headline"   weight 50
```

Nothing about the document's canonical state changes. The block still points at
`v9`; the experiment is a parallel table saying "for assigned visitors, serve
the version this variant points at instead."

Because the control *is* the current version — not a copy of it — the system
never has to ask "what did the article look like when the test started?" The
version pool already knows, and the control variant literally points at it.

## The renderer is the only thing that flips

The live block renderer checks, per visitor, whether a running experiment
applies to the block, and if so serves the assigned variant's version. That is
the entire switch. There is only one place where "which version is shown" is
decided: the renderer, which reads `current_version` for the general case or
the experiment assignment for the tested case.

This single point of decision is the safety property. Promotion is just the
renderer's special case:

```
promote winning variant:
  begin transaction
    record decision (winner, promoted version, effect size, confidence)
    set block.current_version = winning version
    close experiment
  commit
```

Both rows change in the same transaction, so a crash can never leave an
article half-updated — a new current version without a recorded decision, or a
recorded decision pointing at a version that was never promoted. The atomic
swap is the whole trick, and immutability is what makes it cheap: the promoted
version was already written when the experiment was created, so "winning" never
involves writing content at decision time.

## Why a version pool instead of "the current block"

A naive design keeps one mutable "current block text" column and updates it on
edit. The version pool exists because every feature in the product wants the
history that mutable column throws away:

- **Experiments** want to compare a challenger against the *exact* control that
  readers saw, and to promote by repointing a pointer.
- **Attribution** wants the served version to be a stable key, joinable with
  events forever.
- **Editorial review** wants "what changed between this saved draft and that
  one" (via block-level diffs on version identity), with old versions intact.

All three collapse into one mechanism: never rewrite, only append and repoint.

## The cost of immutable append

Nothing is free, and the pool has three costs the docs are honest about:

- **Storage growth.** Every edit and every experiment variant adds a row. For
  a solo blog measured in megabytes this is philosophical; the discipline is
  simply to remember that the pool is meant to be cheap and boring, not
  precious.
- **No in-place corrections.** There is no "fix a typo in v1"; the fix is a
  new version. The human work is the same (fix the typo), but the model refuses
  to pretend the published words were always correct.
- **Deletion is a batch operation.** Removing a version that events reference
  would orphan those events, so deletion requires the tooling (a future
  compaction) to handle the dangling references deliberately.

For a writing tool, these costs are the right trade — the opacity of "the
current text" costs far more in trust than any of them.

## One model, running the whole demo

The experiment living on this very headline is a live demonstration of the
model: the control points at the original heading, a challenger sits in the
pool waiting, and `current_version` will flip in a single committed decision
when the numbers clear the bar. Look at the demo after it runs its course —
the block still shows one headline, the pool still holds both, and the decision
row explains exactly how the article got from here to there.