# Cargo feature-gated test inventory

Date: 2026-07-30

Checkpoint base: `a20bfd6179d9942093b4eabfbefff02b46937e70`

## Scope

This checkpoint inventories local Cargo features that can change first-party
workspace test discovery. It revalidates the existing `mc-script`
`lua-runtime` and `mc-net` `load-bench` classifications against current
manifests, source gates, and Cargo test lists.

Dependency features selected only inside third-party crates are outside this
inventory: they do not independently hide a Solaris test target or module.

## Workspace inventory

Only two workspace packages declare local features:

| Package and feature | Default state | Test-discovery effect | Owning evidence |
| --- | --- | --- | --- |
| `mc-script/lua-runtime` | Disabled in `mc-script`; enabled by production `mc-server`, `mc-test-harness`, and the explicit feature suite | Adds 80 in-process Luau runtime tests: 85 default entries become 165 | [`mc-script-lua-runtime-tests.md`](mc-script-lua-runtime-tests.md) |
| `mc-net/load-bench` | Disabled | Adds benchmark support surfaces and enables `mc-script/lua-runtime`, but both `mc-net` configurations list the same 1,857 unit tests and three doc tests | [`mc-net-load-bench-tests.md`](mc-net-load-bench-tests.md) |

No other `crates/*/Cargo.toml` contains a `[features]` table. No workspace test
or benchmark target declares `required-features`. Every first-party
`cfg(feature = ...)` or `cfg(any(test, feature = ...))` names one of the two
features above.

`mc-script` also declares the standard empty `default = []` feature set. It
does not gate a test. Workspace-level feature unification enables
`lua-runtime` through `mc-server` and `mc-test-harness`, and enables
`load-bench` through the harness, so a workspace test list is not a
default-off comparison. The package-local commands below are the discovery
authority.

The complete current-tree list comparison is:

| Configuration | Unit entries | Doc entries | Feature-only entries |
| --- | ---: | ---: | ---: |
| `mc-script` default | 85 | 0 | — |
| `mc-script --features lua-runtime` | 165 | 0 | 80 |
| `mc-net` default | 1,857 | 3 | — |
| `mc-net --features load-bench` | 1,857 | 3 | 0 |

The `mc-script` feature-only set matches the already classified payload, core
VM, Loader, player-inventory, plugin-config, timer, and startup-worldgen
groups. The `mc-net` set difference is empty. Neither class is ignored,
self-skipping, graphical, or dependent on local Mojang data merely because
the feature is enabled.

## Reproduction

The manifest and source inventory is:

```sh
rg -l '^\[features\]' crates/*/Cargo.toml
rg -n '#\[cfg(?:_attr)?\([^]]*feature[[:space:]]*=' \
  crates --glob '*.rs'
rg -n 'required-features' Cargo.toml crates/*/Cargo.toml
```

The current lists were captured under
`.analysis/codex-logs/cargo-feature-test-inventory/` with:

```sh
cargo test -p mc-script -- --list
cargo test -p mc-script --features lua-runtime -- --list
cargo test -p mc-net -- --list
cargo test -p mc-net --features load-bench -- --list
```

Sorted test-name differences contain 80 `mc-script` entries and zero `mc-net`
entries.

## Validation

- Workspace manifest feature inventory: exactly two non-default local feature
  boundaries plus `mc-script`'s empty `default` set.
- Source `cfg(feature)` projection: only `lua-runtime` and `load-bench`.
- `required-features` test/benchmark inventory: no matches.
- Four current-tree Cargo test lists and both sorted set differences: passed.
- Markdown links and scoped path checks: passed.
- `git diff --check`: passed.

The four Cargo commands used `--list`; they compiled and enumerated tests but
did not execute them. This documentation/focused checkpoint changes no code,
so affected-package, workspace, Clippy, formatter, benchmark, and graphical
gates were not rerun.

Independent read-only review verdict: `pass`; no findings.

## Evidence boundary and next cursor

This closes only the Cargo feature-gated portion of Phase 1 item 1. The linked
documents retain each feature's owner and exact close condition. This
checkpoint does not close failing, flaky, ignored, manual-only, gameplay,
graphical, or performance gates and does not close Phase 1 item 1 itself.

Benchmark: not applicable. The checkpoint changes test inventory
documentation only and makes no performance claim.

Next: inventory retry, quarantine, serial-only, and environment-sensitive
self-skip patterns outside the already classified local-artifact gates, then
select the first unexplained flaky test class.
