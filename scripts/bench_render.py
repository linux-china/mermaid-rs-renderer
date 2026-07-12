#!/usr/bin/env python3
"""Compare end-to-end PNG render latency for mmdr and mermaid-cli."""

from __future__ import annotations

import argparse
import os
import shlex
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def run_quiet(command: list[str]) -> None:
    subprocess.run(
        command,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def benchmark(command: list[str], runs: int) -> list[float]:
    times = []
    for _ in range(runs):
        start = time.perf_counter()
        run_quiet(command)
        times.append(time.perf_counter() - start)
    return times


def summarize(values: list[float]) -> dict[str, float]:
    return {
        "mean": statistics.mean(values),
        "p50": statistics.median(values),
        "min": min(values),
        "max": max(values),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare end-to-end PNG render latency for mmdr and mermaid-cli"
    )
    parser.add_argument(
        "--input",
        type=Path,
        default=ROOT / "docs" / "diagrams" / "architecture.mmd",
        help="Mermaid input file",
    )
    parser.add_argument(
        "--mmdr",
        default=os.environ.get("MMDR_BIN", str(ROOT / "target" / "release" / "mmdr")),
        help="Path to the mmdr binary (default: MMDR_BIN or target/release/mmdr)",
    )
    parser.add_argument(
        "--mmdc",
        default=os.environ.get("MMD_CLI", "npx -y @mermaid-js/mermaid-cli"),
        help="mermaid-cli command",
    )
    parser.add_argument(
        "--puppeteer-config",
        type=Path,
        default=Path(os.environ["MMD_PUPPETEER_CONFIG"])
        if os.environ.get("MMD_PUPPETEER_CONFIG")
        else None,
        help="Optional Puppeteer JSON config (or set MMD_PUPPETEER_CONFIG)",
    )
    parser.add_argument("--runs", type=int, default=5, help="Measured runs per renderer")
    parser.add_argument("--warmup", type=int, default=1, help="Warmup runs per renderer")
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if args.warmup < 0:
        parser.error("--warmup cannot be negative")
    return args


def main() -> int:
    args = parse_args()
    if not args.input.is_file():
        raise SystemExit(f"input file not found: {args.input}")

    with tempfile.TemporaryDirectory(prefix="mmdr-bench-render-") as temp_dir:
        output_dir = Path(temp_dir)
        rust_output = output_dir / "mmdr.png"
        mmdc_output = output_dir / "mmdc.png"
        rust_command = [
            args.mmdr,
            "-i",
            str(args.input),
            "-o",
            str(rust_output),
            "-e",
            "png",
        ]
        mmdc_command = shlex.split(args.mmdc)
        if args.puppeteer_config is not None:
            mmdc_command.extend(["-p", str(args.puppeteer_config)])
        mmdc_command.extend(["-i", str(args.input), "-o", str(mmdc_output)])

        for _ in range(args.warmup):
            run_quiet(rust_command)
            run_quiet(mmdc_command)

        rust_times = benchmark(rust_command, args.runs)
        mmdc_times = benchmark(mmdc_command, args.runs)

    print("Rust renderer (seconds):", ", ".join(f"{value:.3f}" for value in rust_times))
    print("Mermaid CLI (seconds):", ", ".join(f"{value:.3f}" for value in mmdc_times))
    print("\nSummary (seconds):")
    print("Rust renderer:", summarize(rust_times))
    print("Mermaid CLI:", summarize(mmdc_times))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
