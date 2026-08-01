# Solaris

A custom Minecraft Java Edition 26.1-compatible server engine, written in Rust.

Solaris is an authoritative server implementing the vanilla 26.1 Java protocol
plus a custom protocol extension consumed by a Fabric/NeoForge client mod. See
[`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) for the full design document.

**Status:** public alpha for Minecraft Java Edition 26.1.2. Solaris is suitable
for testing, development servers, plugin experiments, and bounded multiplayer
sessions. It is **not** a production-safe drop-in replacement for vanilla or an
existing server fleet. Persistence schemas, plugin APIs, and client-extension
contracts may break between alpha releases without migration support.

The public alpha ships Linux x86_64 and AArch64 binaries. Windows and macOS have
no prebuilt artifacts. Validate broader compatibility and replacement-readiness
claims against:
[docs/REPLACEMENT_READINESS.md](docs/REPLACEMENT_READINESS.md),
[docs/VALIDATION_LEDGER.md](docs/VALIDATION_LEDGER.md), and
[docs/VALIDATION_COVERAGE_AUDIT.md](docs/VALIDATION_COVERAGE_AUDIT.md).

### Alpha boundaries

- Exact target: unmodified Minecraft Java Edition `26.1.2`.
- Ordinary survival, persistence, multiplayer, mobs, merchant trading, and Lua
  plugins are implemented far enough for public testing, not full vanilla parity.
- Some species-specific mob attacks, village population/defence, zombie-villager
  curing, Hero of the Village pricing, rare redstone/vehicle behavior, and broad
  performance envelopes remain incomplete.
- Existing Solaris worlds and plugins may require deletion or manual adaptation
  after an alpha update. Backward compatibility is not promised before `1.0`.
- Report reproducible bugs with the server version, config, client version, logs,
  and the smallest reproduction. The remaining release path is tracked in
  [`docs/PUBLIC_ALPHA_PLAN.md`](docs/PUBLIC_ALPHA_PLAN.md).

## Build

```sh
cargo build
```

Use debug builds for development; release builds are reserved for CI/owner-run
checks.

## Install the public alpha on Linux

Tagged releases publish SHA-256-verified server archives for Linux x86_64 and
AArch64. Prereleases are deliberately not resolved through GitHub's `latest`
alias, so pin the alpha tag explicitly:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/main/install.sh | \
  SOLARIS_VERSION="v0.0.1-alpha.1" bash
```

The installer writes `solaris` to `$HOME/.local/bin` for a regular user and to
`/usr/local/bin` when run as root. Override the destination while keeping the
release pinned:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/main/install.sh | \
  SOLARIS_INSTALL_DIR="$HOME/bin" SOLARIS_VERSION="v0.0.1-alpha.1" bash
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
# Keep permissive for local iteration. Production should set strict = true and
# list the exact external and bundled ids in expected.
strict = false
expected = []

[data]
world_dir = "world"
seed = 0

[simulation]
random_tick_speed = 5
save_interval_ticks = 1200
friendly_spawn_interval_ticks = 400
hostile_spawn_interval_ticks = 20

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

The `--check` JSON includes `operator_warnings`. Review every warning before
serving. A fresh `world_dir_missing_on_disk` warning is expected when creating a
new world and `serve` will create that directory. Public binds with offline-mode
auth or `allow_local_dev_operators` are unsafe, while an unusable world path or
stale `data.vanilla_data_dir` is a real persistence/data readiness blocker.

Then connect a vanilla 26.1.2 PrismLauncher client to the configured address.

Server-side plugins run as sandboxed strict Luau. External packages are loaded
from `[plugins].directory`; server-embedded examples are enabled explicitly with
`[plugins].bundled`. See [`docs/PLUGINS.md`](docs/PLUGINS.md) for the package
format, bundled ids, type-checking rules, and API.

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
