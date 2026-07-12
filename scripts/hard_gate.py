#!/usr/bin/env python3
"""Hard-gate pre-filter for layout quality (benchmark suite domain 2).

The aesthetic score (`scripts/layout_score.py` / `scripts/quality_gate.py`)
ranks how *pretty* a layout is. But a layout with a hard geometry violation -
overlapping nodes, an edge passing through an unrelated node, an off-boundary
endpoint, a subgraph boundary intrusion - is broken regardless of how good its
crossing/bend numbers look. Soft improvements must never mask a hard
regression.

This script renders every fixture, computes the hard-violation predicates, and
emits a report partitioned into:

  * RED   - one or more hard violations. Excluded from aesthetic ranking.
  * GREEN - hard-clean. Eligible for soft scoring.

It exits non-zero if any fixture is RED (so it can gate CI), and prints a
triage-ranked list (worst first) so the most broken diagrams surface first.

Hard predicates (all must be zero):
  node_overlap_count            nodes overlapping each other
  edge_node_crossings           edge passing through a non-endpoint node
  endpoint_off_boundary_count   edge endpoint not on its node's boundary
  subgraph_boundary_intrusion_pairs  edge cutting through an unrelated subgraph
  non_finite                    any NaN/Inf coordinate in the dump

Usage:
  scripts/hard_gate.py                      # gate the default corpus
  scripts/hard_gate.py --pattern flowchart  # subset by path regex
  scripts/hard_gate.py --json out.json      # machine-readable report
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import math
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Optional

ROOT = Path(__file__).resolve().parents[1]

# Hard-violation metrics: every one of these must be zero for a GREEN fixture.
# Each predicate only applies to diagram kinds where it is a genuine invariant.
# `endpoint_off_boundary` is meaningful for box-and-arrow graph diagrams, but
# not for sequence (messages run down lifelines, far from the actor box) or
# mindmap (organic curves to node centers), so it is gated by kind.
GRAPH_KINDS = {"flowchart", "state", "class", "er", "c4"}

HARD_METRIC_KINDS = {
    "node_overlap_count": GRAPH_KINDS,
    "edge_node_crossings": GRAPH_KINDS,
    "endpoint_off_boundary_count": GRAPH_KINDS,
    "subgraph_boundary_intrusion_pairs": GRAPH_KINDS,
    # Semantic containment: nodes rendered inside subgraph boxes they don't
    # belong to, or member nodes spilling outside their own box. Both
    # misstate the diagram's declared structure.
    "containment_foreign_node_count": GRAPH_KINDS,
    "containment_member_escape_count": GRAPH_KINDS,
    # Text clipping: the measured label block must fit inside its node box
    # for box-shaped kinds. Sankey/architecture/state-markers legitimately
    # draw text outside the shape and are excluded via kind gating or id
    # filtering inside layout_score.
    "label_overflow_count": GRAPH_KINDS,
    # Canvas overflow is universal for visible nodes: nothing may render
    # outside the emitted SVG canvas.
    "canvas_overflow_count": None,
    # non_finite is universal: no diagram may emit a NaN/Inf coordinate.
    "non_finite": None,
}

DEFAULT_FIXTURE_DIRS = [
    ROOT / "tests" / "fixtures",
    ROOT / "docs" / "comparison_sources",
    ROOT / "benches" / "fixtures",
]


def load_layout_score():
    module_path = ROOT / "scripts" / "layout_score.py"
    spec = importlib.util.spec_from_file_location("layout_score", module_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)  # type: ignore[call-arg]
    return module


def pick_binary() -> Path:
    primary = ROOT / "target" / "release" / "mmdr"
    if primary.exists():
        return primary
    fallback = ROOT / "target" / "release" / "mermaid-rs-renderer"
    return fallback if fallback.exists() else primary


def collect_fixtures(patterns: List[str]) -> List[Path]:
    out: List[Path] = []
    for base in DEFAULT_FIXTURE_DIRS:
        if not base.exists():
            continue
        for path in sorted(base.rglob("*.mmd")):
            rel = str(path.relative_to(ROOT))
            if patterns and not any(re.search(p, rel) for p in patterns):
                continue
            out.append(path)
    return out


def dump_layout(binary: Path, source: Path, out_dir: Path) -> Optional[Path]:
    # Use the full relative path (slashes -> __) as the key so fixtures that
    # share a stem (e.g. the many `basic.mmd` files across diagram dirs) do not
    # collide and overwrite each other's layout JSON.
    rel = source.relative_to(ROOT)
    key = "__".join(rel.with_suffix("").parts)
    out_json = out_dir / (key + "-layout.json")
    out_svg = out_dir / (key + ".svg")
    res = subprocess.run(
        [str(binary), "-i", str(source), "--dumpLayout", str(out_json),
         "-o", str(out_svg), "-e", "svg"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    if res.returncode != 0 or not out_json.exists():
        return None
    return out_json


def non_finite_count(data: dict) -> int:
    """Count NaN/Inf coordinates anywhere in nodes/edges/subgraphs."""
    bad = 0

    def check(v) -> int:
        try:
            f = float(v)
        except (TypeError, ValueError):
            return 0
        return 0 if math.isfinite(f) else 1

    for node in data.get("nodes", []):
        for k in ("x", "y", "width", "height"):
            bad += check(node.get(k))
    for sg in data.get("subgraphs", []):
        for k in ("x", "y", "width", "height"):
            bad += check(sg.get(k))
    for edge in data.get("edges", []):
        for pt in edge.get("points", []):
            if isinstance(pt, (list, tuple)):
                for c in pt:
                    bad += check(c)
    return bad


def evaluate(layout_score, path: Path) -> Dict:
    data, nodes, edges = layout_score.load_layout(path)
    metrics = layout_score.compute_metrics(data, nodes, edges)
    kind = str(data.get("kind", "")).strip().lower()
    violations = {}
    for key, kinds in HARD_METRIC_KINDS.items():
        if kinds is not None and kind not in kinds:
            continue
        if key == "non_finite":
            n = non_finite_count(data)
        else:
            val = metrics.get(key, 0)
            try:
                n = int(round(float(val)))
            except (TypeError, ValueError):
                n = 0
        if n > 0:
            violations[key] = n
    total = sum(violations.values())
    return {
        "fixture": path.stem.replace("-layout", "").replace("__", "/"),
        "kind": kind,
        "violations": violations,
        "hard_violation_total": total,
        "status": "RED" if total else "GREEN",
        "node_count": metrics.get("node_count", 0),
        "edge_count": metrics.get("edge_count", 0),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Hard-gate pre-filter for layout quality")
    parser.add_argument("--pattern", action="append", default=[],
                        help="regex to filter fixture paths (repeatable)")
    parser.add_argument("--json", default="", help="write machine-readable report")
    parser.add_argument("--bin", default="", help="mmdr binary path")
    parser.add_argument("--baseline", default=str(ROOT / "tests" / "hard_gate_baseline.json"),
                        help="known-RED baseline; gate fails only on regressions vs it")
    parser.add_argument("--strict", action="store_true",
                        help="ignore the baseline; fail on any RED fixture")
    parser.add_argument("--write-baseline", action="store_true",
                        help="overwrite the baseline with the current RED set and exit 0")
    args = parser.parse_args()

    layout_score = load_layout_score()
    binary = Path(args.bin) if args.bin else pick_binary()
    if not binary.exists():
        print(f"error: mmdr binary not found at {binary}; run `cargo build --locked --release`",
              file=sys.stderr)
        return 2

    fixtures = collect_fixtures(args.pattern)
    if not fixtures:
        print("error: no fixtures matched", file=sys.stderr)
        return 2

    results: List[Dict] = []
    render_failures: List[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        tmp_dir = Path(tmp)
        for src in fixtures:
            layout = dump_layout(binary, src, tmp_dir)
            if layout is None:
                render_failures.append(str(src.relative_to(ROOT)))
                continue
            try:
                results.append(evaluate(layout_score, layout))
            except Exception as exc:  # pragma: no cover - defensive
                render_failures.append(f"{src.relative_to(ROOT)}: score error {exc}")

    red = [r for r in results if r["status"] == "RED"]
    green = [r for r in results if r["status"] == "GREEN"]
    # Triage: worst (most violations) first.
    red.sort(key=lambda r: r["hard_violation_total"], reverse=True)

    print(f"Hard-gate report: {len(green)} GREEN / {len(red)} RED "
          f"/ {len(render_failures)} render-failure "
          f"({len(results) + len(render_failures)} fixtures)\n")
    if render_failures:
        print("RENDER FAILURES (cannot even produce a layout):")
        for f in render_failures:
            print(f"  {f}")
        print()
    if red:
        print("RED (hard geometry violations, excluded from aesthetic ranking):")
        for r in red:
            detail = ", ".join(f"{k}={v}" for k, v in sorted(r["violations"].items()))
            print(f"  [{r['hard_violation_total']:>3}] {r['fixture']:<40} {detail}")
        print()

    report = {
        "summary": {
            "green": len(green),
            "red": len(red),
            "render_failures": len(render_failures),
        },
        "red": red,
        "green": [r["fixture"] for r in green],
        "render_failures": render_failures,
    }
    if args.json:
        Path(args.json).write_text(json.dumps(report, indent=2))

    current = {r["fixture"]: r["hard_violation_total"] for r in red}

    if args.write_baseline:
        baseline_doc = {
            "_comment": "Known hard-gate RED fixtures; gate fails on new reds or "
                        "worse counts vs this baseline. Lower is better.",
            "fixtures": dict(sorted(current.items())),
        }
        Path(args.baseline).write_text(json.dumps(baseline_doc, indent=2) + "\n")
        print(f"Wrote baseline with {len(current)} RED fixtures to {args.baseline}")
        return 0

    if args.strict:
        return 1 if (red or render_failures) else 0

    # Baseline-aware gating: fail only on regressions (new RED fixtures, higher
    # violation counts, or any render failure). Improvements are reported as a
    # nudge to update the baseline but do not fail.
    baseline = {}
    bpath = Path(args.baseline)
    if bpath.exists():
        try:
            baseline = json.loads(bpath.read_text()).get("fixtures", {})
        except (ValueError, OSError):
            baseline = {}

    regressions = []
    for fixture, count in current.items():
        prev = baseline.get(fixture)
        if prev is None:
            regressions.append(f"NEW RED   {fixture} ({count} violations)")
        elif count > prev:
            regressions.append(f"WORSE     {fixture} ({prev} -> {count})")
    improvements = []
    for fixture, prev in baseline.items():
        cur = current.get(fixture, 0)
        if cur < prev:
            improvements.append(f"{fixture} ({prev} -> {cur})")

    if improvements:
        print("IMPROVED vs baseline (run --write-baseline to lock in):")
        for line in improvements:
            print(f"  {line}")
        print()
    if regressions or render_failures:
        print("HARD-GATE REGRESSIONS (fail):")
        for line in regressions:
            print(f"  {line}")
        for line in render_failures:
            print(f"  RENDER FAILURE {line}")
        return 1
    print("Hard gate OK: no regressions vs baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
