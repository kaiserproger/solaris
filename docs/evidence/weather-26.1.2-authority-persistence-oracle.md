# Weather authority, persistence, and Minecraft 26.1.2 wire oracle

Status: Phase-4 bounded common-play evidence for authoritative command-controlled weather.

This closes the old `weather-disabled` implementation gap. It does **not** claim the autonomous vanilla random weather scheduler or `/weather ... <duration>` override lifecycle; those remain explicitly unimplemented rather than approximated.

## Implemented authority

`SessionRegistry` now owns weather independently of client connections:

- target kind: `clear`, `rain`, or `thunder`;
- exact `f32` rain level;
- exact `f32` thunder level;
- `0.01` level movement per simulation tick;
- reliable publication to every active session;
- complete current-state projection for a client joining while weather is already active.

Weather ticks inside the existing simulation-owner `advance_world_time` turn. The root entity ticker did not receive a new scheduling branch.

The player and console command surfaces expose:

```text
/weather clear
/weather rain
/weather thunder
```

The command tree is operator-only. Duration syntax remains fail-closed until the separate natural weather scheduler is modelled.

## Exact 26.1.2 GameEvent behavior

The ignored local-oracle gate:

```text
cargo test -p mc-test-harness --test parity_oracle \
  weather_game_events_match_vanilla_26_1_2 -- --ignored --exact --nocapture
```

runs a local Mojang Minecraft 26.1.2 server and Solaris side by side and compares the complete weather `GameEvent` sequence.

Final result on 2026-08-18:

```text
1 passed / 0 failed
```

The oracle established the following exact packet behavior:

- rain and thunder intensity move by `0.01` per tick using the same `f32` accumulation/rounding as vanilla;
- `GameEvent` reason `7` is rain-level change;
- reason `8` is thunder-level change;
- reason `1` is the raining-state start event;
- reason `2` is the raining-state stop event;
- when rain crosses the 20% client-state threshold, vanilla first sends the ordinary level update and then sends the state event followed by a full rain/thunder level snapshot;
- Solaris reproduces that duplicate snapshot ordering rather than normalizing it away.

The checked oracle compares three full transitions:

| Transition | Compared event shape | Result |
| --- | --- | --- |
| clear -> rain saturation | 101 rain-level updates plus the three-event state/full-level sync | PASS |
| saturated rain -> thunder saturation | complete thunder-level ramp | PASS |
| saturated thunder -> clear | rain and thunder fade plus stop/full-level sync | PASS |

This test originally exposed two real incompatibilities: Solaris first jumped levels directly to `1.0`, and the initial reason-1/reason-2 names were reversed. Both were corrected before the final PASS.

## Ordinary raw-TCP gate

The normal harness test:

```text
cargo test -p mc-test-harness --test commands \
  command_tree_gamemode_and_feedback_round_trip -- --exact --nocapture
```

now executes `/weather rain`, requires normal command feedback, and then waits for a positive `RAIN_LEVEL_CHANGE` emitted by the production simulation ticker. Final result:

```text
1 passed / 0 failed
```

This makes weather regressions visible in the ordinary test suite without requiring the Mojang sidecar.

## Persistence and restart

World metadata now persists:

- target weather kind;
- exact rain-level `f32` bits;
- exact thunder-level `f32` bits.

Missing weather fields in an older Solaris world default to clear weather. Invalid kinds or non-finite/out-of-range levels fail closed.

Focused gates on the final weather tree:

```text
cargo test -p mc-net weather --quiet
4 passed / 0 failed

cargo test -p mc-net world_metadata --quiet
5 passed / 0 failed

cargo test -p mc-net save_all_then_bind_restores_world_time_and_item_entities -- --nocapture
1 passed / 0 failed
```

The save/rebind gate now saves a partially ramped thunder state and requires the rebound server to restore both the target kind and exact intermediate rain/thunder levels.

## Duration and autonomous weather boundary

A temporary Mojang 26.1.2 oracle probe was used to inspect:

```text
weather rain 2s
```

After the explicit 40-tick rain override expired, vanilla resumed a pre-existing natural weather scheduler state: rain began fading while thunder began increasing. Therefore command duration is not equivalent to `sleep N ticks; set clear`, and implementing it that way would create false parity.

Solaris consequently rejects duration syntax for now and does not claim autonomous random weather cycling. The supported alpha contract is authoritative, persisted, multiplayer, command-controlled weather with vanilla-exact client transition packets.

## Structural/performance boundary

Weather adds constant-size atomic state and a bounded three-packet state-sync burst at threshold crossings. Normal ramp ticks emit only the level fields that changed. No world/entity scan, new lock class, or new entity-ticker scheduling branch was introduced.
