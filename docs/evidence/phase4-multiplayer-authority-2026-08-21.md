# Phase 4 multiplayer authority / publication evidence — 2026-08-21

Target: Phase 4 item 4 in `docs/PUBLIC_ALPHA_PLAN.md`: prove shared-state authority and publication with at least two real protocol clients for shared blocks, containers, combat, pickups, entity visibility, disconnect, and reconnect.

Tests marked ignored by default because they require local 26.1.2 sidecar/data fixtures were executed explicitly with `--ignored`; an ignored default listing is not treated as evidence by itself.

| Required domain | Fresh executable evidence | Disposition |
| --- | --- | --- |
| Shared blocks | `cargo test -p mc-test-harness --test block_edit stale_survival_break_cannot_break_peer_replacement -- --ignored --nocapture` — PASS (`1/1`). One client invalidates another client's survival-break snapshot by breaking/replacing the target; the stale completion cannot mutate the peer replacement and resynchronizes authoritative state. | Proven over two TCP clients with a stale-owner race, not merely a sequential happy path. |
| Containers | `two_clients_stale_chest_click_after_peer_update_resyncs` and `two_clients_stale_furnace_click_after_peer_update_resyncs`, both run explicitly with `--ignored --nocapture` — PASS (`1/1` each). | Shared chest and furnace state IDs reject/resync stale peer clicks after another client's committed update. |
| Combat | `cargo test -p mc-test-harness --test block_edit melee_pvp_damages_only_the_observed_target_player_over_wire -- --nocapture` — PASS (`1/1`). | Alice attacks Bob by the entity identity she actually observed; Bob alone receives the health transition while attacker/target publication fences remain distinct. |
| Pickups | `cargo test -p mc-test-harness --test block_edit near_full_inventory_partially_picks_up_and_preserves_remainder_identity -- --nocapture` — PASS (`1/1`). A second TCP client (`PartialDropper`) creates the item entity; the first client partially picks it up, receives only admissible inventory credit, and observes the same entity identity with the exact remainder rather than a fabricated full take/removal. | Cross-client item ownership/credit and partial-pickup publication proven. |
| Entity visibility | `cargo test -p mc-test-harness --test mob_presence two_clients_receive_same_server_owned_mob -- --ignored --nocapture` — PASS (`1/1`); `player_presence -- --ignored` also passes the two-client player spawn/move/despawn case. | Two observers receive one server-owned mob identity; player visibility uses the same authoritative publication model. |
| Disconnect / reconnect | `cargo test -p mc-test-harness --test player_presence -- --ignored --nocapture` — PASS (`2/2` total), including `disconnect_reconnect_replaces_player_visibility_cleanly`. | Old visible-player identity is removed and the reconnect materializes the replacement cleanly without duplicate visibility. |

## Fresh command results

- PvP: PASS (`1/1`, ~1.12 s).
- Shared chest: PASS (`1/1`, ~1.11 s, explicit ignored-fixture execution).
- Shared furnace: PASS (`1/1`, ~1.05 s, explicit ignored-fixture execution).
- Stale shared block race: PASS (`1/1`, ~8.44 s, explicit ignored-fixture execution).
- Shared mob visibility: PASS (`1/1`, ~1.57 s, explicit ignored-fixture execution).
- Player visibility + reconnect: PASS (`2/2`, ~2.15 s, explicit ignored-fixture execution).
- Cross-client partial pickup: PASS (`1/1`, ~18.65 s).

## Boundary

This closes the named Phase 4 item-4 authority/publication matrix. It does not promote unrelated release-candidate real-client long-soak scenarios, side-by-side vanilla parity rows, or Phase-4 item-6 fresh-world survival/restart acceptance; those remain separate gates.
