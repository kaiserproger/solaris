# Validation Coverage Audit

**Status:** M95 snapshot. Quality label: `stabilization`.

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

## Status Breakdown

| Status | Rows | Counts |
|---|---:|---|
| `partial` | 28 | no |
| `blocked` | 5 | no |
| `unknown` | 7 | no |
| `draft debt` | 4 | no |
| `accepted divergence` | 2 | no |
| `ready` | 0 | yes, if evidence also passes the conservative rule |

## Current Counted Rows

None. The frozen ledger has no row currently marked `ready`, so no row can
enter the conservative numerator even where useful Solaris harness, wire-probe,
manual notes, protocol metadata, or vanilla captures exist.

## Immediate Coverage Gaps

- Real-client evidence remains blocked for broad M100 accounting (`Q2`).
- Systematic vanilla oracle scenario evidence remains blocked (`Q1`).
- Performance and concurrency remain blocked (`O1`, `O2`), including the M77
  generated-world join/chunk-stream stall evidence.
- Public/session safety, common stations, vehicles, autoscale, operator/deps,
  and data drift rows remain unknown or draft debt.
- Most gameplay rows are `partial`: they have implementation and focused tests,
  but not enough row-specific vanilla oracle or real-client evidence to count.
- Farming, plants, campfires, bows, shields, storage/cache/persistence,
  generated exploration, and common stations remain split in the denominator;
  unit-only or Solaris-only subcoverage is excluded from the numerator.
