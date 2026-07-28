# Public Alpha Release Plan

Target: `v0.0.1-alpha.1`

This document is the exact remaining plan for the first public Solaris alpha. It is intentionally narrower than vanilla replacement readiness. A public alpha must be installable, honestly described, reproducible from a tag, and safe enough for voluntary testing; it does not need full vanilla parity.

## Release boundary

The first public alpha targets exactly Minecraft Java Edition 26.1.2 and publishes Linux x86_64 and AArch64 server archives. It supports ordinary testing, development servers, Lua plugin experiments, and bounded multiplayer sessions.

The release explicitly does not promise:

- backward-compatible Solaris worlds, plugin APIs, or client-extension schemas between alpha versions;
- complete vanilla behavior;
- production fleet safety or zero-downtime upgrades;
- Windows/macOS binaries;
- complete species-specific attacks, zombie-villager curing, Hero pricing, village population/defence, rare redstone/vehicle parity, or broad performance envelopes.

## Required before tagging

- [x] Use SemVer prerelease version `0.0.1-alpha.1` in Cargo metadata.
- [x] Use the real repository URL in Cargo metadata.
- [x] Describe the project as a public alpha without claiming replacement readiness.
- [x] Publish exact alpha limitations and pinned install instructions.
- [x] Provide public-alpha release notes.
- [x] Use a safe starter `example.toml` (`127.0.0.1`, VD8, balanced profile, ordinary `world` directory).
- [x] Mark hyphenated GitHub releases as prereleases.
- [x] Require the Git tag to equal `v${workspace.package.version}`.
- [x] Include `example.toml`, README, licenses, and VERSION in release archives.
- [x] Keep the installer fail-closed for archive contents and checksums.
- [x] Update `Cargo.lock` for the workspace version.
- [x] `cargo fmt --all -- --check`.
- [x] `cargo run -p xtask -- code-health`.
- [x] Cover `cargo test --workspace --all-targets` through terminating package shards. The monolithic command exceeded the CodexPro wall-clock after green completed suites; every workspace package and all heavy targets then passed separately.
- [x] Compile every workspace all-target test with `RUSTFLAGS=-D warnings`.
- [x] `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] `cargo build --locked --release --workspace`.
- [x] `tools/build-loader-live-gate-fixture.sh --check`.
- [x] Loader tests for core, Fabric, NeoForge, and Forge on Java 25.
- [x] `tools/test-install.sh`.
- [x] Parse the workflow YAML and syntax-check touched shell scripts.
- [x] Build and inspect a local x86_64 release archive using the CI file list.
- [x] Run the packaged binary with `--version` and `--check --config example.toml` from an isolated directory.
- [x] Independent read-only review run. Its two closeout-sequencing findings are addressed by committing before publication step 1 is marked complete; no second reviewer is run.

## Publication sequence

Do not move the tag after publication.

- [x] Commit only the reviewed release-preparation files.
- [ ] Push `main` and require the main-branch CI run to pass.
- [ ] Create annotated tag `v0.0.1-alpha.1` on that exact green commit.
- [ ] Push the tag.
- [ ] Require both release-build matrix jobs and the GitHub Release job to pass.
- [ ] Verify the GitHub release is visibly marked **Pre-release** and contains both architectures plus checksum files.
- [ ] Install the published artifact using the README command on Linux x86_64 or AArch64.
- [ ] Verify `solaris --version`, `solaris --check --config server.toml`, and one real 26.1.2 client join.

## After the first public alpha

Treat reports as a queue of concrete client-visible failures. Fix ordinary gameplay, data loss, disconnects, security issues, and installer/release failures before adding broad systems. Keep full replacement-readiness work separate from alpha patch releases.

Recommended versioning:

- `v0.0.1-alpha.N`: fixes and bounded additions while persistence/API breakage remains normal.
- `v0.0.1-beta.N`: only after common gameplay is stable and intentional migration/version policy exists.
- `v0.1.0`: first non-prerelease only after the repository's public release contract is explicitly redefined and validated.
