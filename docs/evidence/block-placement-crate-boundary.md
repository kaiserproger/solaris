# Block-placement crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

`mc-data::block_placement_26_1_2` owns protocol-neutral deterministic placement
math for Java Edition 26.1.2.

Owned rules:

- direction primitives: horizontal axis classification, opposite, and
  counter-clockwise rotation;
- stair shape resolution (`resolve_stair_shape`) from a neutral
  `StairNeighborState` of the four horizontal cells;
- slab merge into a dry double slab (`merge_slab_state` / `can_merge_slab`);
- waterlogged placement (`apply_waterlogged_state`);
- torch placement state for a clicked face (`torch_state_for_direction`);
- sign orientation for a clicked face and player yaw (`sign_state_for_direction`).

`mc-net` retains world snapshot access, support checks (`is_supported` /
`has_full_sturdy_face`), edit preconditions, block edit planning, inventory
settlement, owner commit, and publication. `mc-net` converts registry states to
the neutral `PlacementBlockState`/`StairNeighborState` at the boundary and
reapplies resolved properties through the registry.

## Correctness fences

- `crates/xtask` `code-health` fence enforces that the owner module exports
  `resolve_stair_shape`, `merge_slab_state`, `apply_waterlogged_state`,
  `torch_state_for_direction`, and `sign_state_for_direction`, and that
  `mc-net/src/play/block_placement.rs` does not reimplement them or the
  direction primitives.
- Placement rules have focused lower tests in `mc-data`.
- Existing block placement/NBT tests remain the authority regression surface.
- World mutation and commit ownership stay in `mc-net`.
