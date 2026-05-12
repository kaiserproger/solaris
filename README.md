# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** M4 complete on `dev/M4-lighting`, awaiting `m4` tag. A
vanilla 26.1.2 client connecting to Solaris walks Handshake → Login →
Configuration → Play, receives the full vanilla tag set via
`Update Tags`, renders the spawn floor from `.analysis/test-world/`
streamed straight out of `WorldStorage`, and now also receives
real sky / block light: every `LevelChunkWithLight` packet carries
nibble layers from `mc_world::light`'s BFS propagation engine
(seeded by an extracted per-block-state emission / opacity /
sky-propagation table from `tools/extract-block-light.sh`). The
manual gate has been re-driven through with a PrismLauncher 26.1.2
client: terrain loads, spawn drops the player on grass, the world
renders lit through the view-distance window.

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
