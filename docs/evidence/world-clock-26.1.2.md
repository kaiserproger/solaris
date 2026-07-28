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
- `rate = 1.0`;
- no End clock update until Solaris owns an active End dimension clock.

Focused protocol and TCP tests verify the complete map payload. A real 26.1.2
client gate must additionally observe `overworld_clock_time` advancing; packet
unit tests alone do not prove the rendered sun moved.
