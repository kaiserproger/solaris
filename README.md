# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** M2 code-complete on `dev/M2-world-representation`. The
world model — block registry, palette-based chunk sections,
Anvil-compatible `.mca` codec, lazy `WorldStorage` façade — is in
place and round-trips real vanilla 26.1.2 chunks end-to-end. The
network layer (M1) still does not stream chunks; M3 will wire
`WorldStorage` into the Play state so connecting clients actually
see a world.

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
