# Minecraft 26.1.2 world-clock wire evidence

Date: 2026-07-28

This fact sheet records the local metadata used to implement the 26.1.2
`ClientboundSetTime` payload. It contains no Mojang-owned class bytes or source.

## Oracle

- Local client jar: `/home/kaiserroman/.gradle/caches/fabric-loom/26.1.2/minecraft-client.jar`
- SHA-256: `b1b3158572666445eff01e82fad8c7de2e4953db6d354f311730d77a8359d0b0`
- Inspection: `javap -p -c`
- Relevant classes:
  - `net.minecraft.network.protocol.game.ClientboundSetTimePacket`
  - `net.minecraft.world.clock.WorldClock`
  - `net.minecraft.world.clock.ClockNetworkState`
  - `net.minecraft.world.clock.WorldClocks`
  - `net.minecraft.client.ClientClockManager`
  - `net.minecraft.network.codec.ByteBufCodecs$29`

## Exact packet shape

`ClientboundSetTimePacket.STREAM_CODEC` is a composite of:

1. big-endian `long gameTime`;
2. a VarInt-sized map;
3. each map key through `WorldClock.STREAM_CODEC`;
4. each map value through `ClockNetworkState.STREAM_CODEC`.

`WorldClock.STREAM_CODEC` is a holder-registry codec. The local
`ByteBufCodecs$29` bytecode writes the holder's registry id as a VarInt.
`crates/mc-data/data/required_registry_index.json` records the 26.1.2
`world_clock` registry order as:

- id `0`: `minecraft:overworld`;
- id `1`: `minecraft:the_end`.

The value layout is:

1. VarLong `totalTicks`;
2. big-endian float `partialTick`;
3. big-endian float `rate`.

## Client behavior

`ClientClockManager.handleUpdates(gameTime, updates)` first advances existing
clocks by the difference from the previous packet `gameTime`, then replaces the
provided clocks with their authoritative `totalTicks`, `partialTick`, and
`rate`. Between updates each clock advances by `gameTimeDelta * rate`.

Therefore Solaris must keep packet `game_time` monotonic and send the mutable
overworld day clock separately. Reusing the mutable time-of-day value as
`game_time` makes `/time set` produce an invalid client clock delta. Sending an
empty update map leaves the client's overworld clock at its default value, which
matches the owner-observed sun fixed at noon.

## Solaris boundary

For the current single-overworld runtime Solaris sends:

- monotonic simulation tick as packet `game_time`;
- server-owned persisted world time as clock id `0` `total_ticks`;
- `partial_tick = 0.0`;
- `rate = 1.0` while the daylight cycle is enabled and `0.0` while frozen;
- no End clock update until Solaris owns an active End dimension clock.

Focused protocol and TCP tests verify the complete map payload. A real 26.1.2
client gate must additionally observe `overworld_clock_time` advancing; packet
unit tests alone do not prove the rendered sun moved.

## Daylight-cycle policy

Date: 2026-07-30

The operator gamerule `/gamerule do_daylight_cycle [true|false]` now owns the
daylight-cycle policy. It defaults to `true`, persists in the world metadata,
and is restored before players join. Changing it publishes the current
overworld clock immediately with the corresponding wire rate, so connected
clients start or stop interpolation without waiting for a separate time change.

When disabled, the persisted overworld clock stops advancing while the
monotonic simulation clock continues. Entity lifecycles, scheduled work, and
other tick-based systems therefore keep running. `/time set` remains
authoritative in either policy state and publishes the requested time at the
current rate.

Focused tests cover the running and frozen packet payloads, monotonic simulation
ticks while frozen, parser/suggestion exposure, legacy metadata defaulting,
metadata round-trip, and save/rebind restoration. The real-client observations
below remain the graphical proof for the default enabled policy; this
checkpoint did not repeat that already-closed gate.

## Graphical client gate

Date: 2026-07-30

This was an agent-run gate against tree
`1e2fcc62101c65a5d06dcfa7431dd962f0f62022`, not an owner-played subjective
gate. The host exposed a working X display at `DISPLAY=:1`. The repository's
fixed launcher started the real Minecraft `26.1.2` client with NeoForge
`26.1.2.76`, username `ClockGate`, and an isolated game directory. The client
MCP reported pushed `in_play=true` before observations began.

The server used the debug binary and an ignored config copied directly from
`playable.toml`. The only meaningful test-only change was granting `ClockGate`
operator access so the client could issue the required time commands, enter
spectator mode, and hold an unobstructed sky view at `(0, 180, 0)`. This is
clock-rendering evidence only; it is not no-operator survival evidence.

### Observations

| Point | `game_time` | Overworld clock | Client-visible result |
| --- | ---: | ---: | --- |
| `/time set night` | 4,533 | 13,019 | Chat confirmed `Set time to 13000`; the screenshot shows the moon and stars. |
| `/time set day`; cycle start | 4,554 | 1,020 | Chat confirmed `Set time to 1000`; the screenshot shows the morning sun. |
| First advancing interval | 5,320 | 1,786 | Both clocks advanced 766 ticks, exceeding the required 600. The rendered sun moved from its start position. |
| Day sky | 10,676 | 7,142 | Both clocks advanced 6,122 ticks; the sun rendered high overhead. |
| Sunset | 16,796 | 13,262 | Both clocks advanced 12,242 ticks; the sun rendered at the red western horizon. |
| Natural late night | 26,762 | 23,228 | The open-screen recovery capture shows a dark star field before dawn. |
| Dawn, day 1 | 27,786 | 24,252 | Both clocks advanced 23,232 ticks; the rising sun rendered in the east. |
| Complete cycle, day 1 | 28,557 | 25,023 | Both clocks advanced 24,003 ticks; the rendered view returned to daytime. |

The scheduled 18,000-tick screenshot was discarded because Minecraft's
`PauseScreen` was open. Its structured time observation remained valid, but it
is not counted as visual evidence. The gate closed from the valid command-night,
day, sunset, recovered natural-night, dawn, and complete-cycle captures.

Before the restart, the client observed overworld time `25,260`. The execution
controller recorded the first server process handling `SIGINT`, completing its
final world-metadata save, and exiting with status 0; the structured controller
witness is preserved with the artifacts below. The restarted server loaded
persisted `world_time=25357`; after the same real client returned to pushed
Play state it observed overworld time `25,540`, day `1`, with no screen open.
The restart screenshot shows the expected early-day sun instead of a reset or
frozen noon.

### Local artifacts

All runtime artifacts remain ignored under `.analysis/`; no Mojang bytes or
screenshots enter Git.

- Structured cycle observations:
  `.analysis/clock-gate-20260730/observations.jsonl`
  (`SHA-256 be4188d796b79dcfbc2970756f1f44c677445b9b268144ff5ed21ebcf5e3ea85`).
- Restart observation:
  `.analysis/clock-gate-20260730/restart-observation.json`.
- First-process clean-shutdown controller witness:
  `.analysis/clock-gate-20260730/first-server-clean-shutdown.json`
  (`SHA-256 95ab16694efc249a071b288bdcc404ef3f1c415b7b907114309e69a1ee1087c2`).
- Discarded/recovered screen classification:
  `.analysis/clock-gate-20260730/night-recovery-observation.json`.
- Valid screenshots:
  `.analysis/clock-gate-20260730/screenshots/{command-night-east,cycle-start-east,cycle-600-east,cycle-noon-up,cycle-sunset-west,cycle-midnight-up-recovered,cycle-dawn-east,cycle-complete-east,restart-east}.png`.
- Client log:
  `.analysis/minecraft-clock-gate-20260730/logs/latest.log`.
- Restart server log:
  `.analysis/clock-gate-20260730/server-restart.log`.

This closes the public-alpha graphical clock gate on the recorded tree. It
does not close the separate owner-observed movement, combat, water, terrain, or
overall-session feel gate.
