# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** M10 complete on `dev/M10-light-perf-geometry`, awaiting
owner review, merge, and `m10` tag. A vanilla 26.1.2 client connecting to Solaris walks the
full Handshake → Login → Configuration → Play sequence, receives
the streamed spawn-area chunks lit by `mc_world::light`'s BFS
engine (M4), can **break and place blocks** (M5), sees those
edits **persist** across restarts (M6), walks in any direction
on infinite hill-noise terrain (M7), and the spawn burst starts
at the player's chunk and fills outwards in chebyshev rings (M8).
Edits now also drive an **incremental relight** (M9): a single
break/place runs a bounded BFS over a 3×3-chunk window instead
of recomputing the full 5-chunk neighbourhood, and emits only
the `LightUpdate` packets for chunks whose arrays actually
changed — wire-byte-identical to a fresh full recompute. The
BFS uses Starlight's early-skip shortcut (skip the opacity
lookup when the neighbour is already at the best propagatable
level), cutting block-state reads roughly 5–6× in dense regions.
M10 adds a light-engine bench harness, packed window-local queues
for incremental BFS, lazy per-section nibble storage for cached
light, and a light-table-driven highest-opaque heightmap used for
adaptive spawn Y and sky-column reseed guarding. Single plains
biome, no caves / ores / structures; Set Compression, LZ4 chunk
read, light persistence, and survival validation remain M11+
polish items.

## Build

```sh
cargo build --release
```

## Prerequisite: vanilla data sidecar

Solaris reads its registry data from a local sidecar populated from the
official Mojang server jar. Drop a `server.jar` (any 26.1.x release)
into `.analysis/` and run:

```sh
tools/extract-vanilla-data.sh
```

This populates `data/vanilla/` (gitignored — Mojang bytes never enter
this repo). See [ADR 0001](docs/decisions/0001-vanilla-data-as-runtime-input.md)
and [`data/vanilla/README.md`](data/vanilla/README.md) for the why.

The server fails fast with a clear error pointing at this script if
the sidecar is missing.

## Run

```sh
# Just validate the config:
cargo run --bin mc-server -- --check --config example.toml

# Actually serve:
cargo run --bin mc-server -- --config example.toml
```

## Test

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## Layout

```
crates/
├── mc-protocol/     wire protocol: packets, codec, encryption
├── mc-nbt/          NBT helpers
├── mc-world/        block states, chunk format, world storage
├── mc-worldgen/     generation pipeline, biomes, structures
├── mc-physics/      block physics, collisions, fluids
├── mc-entity/       entity system, AI, pathfinding
├── mc-net/          connection management, session lifecycle
├── mc-data/         data pack loader, registries, recipes
├── mc-extension/    custom protocol extension (for the client mod)
├── mc-script/       plugin/script API (Lua/WASM)
├── mc-server/       main binary
└── mc-test-harness/ diff testing infrastructure
```

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
