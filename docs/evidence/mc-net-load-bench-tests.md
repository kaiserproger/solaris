# `mc-net` `load-bench` feature-gate classification

Scope: Phase 1 feature-gated test inventory for `crates/mc-net`.

`mc-net` defines `load-bench` as a performance-harness boundary. The feature
exposes the benchmark-only server handle and reports, bulk entity seeding and
readiness snapshots, per-command timing, and entity-goal phase diagnostics.
These items are unavailable in the normal server build. Since WASM migration
P1, it no longer enables a VM: retained Luau is selected by the server through
`mc-plugin-host/legacy-luau`; `mc-net` uses that host only as a development
dependency for integration coverage.

## Inventory

The counts below are the historical inventory, not a refreshed P1 test count.

Comparing the two Cargo test lists yields no additional tests:

| Configuration | Unit-test entries | Doc-test entries | Feature-only entries |
| --- | ---: | ---: | ---: |
| default | 1,857 | 3 | 0 |
| `--features load-bench` | 1,857 | 3 | 0 |

There is no test module, `#[test]`, or `#[tokio::test]` gated only by
`load-bench`. The five ignored tests present in both lists are the already
classified `mc-net` performance and local-parity gates recorded in
[`mc-net-ignored-tests.md`](mc-net-ignored-tests.md); enabling this feature
does not add or hide an ignored test.

The exact owning gate is therefore the explicit feature build and package
suite, not a separate benchmark test count:

| Gated boundary | Owner and exact close condition |
| --- | --- |
| Public `LoadBenchHandle`, entity specification, seed/readiness/activity reports, and simulation-command statistics | `mc-net::server`; after changing the benchmark API or server wiring, the explicit feature suite must compile and pass while the default suite remains green. |
| Bulk entity seeding plus readiness/activity snapshots | `mc-net::play::session::load_bench`; after changing session/entity ownership or visibility publication, the explicit feature suite must pass and the mapped entity-scale benchmark must be re-run only when performance evidence is required. |
| Simulation command timing and entity-goal phase diagnostics | `mc-net::play::simulation` and `mc-net::play::session::entity_simulation`; after changing the instrumented command or goal pipeline, the explicit feature suite must compile without altering default-build behavior. Performance claims require the separately mapped benchmark, not this correctness gate. |
| Retained Luau runtime for integration coverage | `mc-plugin-host/legacy-luau`, selected by `mc-net` only as a development dependency. Runtime correctness belongs to the host feature suite; `load-bench` itself selects no VM. |

No test in this classification becomes graphical, network-dependent,
self-skipping, or dependent on local Mojang data when the feature is enabled.
The feature does expose an opt-in performance harness, but normal Cargo test
execution does not run that workload.

## Current disposition

The original classification run passed both configurations with 1,848 unit
tests passing, five ignored, and three passing doc tests. Four default-visible
unit tests were added later.

The 2026-07-30 current-tree inventory at
`a20bfd6179d9942093b4eabfbefff02b46937e70` listed 1,857 unit tests and three
doc tests in both configurations, with an empty sorted set difference. It did
not execute either suite and therefore makes no new pass claim.

The sorted set difference between the two complete test lists is empty. A
default-only package run is insufficient after editing code behind
`load-bench`; the explicit feature command below is the owning compile and
correctness gate.

These package tests do not prove entity-scale throughput, tail latency, or
manual-client behavior. No benchmark was run in this checkpoint because it
changes no runtime path and makes no performance claim.

## Reproduction

List and run both configurations explicitly:

```sh
cargo test -p mc-net -- --list
cargo test -p mc-net

cargo test -p mc-net --features load-bench -- --list
cargo test -p mc-net --features load-bench
```

The feature-only inventory is the sorted set difference between the two
`--list` outputs.
