# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** stabilization-alpha/private vanilla-near baseline in active
development. Solaris is not release-ready. Validate compatibility and readiness
against:
[docs/REPLACEMENT_READINESS.md](docs/REPLACEMENT_READINESS.md),
[docs/VALIDATION_LEDGER.md](docs/VALIDATION_LEDGER.md),
[docs/VALIDATION_COVERAGE_AUDIT.md](docs/VALIDATION_COVERAGE_AUDIT.md)
before making compatibility or readiness claims.

## Build

```sh
cargo build
```

Use debug builds for development; release builds are reserved for CI/owner-run
checks.

## Install a tagged Linux build

Tagged releases publish SHA-256-verified server archives for Linux x86_64 and
AArch64. Solaris is still stabilization-alpha, so treat installed binaries as
test builds rather than replacement-ready production releases.

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/main/install.sh | bash
```

The installer writes `solaris` to `$HOME/.local/bin` for a regular user and to
`/usr/local/bin` when run as root. Override the destination or pin a release:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/main/install.sh | \
  SOLARIS_INSTALL_DIR="$HOME/bin" SOLARIS_VERSION="v0.1.0" bash
```

The script downloads the matching GitHub release archive, verifies its published
SHA-256 checksum before extraction, rejects unsafe archive paths, and only then
replaces the destination binary.

## Runtime Data

Solaris ships its required registry/data baseline as repo-owned JSON assets
embedded into the server binary. No external vanilla data sidecar is required
to start the server. If `data.vanilla_data_dir` points at a local extracted
vanilla sidecar, that sidecar is treated as authoritative: registries, tags,
`reports/block_light.json`, and supported simple loot must be present. Generate
it with `tools/extract-vanilla-data.sh`. Both `--check` and `serve` reject a
missing/unusable sidecar root or a `version.json` that is missing, invalid, or
targets a different release id, world version, or protocol. Without
`data.vanilla_data_dir`, Solaris uses embedded repo-owned fallback data.

## Run

```sh
# Optional: copy a minimal starter config.
# The file uses the same schema as example.toml.
cat > server-run.toml <<'EOF'
[server]
name = "solaris-local"
motd = "Solaris local server"
view_distance = 8

[network]
bind_address = "127.0.0.1"
port = 25565

[auth]
online_mode = false
prevent_proxy_connections = false
whitelist_enabled = false
whitelist = []
banned_players = []

[admin]
operators = []
allow_local_dev_operators = true

[plugins]
directory = "plugins"

[data]
world_dir = "world"
seed = 0

[simulation]
random_tick_speed = 5
save_interval_ticks = 1200
spawn_monsters = true

[chunk_pipeline]
chunk_send_rate = 8
chunk_load_rate = 16
chunk_generate_rate = 16
chunk_prepare_budget_ms = 0
chunk_prepare_batch_size = 8
chunk_result_queue_size = 64
region_cache_size = 9

[autoscale]
enabled = true
profile = "balanced"
EOF

# Just validate the config:
cargo run --release --bin mc-server -- --check --config server-run.toml

# Actually serve:
cargo run --release --bin mc-server -- --config server-run.toml
```

The `--check` JSON includes `operator_warnings`. Treat non-empty warnings as
deployment readiness blockers; for example, public binds with offline-mode auth
or `allow_local_dev_operators` are not production-safe, and a missing or
unusable `[data].world_dir` or stale `data.vanilla_data_dir` means chunk
streaming, persistence, or data/protocol readiness is not ready.

Then connect a vanilla 26.1.2 PrismLauncher client to the configured address.

Server-side Lua plugins are loaded from the configured `[plugins].directory`.
See [`docs/PLUGINS.md`](docs/PLUGINS.md) for the package format and API.

For agent-driven real-client checks, the reusable NeoForge client mod embeds a
loopback MCP server with structured world observation and input controls. See
[`client-mod/solaris-client-agent/README.md`](client-mod/solaris-client-agent/README.md)
and start it with `tools/run-minecraft-client-mcp.sh`.

## Test

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Current performance evidence

The latest bounded debug/release matrix is recorded in
[`docs/performance/2026-07-27-benchmark-matrix.md`](docs/performance/2026-07-27-benchmark-matrix.md).
The focused 20-client VD8 gate passes in both builds on the calibration host, but
the frozen low/balanced/high duration and cgroup envelopes are still incomplete.
The current O3 explosion-authority benchmark remains over its 50 ms p99 budget;
see the matrix before making performance or readiness claims.

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
├── mc-script/       plugin API and sandboxed Lua host
├── mc-server/       main binary
└── mc-test-harness/ diff testing infrastructure
```

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
