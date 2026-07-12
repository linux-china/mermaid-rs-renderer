# Release and crates.io Publish

This repository publishes binaries and the Rust crate from a version tag.

## One-time setup

Add the release credentials to GitHub repository secrets:

1. `CARGO_REGISTRY_TOKEN`: a crates.io API token with publish permission.
2. `HOMEBREW_TAP_TOKEN`: a GitHub token that can push to `1jehuang/homebrew-mmdr`.
3. `AUR_SSH_KEY`: the private SSH key registered for the `mmdr-bin` AUR package.

Add each secret under `Settings -> Secrets and variables -> Actions`. The
release workflow publishes the crate before updating Homebrew and AUR, so test
all three credentials before creating a tag.

## Release checklist

1. Update version in `Cargo.toml`.
2. Update `CHANGELOG.md`.
3. Run local checks:
```bash
python3 scripts/check_repo.py
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked --all-features --lib --bins --tests --examples
cargo test --locked --no-default-features --lib
cargo test --locked --doc --all-features
cargo package --locked
cargo build --locked --release --features cli
python3 scripts/hard_gate.py
cargo publish --dry-run --locked
```
4. Commit and push to `master`.
5. Verify the tree is clean, derive the version from Cargo metadata, then create
   and push an annotated tag:
```bash
git diff --exit-code
git diff --cached --exit-code
version="$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
git tag -a "v${version}" -m "Release v${version}"
git push origin master "v${version}"
```

## What CI does on tag push

Workflow: `.github/workflows/release.yml`

- Builds release binaries for Linux/macOS/Windows.
- Uploads assets to GitHub Release.
- Publishes `mermaid-rs-renderer` to crates.io.
- Verifies tag version matches `Cargo.toml`.
- Verifies the tagged commit is reachable from `master` and reruns release gates.
- Skips publish if that exact crate version already exists.
- Updates the Homebrew formula for Intel/Arm macOS and Linux.
- Updates the `mmdr-bin` AUR package.

## Verify publish

```bash
cargo search mermaid-rs-renderer --limit 1
cargo info mermaid-rs-renderer
```

## Manual fallback (if needed)

If CI publish fails but the release commit and dry run are verified, set the
token only for the current shell and publish the locked dependency graph:

```bash
read -rsp "crates.io token: " CARGO_REGISTRY_TOKEN && echo
export CARGO_REGISTRY_TOKEN
cargo publish --dry-run --locked
cargo publish --locked
unset CARGO_REGISTRY_TOKEN
```
