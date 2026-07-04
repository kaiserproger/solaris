# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** stabilization-alpha/private vanilla-near base, not a release-ready
vanilla replacement. The M100 frozen denominator currently has 46 in-scope rows,
0 countable `ready` rows, and 0.00% conservative coverage under the runtime-test
plus separate vanilla oracle/real-client evidence rule. A 2026-06-13 static
review found no cargo, client, or profiler run evidence; see
[`docs/REPLACEMENT_READINESS.md`](docs/REPLACEMENT_READINESS.md),
[`docs/VALIDATION_LEDGER.md`](docs/VALIDATION_LEDGER.md), and
[`docs/VALIDATION_COVERAGE_AUDIT.md`](docs/VALIDATION_COVERAGE_AUDIT.md) before
making compatibility or readiness claims.

## Build

```sh
cargo build
```

Use debug builds for development; release builds are reserved for CI/owner-run
checks.

## Runtime Data

Solaris ships its required registry/data baseline as repo-owned JSON assets
embedded into the server binary. No external vanilla data sidecar is required
to start the server. If `data.vanilla_data_dir` points at a local extracted
vanilla sidecar, that sidecar is treated as authoritative: registries, tags,
`reports/block_light.json`, and supported simple loot must be present. Generate
it with `tools/extract-vanilla-data.sh`; without `data.vanilla_data_dir`, Solaris
uses embedded repo-owned fallback data.

## Run

```sh
# Just validate the config:
cargo run --bin mc-server -- --check --config example.toml

# Actually serve:
cargo run --bin mc-server -- --config example.toml
```

The `--check` JSON includes `operator_warnings`. Treat non-empty warnings as
deployment readiness blockers; for example, public binds with offline-mode auth
or `allow_local_dev_operators` are not production-safe, and a missing
`[data].world_dir` means chunk streaming/persistence is not ready.

Then connect a vanilla 26.1.2 PrismLauncher client to the configured address.

## Test

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
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
