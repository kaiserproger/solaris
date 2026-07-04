# Validation Coverage Audit

**Status:** M95 snapshot plus 2026-06-13 static-review note and 2026-06-18
live-audit alignment. Quality label:
`stabilization`.

This report is a committed M95 snapshot derived from the frozen denominator
table in [`VALIDATION_LEDGER.md`](VALIDATION_LEDGER.md). Reproduce the raw
row audit with:

```sh
cargo run -p mc-test-harness --bin coverage-audit -- docs/VALIDATION_LEDGER.md
```

## Conservative Counting Rule

A row counts toward the 80% M100 target only when all of these are true:

- The row is in the frozen M100 denominator.
- The row status is `ready`.
- The evidence contains focused runtime coverage.
- The evidence also contains separate vanilla oracle or real-client evidence
  for that exact row.

Rows with only unit tests, Solaris-only harness coverage, bare capture,
wire-probe capture, Solaris harness capture, wire-probe-only or
protocol-metadata-only evidence, negated evidence such as "no oracle evidence",
partial implementation, implementation existence, missing/manual-pending
evidence, blocked gates, unknown status, draft debt, accepted divergence, or
non-goal classification do not count.

## M95 Snapshot

| Metric | Value |
|---|---:|
| In-scope denominator rows | 46 |
| Conservative numerator rows | 0 |
| Current conservative coverage | 0.00% |
| Rows needed for 80% | 37 |
| Gap to 80% | 37 rows |

## 2026-06-13 Static Review Note

The 2026-06-13 static review preserves this accounting: 46 frozen denominator
rows, 0 countable `ready` rows, and 0.00% conservative coverage. It did not run
`cargo`, a real vanilla/PrismLauncher client, or profiler/load workloads, so it
does not change the numerator.

The review describes Solaris as a strong stabilization-alpha/private vanilla-near
base, not as a release-ready vanilla replacement. The most important evidence
gaps remain real-client/oracle proof, generated-world chunk/light performance,
global `WorldStorage` and `SessionRegistry` lock ownership, entity AI/pathing,
water/swim and movement, stale block-edit/CAS safety, public auth/offline-mode,
persistence/crash/soak/autoscale, and plugin API non-goal clarity.

## 2026-06-18 Live Audit Alignment

A fresh `coverage-audit` rerun still reports 46 denominator rows, 0 conservative
`ready` rows, and 0.00% coverage. The status breakdown below reflects the
current ledger row classifications after later post-M95 row reclassifications:
29 `partial`, 5 `blocked`, 3 `unknown`, 7 `draft debt`, 2
`accepted divergence`, and 0 `ready`.

## Status Breakdown

| Status | Rows | Counts |
|---|---:|---|
| `partial` | 29 | no |
| `blocked` | 5 | no |
| `unknown` | 3 | no |
| `draft debt` | 7 | no |
| `accepted divergence` | 2 | no |
| `ready` | 0 | yes, if evidence also passes the conservative rule |

## Current Counted Rows

None. The frozen ledger has no row currently marked `ready`, so no row can
enter the conservative numerator even where useful Solaris harness, wire-probe,
manual notes, protocol metadata, or vanilla captures exist.

## Immediate Coverage Gaps

- Real-client evidence remains blocked for broad M100 accounting (`Q2`).
  The 2026-06-22 M94 pack now includes focused m94-06 same-client restart,
  two-client block-visibility, shared-drop visibility, and shared-pickup
  removal evidence, but Q2 remains blocked because broad two-client contention,
  shared container behavior, vanilla oracle, performance, and soak evidence are
  still missing.
- Systematic vanilla oracle scenario evidence remains blocked (`Q1`).
- Performance and concurrency remain blocked (`O1`, `O2`), including the M77
  generated-world join/chunk-stream stall evidence.
- The ignored generated-world chunk-stream guard now has one focused O2
  lock-pressure assertion pass: `chunk_prepare` deltas are exercised while
  in-memory save-all/player-persistence lock paths stay unchanged. It remains
  ignored, sidecar-dependent, and insufficient for disk-backed latency,
  hardware-profile, slow-client, broad lock-review, or soak coverage.
- Public/session safety, common stations, vehicles, autoscale, operator/deps,
  and data drift rows remain unknown or draft debt. O4 now has one focused
  `--check` warning regression for public-bind offline/local-dev operator
  blockers, but no backup/restore, dependency audit, data-drift, or real
  operations evidence.
- Most gameplay rows are `partial`: they have implementation and focused tests,
  including B1/B3 focused rejected occupied-target placement, occupied-target
  water-bucket fallback/resync, survival out-of-reach `UseItemOn` resync,
  early rejected survival-break target resync, scheduled water spread, and
  lava-bucket-plus-water scheduled solidification harness coverage. Those
  regressions are local-sidecar-dependent and degrade when required
  `data/vanilla/reports` files are absent; they still lack enough row-specific
  vanilla oracle or real-client evidence to count.
- I1/I2 now have one focused real-client inventory recipe run
  (`m94-03a-inventory-oak-log-to-planks`), but recipe-book UI, crafting-table
  UI, cursor recovery, containers/stations, malformed clicks, furnace-family
  and campfire client recipes, and broad recipe execution remain outside that
  evidence.
- K1/S2 stale shared-container coverage now includes storage/packet regressions
  for chest and furnace plus two-protocol-client chest and furnace stale-click
  resyncs after live peer slot updates. That is still protocol harness
  evidence, not real-client concurrent-click, broad shared-container, oracle,
  contention, performance, or soak evidence.
- Q3 malformed-action evidence now has focused unsupported/malformed chest and
  furnace protocol harnesses covering `QuickCraft`, `Clone`, and `PickupAll`
  with lying client slot/cursor deltas plus one furnace pickup with a
  conflicting non-empty post-click carried-item prediction; Solaris rolls that
  pickup back, resyncs authoritative content, and leaves backing storage
  unchanged. It remains far short of a replay corpus, invalid-wire
  fuzzer, broad malformed-action matrix, real-client evidence, or vanilla
  oracle.
- Farming, plants, campfires, bows, shields, storage/cache/persistence,
  generated exploration, and common stations remain split in the denominator;
  unit-only or Solaris-only subcoverage is excluded from the numerator.
