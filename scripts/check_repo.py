#!/usr/bin/env python3
"""Validate repository documentation, tooling, packaging, and CI policy."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def check_markdown_links(errors: list[str]) -> None:
    link_pattern = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
    for path in [ROOT / "README.md", *sorted((ROOT / "docs").glob("*.md"))]:
        for match in link_pattern.finditer(path.read_text(encoding="utf-8")):
            target = match.group(1).strip().split("#", 1)[0]
            if not target or "://" in target or target.startswith("mailto:"):
                continue
            if not (path.parent / target).exists():
                errors.append(f"{path.relative_to(ROOT)}: missing link target {target}")


def check_versions(errors: list[str]) -> None:
    cargo = tomllib.loads(read("Cargo.toml"))
    version = cargo["package"]["version"]
    expected = {
        "README.md": [
            rf'mermaid-rs-renderer = "{re.escape(version)}"',
            rf'mermaid-rs-renderer = \{{ version = "{re.escape(version)}", default-features = false \}}',
        ],
        "src/lib.rs": [
            rf'mermaid-rs-renderer = \{{ version = "{re.escape(version)}", default-features = false \}}'
        ],
    }
    for path, patterns in expected.items():
        text = read(path)
        for pattern in patterns:
            if re.search(pattern, text) is None:
                errors.append(f"{path}: crate dependency example is not synchronized to {version}")

    release = read("docs/release.md")
    if re.search(r"git (?:tag|push origin) v\d+\.\d+\.\d+", release):
        errors.append("docs/release.md: release commands must derive the version, not hardcode it")
    stale_claim = "remains **100x+ faster** even for 200-node diagrams"
    if stale_claim in read("README.md"):
        errors.append("README.md: 200-node speedup claim contradicts the benchmark table")


def check_python_tools(errors: list[str]) -> None:
    for path in sorted((ROOT / "scripts").glob("*.py")):
        try:
            result = subprocess.run(
                [sys.executable, str(path), "--help"],
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
                timeout=20,
            )
        except subprocess.TimeoutExpired:
            errors.append(f"{path.relative_to(ROOT)}: --help timed out")
            continue
        if result.returncode != 0:
            detail = " ".join(result.stderr.split())[:240]
            errors.append(f"{path.relative_to(ROOT)}: --help failed: {detail}")

    scripts = "\n".join(path.read_text(encoding="utf-8") for path in (ROOT / "scripts").glob("*" ) if path.is_file())
    if re.search(r"/(?:home|Users)/[^/]+/", scripts):
        errors.append("scripts/: developer-specific absolute home path found")
    bench_render = read("scripts/bench_render.py")
    if 'Path("/tmp/' in bench_render or "Path('/tmp/" in bench_render:
        errors.append("scripts/bench_render.py: fixed Unix temporary path found")
    bench_batch = read("scripts/bench_batch.py")
    if "tempfile.gettempdir()" in bench_batch:
        errors.append("scripts/bench_batch.py: fixed shared temp paths are not concurrency-safe")
    if "shlex.split(args.mmdc)" not in bench_batch:
        errors.append("scripts/bench_batch.py: mermaid-cli command must preserve quoted arguments")
    profile_summary = read("scripts/profile_summary.py")
    if "NamedTemporaryFile" in profile_summary and "delete=False" in profile_summary:
        errors.append("scripts/profile_summary.py: timing outputs leak temporary files")
    for path in sorted((ROOT / "scripts").glob("*.py")):
        if path.name == Path(__file__).name:
            continue
        text = path.read_text(encoding="utf-8")
        if re.search(r'\["cargo", "build", "--release"', text):
            errors.append(f"{path.relative_to(ROOT)}: automated Cargo build must use --locked")


def check_package_metadata(errors: list[str]) -> None:
    package = json.loads(read("package.json"))
    lock = json.loads(read("package-lock.json"))
    if package.get("private") is not True:
        errors.append("package.json: development-only package must be private")
    if package.get("name") != lock.get("name"):
        errors.append("package-lock.json: root name differs from package.json")
    lock_root = lock.get("packages", {}).get("", {})
    if package.get("name") != lock_root.get("name"):
        errors.append("package-lock.json: package root name differs from package.json")
    if package.get("dependencies") != lock_root.get("dependencies"):
        errors.append("package-lock.json: root dependencies differ from package.json")
    if not (ROOT / "flake.lock").is_file():
        errors.append("flake.lock: missing reproducible Nix input lock")

    dependabot = read(".github/dependabot.yml")
    for ecosystem in ("cargo", "github-actions", "npm"):
        if f"package-ecosystem: {ecosystem}" not in dependabot:
            errors.append(f".github/dependabot.yml: missing {ecosystem} updates")

    baseline = json.loads(read("tests/quality_baseline.json"))
    baseline_paths = [baseline.get("config", ""), *baseline.get("fixtures", [])]
    baseline_paths.extend(baseline.get("metrics", {}).keys())
    for value in baseline_paths:
        normalized = str(value).replace("\\", "/")
        if normalized.startswith("/") or re.match(r"^[A-Za-z]:/", normalized):
            errors.append("tests/quality_baseline.json: machine-specific absolute path found")
            break
    quality_gate = read("scripts/quality_gate.py")
    if '"config": stable_fixture_key(config_path)' not in quality_gate:
        errors.append("scripts/quality_gate.py: baseline writer must emit portable paths")


def check_workflows(errors: list[str]) -> None:
    ci = read(".github/workflows/ci.yml")
    release = read(".github/workflows/release.yml")
    required_ci = [
        "python3 scripts/check_repo.py",
        "docker://rhysd/actionlint:1.7.12",
        "shellcheck scripts/remote-cargo.sh .github/scripts/*.sh",
        "python3 -m compileall -q scripts",
        "os: [macos-latest, windows-latest]",
        "cachix/install-nix-action@v31.10.7",
        "nix flake check --all-systems --no-build --print-build-logs",
        "nix build .#packages.x86_64-linux.default --print-build-logs",
        "cargo test --locked --all-features --lib --bins --tests --examples",
    ]
    for item in required_ci:
        if item not in ci:
            errors.append(f".github/workflows/ci.yml: missing {item}")
    if "cargo test --all-targets --all-features" in ci:
        errors.append(".github/workflows/ci.yml: test command unintentionally includes criterion benches")

    required_release = [
        "contents: read",
        "verify-release:",
        "fetch-depth: 0",
        "git merge-base --is-ancestor",
        "cargo build --locked --release --features cli",
        "cargo publish --dry-run --locked",
        'case "$status" in',
        "SHA_LINUX_ARM",
    ]
    for item in required_release:
        if item not in release:
            errors.append(f".github/workflows/release.yml: missing {item}")
    build_section = release.split("\n  build:", 1)[-1].split("\n  publish-crate:", 1)[0]
    if "permissions:\n      contents: write" not in build_section:
        errors.append(".github/workflows/release.yml: only the release-asset job should receive contents: write")
    release_docs = read("docs/release.md")
    for secret in ("CARGO_REGISTRY_TOKEN", "HOMEBREW_TAP_TOKEN", "AUR_SSH_KEY"):
        if secret not in release_docs:
            errors.append(f"docs/release.md: missing setup for {secret}")


def check_release_generators(errors: list[str]) -> None:
    with tempfile.TemporaryDirectory(prefix="mmdr-release-check-") as temp_dir:
        root = Path(temp_dir)
        (root / "homebrew-mmdr" / "Formula").mkdir(parents=True)
        env = os.environ | {
            "VERSION": "9.8.7",
            "SHA_MACOS_INTEL": "a" * 64,
            "SHA_MACOS_ARM": "b" * 64,
            "SHA_LINUX_INTEL": "c" * 64,
            "SHA_LINUX_ARM": "d" * 64,
        }
        result = subprocess.run(
            ["bash", str(ROOT / ".github/scripts/update-homebrew.sh")],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            errors.append(f"update-homebrew.sh failed: {' '.join(result.stderr.split())[:240]}")
            return
        formula = (root / "homebrew-mmdr" / "Formula" / "mmdr.rb").read_text(encoding="utf-8")
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
            if target not in formula:
                errors.append(f"update-homebrew.sh: generated formula lacks {target}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    errors: list[str] = []
    check_markdown_links(errors)
    check_versions(errors)
    check_python_tools(errors)
    check_package_metadata(errors)
    check_workflows(errors)
    check_release_generators(errors)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print("repository checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
