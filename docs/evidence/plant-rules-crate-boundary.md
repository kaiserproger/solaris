# Plant-rules crate-boundary evidence

Date: 2026-07-30

Checkpoint base: `7369947f1c8fea7d2bcafe303d46777c6b34f1e5`

## Ownership cutover

Deterministic crop, stem, vertical-plant, bamboo, sapling/tree, bonemeal,
cocoa, survival, harvest, and drop planning moved from
`mc-net::play::plants` to `mc_world::plant_rules_26_1_2`.

The lower-crate contract contains only semantic block edits, identifier-based
item drops, horizontal directions, registries, and a read-only block lookup.
`mc-net` keeps the runtime responsibilities that need network or mutation
authority:

- snapshot acquisition and mutation preconditions;
- accepted world commit, durability, relight, and publication;
- packet directions and identifier-to-protocol item-stack translation;
- random-tick orchestration and interaction sequencing.

The superseded production `mc-net::play::plants` module was removed. Existing
`mc-net` tests remain adapter/integration coverage while focused pure-rule tests
now sit beside the lower-crate owner.

## Correctness fences

- Focused `mc-world` tests cover crop/harvest/drop semantic contracts,
  read-only vertical growth, deterministic seeded sapling planning, and
  protocol-neutral cocoa directions.
- Existing `mc-net` plant, random-tick, placement, survival, and interaction
  tests continue to cover snapshot adaptation, preconditions, commit inputs,
  item translation, and publication-facing behavior.
- `xtask code-health` pins the lower-crate module and rejects any restored
  `mc-net::play::plants` file or network, session, mutation, lock, async, and
  packet or item-registry/protocol-ID dependencies in the lower owner.

Benchmark: not applicable. This is a mechanical ownership cutover with no
algorithm, gameplay rule, or performance-contract change.

## Validation

- `cargo test -p mc-world plant_rules_26_1_2`: 4 passed.
- `cargo test -p mc-world -p mc-net`: `mc-world` 219 passed and 15 ignored;
  `mc-net` 1,849 passed and 5 ignored; 3 `mc-net` doc tests passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Independent read-only review returned `changes`: numeric item protocol IDs
  still crossed the lower-crate contract. The result DTO now carries semantic
  `Identifier` values, all registry/protocol translation moved to `mc-net`,
  and the final focused, workspace, Clippy, formatter, and code-health gates
  passed. Per review policy, the fix did not trigger a second reviewer.

The PrismLauncher graphical/client gate was not run. This checkpoint changes
crate ownership without claiming new gameplay, graphical, performance, or
release readiness evidence.
