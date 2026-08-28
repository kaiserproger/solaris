# Player-combat crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

The first bounded player-combat extraction moves protocol-neutral combat math out of `mc-net::play::combat` without moving inventory mutation, packet-owned stacks, session authority, or publication.

`mc-entity::player_combat_26_1_2` now owns:

- melee and shield-block knockback math;
- horizontal look-direction calculation from Minecraft yaw;
- shield durability damage calculation;
- shield-disable duration conversion from validated item component values.

`mc-data::item_semantics_26_1_2` now owns:

- durability-tool path classification;
- versioned vanilla maximum durability for wooden/stone/iron/diamond/golden/netherite tools.

`mc-net` deliberately retains:

- `ItemStack` and `GameMode` adapters;
- live `ItemRegistry`/`ItemFactsTable` lookup;
- shield identity/stale-stack validation and inventory mutation;
- player/session combat authority, commit ordering, cooldown state, durability publication, and outbound packets.

## Correctness fences

- Direct `mc-entity` tests cover near-zero knockback rejection, ground vertical impulse, yaw direction, shield durability thresholds/non-finite input, and shield-disable tick validation.
- Direct `mc-data` tests cover tool classification and exact versioned durability constants.
- Existing `mc-net` combat tests exercise the same public adapter surface, including exact shield component duration, non-finite damage saturation, shield identity, PVP/projectile authority, durability, and publication ordering.
- `xtask code-health` requires the lower owners and rejects restoring local knockback math or bypassing the lower combat/item primitives.
- Generic lower-crate reverse-dependency and transport/session leakage guards apply.

Benchmark: not applicable. This is an ownership-only extraction of constant-time deterministic rules.

## Validation

- `cargo test -p mc-entity player_combat_26_1_2`: 3 passed.
- `cargo test -p mc-data item_semantics_26_1_2`: 1 passed.
- `cargo test -p mc-net combat::`: 15 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace --quiet`: passed; `mc-data` 233 passed/25 ignored, `mc-entity` 584 passed/6 ignored, and `mc-net` 1,932 passed/5 ignored.

No graphical/client or performance claim is made by this evidence.
