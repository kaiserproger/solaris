# Phase 1 flaky-test and environment-sensitive success inventory

Scope: the final Phase 1 inventory of first-party retry, quarantine,
serial-only, disabled, and environment-sensitive success patterns outside the
already classified ignored, feature-gated, local-artifact, progress-wait, and
manual-client gates.

## Inventory result

The bounded Rust scan found no retry/rerun or quarantine framework and no
`serial_test` dependency or annotation. Runtime names such as connection retry
and finite setup attempts are production or harness behavior, not automatic
test reruns. Explicit ignores, feature gates, local Mojang prerequisites, and
manual-client manifests retain their separately recorded owners and close
conditions.

The scan did expose three unexplained classes:

| Class | Finding | Closure |
| --- | --- | --- |
| `mc-worldgen::structures` local sidecars | Three ordinary tests returned success without the blocks report or plains-fountain NBT. | The three tests are explicit fail-closed opt-in gates; see [`mc-worldgen-structure-local-artifact-tests.md`](mc-worldgen-structure-local-artifact-tests.md). |
| `mc-server` local structure sidecar | Two binary-unit tests returned success without the plains-fountain NBT. | The two tests are explicit fail-closed opt-in gates; see [`mc-server-structure-local-artifact-tests.md`](mc-server-structure-local-artifact-tests.md). |
| Block-drop await-probe serialization | A process-global test probe used an async mutex only among its three installing tests, but unrelated block-drop owners could still reach it concurrently. The workspace run reproduced a closed-receiver panic. | The probe is task-local and one-shot, so the three tests no longer need serial-only execution; see [`mc-net-block-drop-await-probe.md`](mc-net-block-drop-await-probe.md). |

A separate bounded scan of 72 first-party Java/Gradle and Python/shell runner
files found no unexplained automatic rerun, quarantine, disabled-test,
assumption-skip, or allow-failure framework. It found one environment-sensitive
success path: `tools/generate-test-world.sh` accepted a lone region file as a
complete cached fixture. The guard now requires the complete non-empty
region-plus-`level.dat` shape and fails partial output; see
[`test-world-cache-guard.md`](test-world-cache-guard.md).

## Boundary and next checkpoint

This closes the unexplained Phase 1 flaky/self-skip inventory, not every
release-time test obligation. The release-candidate performance gates, the
owner-run seed-`712816` disposition, and the complete release/client matrix
remain queued at their declared boundaries. The next actionable Phase 1
checkpoint is the mechanical extraction of the remaining plant adapter tests
and helpers from aggregate `mc-net::play::tests` into the focused sibling
`play/tests/plants.rs`; the production plant-rules crate extraction is already
complete.

`benchmark: not applicable`: this inventory changes only test/fixture
classification and test-only scheduling infrastructure.

## Reproduction

The focused commands and exact artifact prerequisites live in the four linked
evidence documents. The checkpoint close additionally requires:

```sh
bash -n tools/generate-test-world.sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```
