# The Comprehensive Benchmark Suite (Design)

This document defines what a *perfect* benchmark suite for mmdr would
measure, why each axis matters, and where the current suite has gaps.
It is the north star for benchmark work, not a description of today's
state. "Today" notes mark what already exists.

## 0. First principles: what are we actually optimizing?

A Mermaid renderer has four obligations, in priority order. A benchmark
suite is only "comprehensive" if every obligation is measured and the
hard ones gate the soft ones.

1. **Correctness** - the output faithfully encodes the input semantics
   (every node, edge, label, decoration present and connected correctly)
   and never crashes/hangs/emits invalid SVG.
2. **Hard geometry validity** - no overlaps, no edges through shapes,
   ports on boundaries, containment intact, finite coordinates.
3. **Readability / aesthetics** - the soft objective (crossings, bends,
   length, symmetry, density, label placement, flow direction).
4. **Performance** - parse/layout/render latency, memory, throughput,
   and scaling behavior.

A single scalar "score" is a trap. The suite must report a **vector**
per fixture and aggregate per-domain, because regressions in one domain
must never be hidden by improvements in another (this is exactly how the
`on_segment` bug hid: bend count improved on dense fixtures while
straight chains silently broke).

## 1. Correctness / semantic fidelity (BIGGEST CURRENT GAP)

Today we measure geometry of *whatever was produced*, but almost nothing
checks that the production is *right*. A layout can score perfectly on
crossings while having dropped three edges.

### 1.1 Structural round-trip
- **Node count parity**: rendered node count == parsed node count
  (minus intentional dummies). Per fixture, must be exact.
- **Edge count parity**: every input edge has exactly one rendered path.
- **Label presence**: every node label, edge label, start/end label,
  and decoration string appears in the SVG text content.
- **Connectivity correctness**: each rendered edge actually touches its
  declared `from` and `to` node boundaries (we have endpoint-boundary
  metrics but not "is it the *right* node").
- **Arrowhead/marker correctness**: arrow direction, open vs filled,
  multiplicity markers (class/ER), all present and on the correct end.
- **Z-order / occlusion**: labels not hidden behind shapes; edge labels
  have a background that is actually painted.

### 1.2 Semantic invariants per diagram type
Generic geometry is not enough; each type has rules:
- **Sequence**: actor order preserved; messages monotonic in time (y);
  activation bars nest correctly; loop/alt/opt frames enclose their
  messages; self-messages bend back to same lifeline.
- **Gantt**: task bars positioned by date; dependencies (after/until)
  respected; sections grouped; today-marker placement; weekend exclusion.
- **State**: composite states contain their children; fork/join bars;
  choice diamonds; start/end pseudostates rendered correctly.
- **Class**: member compartments ordered (fields then methods);
  visibility markers; inheritance triangle vs composition diamond vs
  aggregation; multiplicity labels at correct ends.
- **ER**: crow's-foot cardinality correct on the correct side; attribute
  rows; PK/FK markers.
- **Gitgraph**: commit chronology; branch lanes don't collide; merge
  commits connect both parents; tags/labels attached.
- **Pie/quadrant/xychart/radar**: data values map to correct geometry
  (slice angle ∝ value; point at correct (x,y); axis scale correct);
  legend/label correctness; total == 100% for pie.
- **Mindmap**: tree containment; no child outside parent's subtree band.
- **Sankey**: flow conservation (in-width == out-width per node);
  no negative flows.

These should be **assertion-style metrics**: 0 = pass, N = count of
violations. They gate everything else.

### 1.3 Render-validity gate
- SVG parses as valid XML; no NaN/Inf in any attribute (Today: partial,
  `robustness_suite.rs`).
- No element with zero/negative width or height where illegal.
- viewBox encloses all drawn content (no clipped geometry). Today we
  have `content_overflow_ratio` / `label_out_of_bounds`; extend to a
  hard "nothing is clipped" gate.

## 2. Hard geometry validity (mostly covered, formalize as gates)

Today: `assert_layout_is_well_formed`, `invariants.rs`, many metrics.
Make these a single **hard-gate report** that is binary per fixture:

- node_overlap_count == 0 (Today)
- edge_node_crossings == 0 for non-endpoint nodes (Today: counted, not gated)
- endpoint_off_boundary_count == 0 (Today)
- subgraph_boundary_intrusion_pairs == 0 (Today)
- containment intact (Today)
- all coordinates finite (Today)
- ports reflect inferred direction (Today: `port_direction_misalignment`)

The suite should run these as a **pre-filter**: a fixture that fails a
hard gate is excluded from aesthetic ranking and flagged red, so soft
improvements can never mask a hard regression.

## 3. Readability / aesthetics (rich today, but flowchart-centric)

Today's ~100 metrics in `layout_score.py` are excellent for flowcharts.
Gaps:

### 3.1 Per-type readability metrics (currently generic-only)
The scorer keys most metrics off `flow_axis` (flowchart direction).
Non-flowchart types need their own:
- Sequence: lifeline spacing uniformity; message label collisions;
  activation overlap; frame nesting tightness.
- Class/ER: compartment alignment; relation label legibility; box
  aspect ratios; whitespace balance in the grid.
- Gantt: bar height consistency; row striping legibility; label fit
  inside vs outside bar.
- Pie/charts: label leader-line crossings; slice-label overlap; axis
  tick legibility; data-ink ratio.

### 3.2 Perceptual / structural-similarity vs mermaid-cli (BIG GAP)
Today conformance uses mean/RMS pixel difference only (`ImageChops`).
That is brightness-naive and penalizes harmless shifts. Add:
- **SSIM** (structural similarity) - perceptual, shift-tolerant.
- **Edge/contour IoU** after edge detection - "are the same lines in
  roughly the same place."
- **Graph-aware diff**: match nodes by label, compare relative
  positions (left-of / above relations) rather than absolute pixels.
  This is the metric that actually answers "does it look like the
  reference layout" without demanding pixel parity.
- **Text legibility**: OCR the render, compare recovered strings to
  input labels (catches clipped/overlapping/tiny text).

### 3.3 Aesthetic axes not yet measured
- **Symmetry**: mirror/rotational symmetry score for symmetric inputs
  (a balanced tree should render balanced).
- **Orthogonality**: fraction of edge length that is axis-aligned
  (diagonal jaggies are ugly). Today we count bends but not "clean 90°".
- **Angle quality at bends**: are corners true right angles vs near-90°
  jitter (the drift bug). Add `near_axis_jitter_count` distinct from
  `edge_bends`.
- **Edge-length uniformity / variance**, not just total.
- **Node-size consistency** within a type/rank.
- **Visual balance / center of mass** vs canvas center (Today: partial
  `content_center_offset_ratio`, `margin_imbalance_ratio`).
- **Crossing minimality vs theoretical lower bound** where computable.

## 4. Stability / determinism (CURRENT GAP)

A layout engine that produces different output for the same input, or
wildly different output for a one-edge change, is unusable in diffs and
version control.

- **Determinism**: render each fixture N times (and across thread
  counts / `--fastText` on-off where geometry should be identical);
  assert byte-for-byte (or coordinate-for-coordinate) stable.
- **Incremental stability**: take a fixture, add/remove one node or edge,
  measure node displacement of the unchanged nodes. The objective doc
  already lists "node displacement vs prior layout (weight 3)" but there
  is no benchmark driving it. Build a *mutation suite*: each base
  fixture + a family of single-edit variants, scored on how little the
  rest moved.
- **Config sensitivity**: small `nodeSpacing`/`rankSpacing` changes
  should produce proportional, monotonic changes, not chaotic relayouts.

## 5. Performance (covered, extend the envelope)

Today: criterion `benches/renderer.rs`, `--timing`, `bench_compare.py`,
500-1600x speedup tracking vs mermaid-cli.

Gaps:
- **Memory / peak RSS** per fixture and scaling curve (not just time).
- **Scaling curves**: parametric fixtures at N = 10/50/100/500/1000
  nodes per type; fit complexity (verify near-linear, catch O(n^2)
  blowups like the A* router on huge graphs).
- **Worst-case / pathological inputs**: complete graphs, deep nesting,
  long label storms, dense back-edges - latency *and* that hard gates
  still hold under stress.
- **Tail latency** (p95/p99 across the corpus), not just means.
- **Cold vs warm** font cache (Today: documented, fold into the harness).
- **Allocation count / churn** via a tracking allocator in a bench mode.
- **Output size**: SVG byte size and path-point count (bloated paths =
  slow downstream rendering; also a proxy for the jitter bug).
- **Regression guardrails**: fail CI if any fixture regresses > X%.

## 6. Robustness / fuzzing (partially covered)

Today: `robustness_suite.rs` (6 tests), `parse_errors.rs`.
Extend to a real corpus:
- **Parser fuzzing**: random/mutated `.mmd`; never panic, always either
  a structured error or a finite render.
- **Unicode / RTL / CJK / emoji / zero-width** in labels: width
  measurement correctness, no overflow, no panic.
- **Degenerate inputs**: empty diagram, single node, duplicate ids,
  self-loops only, disconnected forest, cycles, 0-area nodes.
- **Adversarial labels**: extremely long single tokens, newlines,
  markdown, HTML entities, quotes.
- **Every diagram type** must have a robustness fixture (Today: mostly
  block/flowchart).

## 7. Cross-cutting: corpus design

The fixture corpus is the foundation; metrics are worthless on a thin or
biased corpus. Today the corpus is **heavily flowchart-weighted** (17
flowchart vs 1 each for sankey/radar/packet/kanban/treemap/architecture).

Target corpus structure - for **every** diagram type:
- **tiny / small / medium / large / mega** size tiers (parametric).
- **canonical** real-world example (the README-quality diagram).
- **stress** variant exercising that type's hard cases.
- **robustness** variant (degenerate/adversarial).
- **mutation** family for stability (base + single-edit variants).
- **reference** render from mermaid-cli, version-pinned.

Diagram types currently under-covered (1 fixture each) that need the
full tier set: architecture, block, c4, kanban, packet, radar, sankey,
treemap, requirement, zenuml, timeline, journey.

## 8. Aggregation, weighting, and reporting

- **Per-domain rollups**: Correctness / Hard-validity / Readability /
  Stability / Performance reported separately. Never collapse to one
  number without showing the vector.
- **Hard gates are pass/fail**, not weighted - a single hard violation
  flags the fixture red regardless of beauty.
- **Auto-weighting** for soft metrics (Today: `priority_bench.py
  --weight-mode auto` derives weights from variance + correlation) -
  keep, but compute weights per-diagram-type, not globally.
- **Baselines that travel with the repo**: `tests/quality_baseline.json`
  today hardcodes absolute paths to a different worktree
  (`mermaid-rs-renderer-master`) and predates the current engine, so it
  is not a usable gate. Fix: store relative paths, regenerate against
  HEAD, and version the baseline so deltas are vs the immediately prior
  committed state, not an ancient snapshot.
- **Two comparison axes**: (a) self-regression vs previous commit;
  (b) conformance vs mermaid-cli reference. Both matter and are
  different questions.
- **Human-reviewable artifacts**: the side-by-side HTML
  (`render_comparison.py`) plus a metrics dashboard with per-domain
  trend lines over commits. A picture catches what metrics miss and
  vice versa - the suite must produce both.
- **Triage ranking**: sort fixtures by `(hard_violations, weighted_soft
  regression vs reference)` so the worst diagrams surface first. This is
  how you "fix until perfect for every diagram."

## 9. CI integration

- Fast tier (cargo test + hard-gate metrics on the small corpus) on
  every commit.
- Full tier (conformance image-diff + perceptual + performance scaling)
  nightly or on demand, since mermaid-cli/Chromium is slow.
- Gate merges on: zero hard-gate regressions, zero correctness
  regressions, no perf regression > threshold, no determinism failures.

## 10. Concrete build order (highest leverage first)

1. **Correctness gate** (Section 1.1/1.3): node/edge/label parity +
   render-validity. Cheap, catches the scariest bugs (dropped content),
   currently nearly absent.
2. **Hard-gate pre-filter** (Section 2): turn existing invariant metrics
   into binary gates that mask soft scores.
3. **Determinism + mutation/stability suite** (Section 4): cheap to run,
   exposes a whole class of invisible bugs; drives the existing but
   unbenchmarked "displacement" objective.
4. **Per-type semantic invariants** (Section 1.2): the long tail, but the
   only way non-flowchart types reach "perfect."
5. **Perceptual conformance** (Section 3.2): SSIM + graph-aware diff to
   replace brightness-naive pixel RMS.
6. **Corpus expansion** (Section 7): full size/stress/robustness tiers
   for the 12 under-covered types.
7. **Performance envelope** (Section 5): memory + scaling curves + tail
   latency + output-size regression guards.
8. **Reporting/aggregation** (Section 8): per-domain dashboard, fixed
   baselines, triage ranking.

## Summary: where today's suite is strong vs missing

Strong today: flowchart geometric metrics (~100), criterion latency
benches, image-diff conformance harness, auto-weighting, side-by-side
HTML, basic invariants/robustness.

Missing / weak: semantic correctness (content parity, per-type
invariants), hard-gate pre-filtering, determinism + incremental
stability, perceptual/graph-aware conformance (vs pixel RMS), per-type
readability metrics (everything is flowchart-axis based), memory +
scaling + tail-latency, balanced multi-type corpus, and a
travels-with-repo baseline that compares vs the prior commit.

## Implementation status (updated)

Built and wired into CI (`cargo test --all-targets` plus a dedicated
`layout-quality-gate` job):

- **Domain 1 - Correctness** (`tests/correctness_suite.rs`): per-fixture
  node/edge/label parity, render validity (well-formed SVG, no NaN/Inf,
  positive viewBox), edge-count parity. Handles wrapped labels, class/ER
  compartment tables, line-break spellings, synthetic-node exclusion.
- **Domain 2 - Hard gate** (`scripts/hard_gate.py`,
  `tests/hard_gate_baseline.json`): renders the corpus, partitions
  GREEN/RED on hard geometry predicates (node overlap, edge-through-node,
  off-boundary endpoint, subgraph intrusion, non-finite), kind-gated and
  shape/arrowhead-aware. Baseline ratchet: fails only on new reds or
  worse counts; improvements nudge a re-lock.
- **Domain 3 - Determinism + stability** (`tests/determinism_suite.rs`):
  byte-identical SVG and identical geometry across runs for every
  fixture; incremental-stability probe (appending a leaf barely moves
  existing nodes).
- **Domain 4 - Per-type semantics** (`tests/semantic_suite.rs`): pie
  slice angles sum to a full circle and are value-proportional; sankey
  positive thickness and node-total throughput; sequence lifeline
  verticality, participant order, autonumber monotonicity; gantt
  in-bounds tasks and duration-monotonic widths.
- **Output-shape guards** (`tests/output_shape_suite.rs`): straight
  chains stay bend-free in all directions/sizes; minimal edge points;
  symmetric fan-out. Directly guards the on_segment bug class.

Renderer bugs these surfaced and fixed:
- C4 connectors no longer cut through intervening shapes (route around).
- State terminal/start pseudostate markers no longer overlap real states.
- Gantt task ids ending in a duration letter (e.g. `arch`) are no longer
  misparsed as durations (this had blown the time axis out to year 2240).

Still open (gated/ratcheted, not regressions):
- Overlapping sibling subgraphs when nodes connect across them in dense
  multi-subgraph flowcharts (9 RED fixtures, mostly mega stress cases).
- Perceptual/graph-aware conformance (SSIM) to replace pixel RMS.
- Corpus expansion to full tiers for under-covered types.
- Memory/scaling/tail-latency performance envelope.
