# Manifest-Driven Vanilla Oracle Replay Plan

**Goal:** Execute the checked core replay manifest through both vanilla and
Solaris protocol lanes and produce a real normalized pass/diff.

**Architecture:** Reuse `run_protocol_replay`, the existing local vanilla
process owner, and the existing in-memory Solaris setup. The test remains
ignored by default because it requires `.analysis/server.jar` and Java 25, but
this checkpoint runs it explicitly.

## Tasks

- [x] Add one ignored integration test loading the checked manifest and running
  `VanillaOracle` plus `SolarisProtocol` lanes.
- [x] Require both sides to expose inventory, action-order, and liveness facts,
  then require an empty normalized diff and print a unique positive marker.
- [x] Run the exact ignored test with the local Java 25.0.3/server.jar oracle.
- [x] Run the M79 oracle suite path or update its manifest routing so the new
  checked scenario cannot be silently omitted.
- [x] Record exact oracle evidence and limitations in M79 and the ledger.
- [x] Run focused/full Cargo gates and scoped diff review.
