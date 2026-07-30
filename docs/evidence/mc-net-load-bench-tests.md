# `mc-net` `load-bench` feature-gate classification

Scope: Phase 1 feature-gated test inventory for `crates/mc-net`.

`mc-net` defines `load-bench` as a performance-harness boundary that also
enables `mc-script/lua-runtime`. The feature exposes the benchmark-only server
handle and reports, bulk entity seeding and readiness snapshots, per-command
timing, and entity-goal phase diagnostics. These items are unavailable in the
normal server build.

## Inventory

Comparing the two Cargo test lists yields no additional tests:

| Configuration | Unit-test entries | Doc-test entries | Feature-only entries |
| --- | ---: | ---: | ---: |
| default | 1,853 | 3 | 0 |
| `--features load-bench` | 1,853 | 3 | 0 |

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
| Luau runtime availability for the benchmark server | `mc-script/lua-runtime`; production runtime correctness remains owned by the explicit `mc-script` feature suite. The `mc-net` gate proves only that the benchmark-enabled network crate composes with it. |

No test in this classification becomes graphical, network-dependent,
self-skipping, or dependent on local Mojang data when the feature is enabled.
The feature does expose an opt-in performance harness, but normal Cargo test
execution does not run that workload.

## Current disposition

The current tree produced the same result in both configurations:

- default: `1,848 passed; 0 failed; 5 ignored`, plus `3` passing doc tests;
- `--features load-bench`: `1,848 passed; 0 failed; 5 ignored`, plus `3`
  passing doc tests.

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
