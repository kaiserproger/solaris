# `mc-world` local-artifact test classification

Scope: Phase 1 inventory of `mc-world` tests that require ignored local Mojang
data or generated oracle worlds.

Fifteen ordinary tests previously returned successfully when a prerequisite
was absent. That made the same Cargo result mean either “assertions passed” or
“nothing ran,” depending on the host. One of the fifteen also asserted a
wall-clock storage threshold, so its presence in the ordinary correctness
suite made that suite host-load-sensitive.

The tests are now explicit opt-in gates. Ordinary Cargo runs report them as
ignored, and an explicit ignored-test invocation fails immediately when its
declared prerequisite or oracle shape is missing.

## Inventory

| Boundary | Tests | Count | Owner and exact close condition |
| --- | --- | ---: | --- |
| Block registry against the real 26.1.2 report | `block::tests::round_trip_real_blocks_report` | 1 | `mc-world::block` with the `mc-data` report loader. Close only when the explicit ignored test passes against the locally extracted target-version `blocks.json`. |
| Anvil region compression and byte round-trip | `anvil::region::tests::{reads_real_vanilla_region, round_trip_real_vanilla_region, reads_real_vanilla_lz4_region, lz4_block_api_matches_real_vanilla_lz4_payload}` | 4 | `mc-world::anvil::region`. The first pair requires `.analysis/test-world`; the LZ4 pair requires `.analysis/test-world-lz4` generated with compression byte 4. Each explicit run must pass or fail its missing artifact. |
| Real chunk NBT round-trip, extras, mutation, and baked light | `anvil::chunk_nbt::tests::{round_trip_real_vanilla_chunks, real_test_world_carries_dropped_root_fields_in_extras, round_trip_modified_chunk_through_disk, real_test_world_carries_some_baked_skylight}` | 4 | `mc-world::anvil::chunk_nbt`. Close against a vanilla-generated local region plus the exact blocks report; the extras and skylight gates additionally require those oracle properties to be present. |
| Lighting over a real spawn chunk | `light::tests::engine_runs_on_real_spawn_chunk_when_data_present` | 1 | `mc-world::light`. Close when the explicit run finds the local region, blocks and block-light reports, selects a `Status:full` chunk, and passes the lighting invariants. |
| Real-world storage, cache, dirty-state, and timing | `storage::tests::{opens_real_test_world_and_queries_blocks, region_cache_holds_one_region_across_quadrant_walk, spawn_burst_load_does_not_dirty_chunks, streams_view_distance_quadrant_within_budget}` | 4 | `mc-world::storage`. The first three close on their explicit local-world runs. The final test is a performance probe: run it only at its mapped performance boundary on a documented host; its ten-second ceiling is not ordinary correctness evidence. |
| Real chunk wire encoding | `wire::tests::encodes_real_test_world_chunk_zero_zero` | 1 | `mc-world::wire`. Close when the explicit run opens the local world, loads the target-version data sidecars, encodes chunk `(0,0)`, and passes its structural packet checks. |

Total: 15 explicit local-artifact gates.

The same modules retain always-executable synthetic coverage for registry
validation, gzip/zlib/uncompressed/LZ4 region decoding, chunk NBT
round-trips/extras/light arrays, lighting invariants, temporary-world
storage/cache/dirty behavior, and chunk wire encoding. The opt-in tests extend
that coverage to local Mojang artifacts; they do not replace it.

## Current disposition

A fresh pre-change
`cargo test --workspace --all-targets --no-fail-fast` run reported zero
failures, but it could not distinguish executed local assertions from
successful early returns. The focused post-change `mc-world` suite reports
`215 passed; 0 failed; 15 ignored`, making the boundary visible.

No ignored oracle or performance workload was reproduced in this checkpoint.
Missing `.analysis` or `data/vanilla` artifacts are now failures when an
operator explicitly selects one of these tests.

`benchmark: not applicable`: the checkpoint changes test classification only,
not a runtime path, and makes no performance claim.

## Reproduction

Run the always-executable suite:

```sh
cargo test -p mc-world
```

After preparing the exact local artifacts, select only the relevant opt-in
gate, for example:

```sh
cargo test -p mc-world \
  storage::tests::streams_view_distance_quadrant_within_budget \
  -- --ignored --exact --nocapture
```

Do not use `cargo test -- --ignored` as an undifferentiated benchmark batch.
