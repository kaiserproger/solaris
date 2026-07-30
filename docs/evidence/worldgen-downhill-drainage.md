# Revision-10 downhill drainage evidence

Date: 2026-07-30

Checkpoint base: `53e938e510a9ed852d859090f179503d6f71f42f`

Adapted source revision: `7054417587144176e94fc7b8bc28d290568f936d`

Candidate code diff SHA-256:
`919033428a6ff682ca83c75bef62cb635b7c73e78ab92c6c1a2508f5d7cec288`

## Drainage boundary

Revision-10 drainage previously selected one of three globally forward
neighbours by branch score. That made the graph acyclic through a monotonic
rank, but the rank was not an explicit terrain-facing hydraulic elevation.

Each coarse cell now has a deterministic seeded hydraulic elevation made from
the monotonic flow rank, bounded local relief, and basin distance. The negative
rank component decreases by at least `1.0` along every candidate edge; basin
relief can oppose that by at most `0.82` and the two endpoint local-relief
terms by at most `0.16`. Their combined opposition stays below the mandatory
drop, so all three forward candidates remain strictly downhill.
`downstream` enforces that invariant before applying the existing deterministic
branch score.

Accumulation depth increases from two to three upstream levels so confluences
carry another bounded generation. With local runoff in `0.72..=1.28`, the
maximum recursive tree is `1 + 3 + 9 + 27` cells and the exact upper bound is
`51.2`. Sampling remains deterministic, seed-sensitive, and independent of
chunk traversal order and the positive/negative chunk border.

This checkpoint changes only the drainage vertical. Rendered height/biome/
vegetation mosaics, the clean seed-`712816` owner playtest, restart evidence,
and the release-host 225-chunk comparison remain separate open gates.

## Mapped debug probes

Environment:

- Linux `7.0.0-28-generic`, x86_64.
- AMD Ryzen 5 7535HS, 12 visible logical CPUs.
- `rustc 1.94.1 (e408947bf 2026-03-25)`, LLVM 21.1.8.
- Cargo debug/test profile.

Both mapped probes use the synthetic registry and generate the exact seed-42
5×5 chunk window. The stage profile reports column planning, fill, caves,
ores, biomes, structures, and decorations without a threshold. The throughput
probe requires all 25 chunks to finish with `minecraft:full` status under its
10-second debug ceiling.

Stage profile:

- column planning: 45 ms (8.7%);
- fill: 71 ms (13.6%);
- caves: 177 ms (33.7%);
- ores: 214 ms (40.7%);
- biomes: 6 ms (1.2%);
- structures: 0 ms (0.0%);
- decorations: 10 ms (2.0%);
- total: 526 ms.

Throughput: all 25 chunks completed in 526 ms (`47.5 chunks/s`), passed the
10-second debug ceiling, and retained `minecraft:full` status.

## Validation

- `cargo test -p mc-worldgen drainage`: 8 passed.
- `cargo test -p mc-worldgen
  downstream_strictly_decreases_hydraulic_elevation`: passed.
- `cargo test -p mc-worldgen
  terrain::tests::generated_spawn_window_debug_stage_profile -- --exact
  --include-ignored --nocapture`: passed.
- `cargo test -p mc-worldgen
  terrain::tests::generated_spawn_window_debug_budget_reports_throughput --
  --exact --include-ignored --nocapture`: passed.
- `cargo test -p mc-worldgen`: 106 passed, 2 ignored; 12 external tests
  passed.
- `cargo test -p mc-test-harness --test worldgen`: 3 passed, 3 ignored.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

Independent read-only review verdict: `pass`; no blocking findings.

No graphical/client or owner-play gate was run. This is deterministic unit and
debug-probe evidence, not rendered-quality, subjective-play, restart, release-
host throughput, or release-readiness evidence.
