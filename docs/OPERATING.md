# Operating Solaris

This guide covers the current alpha server configuration. Start from
[`../example.toml`](../example.toml), keep it under versioned operator control if
appropriate, and validate every change before serving:

```sh
cargo run --bin mc-server -- --check --config server.toml
cargo run --bin mc-server -- --config server.toml
```

For an installed release, replace `cargo run --bin mc-server --` with `solaris`.
The default config path is `config.toml`; `--config` selects another TOML file.

## Network bind and port

```toml
[network]
bind_address = "127.0.0.1"
port = 25565
```

`bind_address` is an IP address, not a hostname. Common choices are:

| Intent | Example | Notes |
| --- | --- | --- |
| Same machine only | `127.0.0.1` | Safest local development default. |
| One LAN interface | `192.168.1.20` | Other LAN clients use `192.168.1.20:25565`. |
| Every IPv4 interface | `0.0.0.0` | Requires deliberate auth and firewall policy. |

`port` is the TCP port Solaris listens on. It has long been configurable; choose
an unused port and give players `host:port` when it is not `25565`. Opening a
firewall or router port does not change Solaris configuration: allow/forward the
same TCP port separately in the operating system and network perimeter.

Before exposing Solaris beyond a trusted private network:

```toml
[auth]
online_mode = true
prevent_proxy_connections = true # include the connecting IP in hasJoined
```

Solaris rejects a public bind when `online_mode = false`. Offline mode derives
identities without Mojang session authentication, so names can be impersonated;
it is only appropriate for isolated loopback/private field tests with a trusted
network. Run `--check` and treat public-bind warnings as security failures, not
noise.

## Authentication and access control

The inline policy is sufficient for a small test server:

```toml
[auth]
online_mode = true
prevent_proxy_connections = true
whitelist_enabled = true
whitelist = ["PlayerName"]
banned_players = []

[admin]
operators = ["OperatorName"]
allow_local_dev_operators = false
```

Names or UUIDs may instead come from vanilla-style JSON profile files:

```toml
[auth]
whitelist_file = "whitelist.json"
banned_players_file = "banned-players.json"

[admin]
operators_file = "ops.json"
```

Example `ops.json`:

```json
[
  { "uuid": "00000000-0000-0000-0000-000000000000", "name": "OperatorName", "level": 4 }
]
```

Relative access-control file paths are resolved from the directory containing
the selected config file. Solaris consumes `name` and/or `uuid`; extra
vanilla fields such as `level` are accepted but do not create multiple Solaris
permission levels. Inline and file entries are merged at startup.

Current operators receive the restricted in-game command tree, including the
implemented administration/gameplay roots such as `status`, `save-all`, `stop`,
`gamemode`, `gamerule`, `give`, `kill`, `summon`, `time`, `tp`, and `weather`.
Manage persisted operators explicitly from the server CLI:

```sh
mc-server --config server.toml operator list
mc-server --config server.toml operator add OperatorName
mc-server --config server.toml operator remove OperatorName
```

The same commands work through Cargo during development:

```sh
cargo run --quiet --bin mc-server -- --config server.toml operator add OperatorName
```

The identity argument is either a 3–16 character Minecraft username (ASCII
letters, digits, or `_`) or a UUID. Names are normalized to lowercase and
output is deterministic. Repeating `add` is a no-op; `remove` is also safe when
the identity is absent. Unknown fields in existing operator profiles (for
example `level`) are retained.

`operators_file` is resolved relative to the selected config. If it is absent,
the CLI uses the deterministic `ops.json` path beside the config without
rewriting TOML; server startup auto-loads that file when it exists. The first
`operator add` creates the file. An explicitly configured but missing file is
an actionable error for `list` and `remove`; run `operator add` to initialize
it. Malformed JSON, names, UUIDs, and oversized files fail closed. Invalid
`add` identities have no file side effect. Operator changes are persisted for
the next server start; they do not hot-reload a running process. `SIGHUP` is a
strict-plugin reload only.

`allow_local_dev_operators = true` is a convenience for throwaway loopback
development when no identities are configured. Leave it `false` for normal
operation; a public bind with it enabled is rejected.

## World contract and fresh worlds

```toml
[data]
world_dir = "world"
seed = 0
worldgen_mode = "tellus_like"
```

`world_dir` is required. If it does not exist, `--check` reports
`world_dir_missing_on_disk` and serve creates a fresh world. If it exists but is
unusable, check/startup fails closed.

Solaris writes `solaris/world.json` inside the world. The contract prevents one
directory from mixing chunks made with incompatible generation revision, seed,
worldgen mode, chunk geometry, ore profile, or settlement plan. Changing one of
those values requires an empty/new `world_dir`; do not delete only the contract
file or combine region files from two contracts. Back up the complete directory
before upgrading an alpha.

Plugin worldgen declarations are startup-only and become part of this contract.
An unversioned vanilla Anvil import cannot use Solaris plugin worldgen to fill
missing chunks. See [Plugin worldgen](PLUGINS.md#package-and-manifest) for the
current bounded declarations.

Solaris embeds its required registry/data baseline. Configure
`vanilla_data_dir` only when deliberately using a locally extracted 26.1.2
sidecar:

```toml
[data]
vanilla_data_dir = "data/vanilla"
```

The sidecar becomes authoritative and must contain matching version metadata,
registries, tags, block-light report, and supported recipes/loot. Generate it
with `tools/extract-vanilla-data.sh`; remove the setting to return to embedded
data.

## Natural population

Natural spawning is controlled independently from random block ticks:

```toml
[simulation]
friendly_spawn_interval_ticks = 400
hostile_spawn_interval_ticks = 20
friendly_spawn_cap = 32
aquatic_spawn_cap = 20
hostile_spawn_cap = 70
friendly_spawn_chunk_budget = 48
hostile_spawn_chunk_budget = 4
```

The three caps are **global server caps for natural mobs in each category**, not per-player multipliers. Multiple players expand the union of eligible simulation chunks, but overlapping players do not duplicate a chunk or create extra cap capacity. A cap of `0` keeps the category at zero natural population. An interval of `0` disables attempts for that category entirely.

A chunk budget is the maximum number of active chunks sampled on one due attempt. It changes refill speed and spatial coverage only: biome/species selection, loaded-simulation-chunk residency, support/fluid checks, hostile light rules, player-distance exclusion and entity collision remain mandatory. Friendly and aquatic mobs share the friendly attempt cadence; their caps remain separate.

Increasing simulation distance can expose more eligible chunks, but it does not raise the global caps. Despawn and movement can free capacity; later due attempts refill toward the configured ceilings. `--check` prints the normalized simulation values, and `periodic natural spawn metrics` logs cumulative sampled/committed/rejection counters for tuning.

The alpha-3 starter profile uses friendly budget `48`: on the measured seed `712816` it produced materially more visible daytime fauna than the former sparse baseline while retaining bounded attempts. Treat population tuning as workload tuning and measure tick latency/RSS before raising caps or budgets further.

## Autoscale

```toml
[autoscale]
enabled = true
profile = "balanced" # low_end, balanced, or high_end
```

Autoscale derives worker capacity from process CPU availability and adapts
bounded view-distance and chunk send/load/generation budgets using runtime p95
pressure. It is not an external cluster autoscaler and does not add worker
processes. The starter `balanced` profile is the recommended baseline.

Optional bounds include `min_view_distance`, `max_view_distance`,
`target_tick_ms`, `target_first_chunk_ms`, `scale_down_after_ticks`, and
`scale_up_after_ticks`. The `[chunk_pipeline]` values provide the configured
initial rates. Use `--check` to inspect `effective_chunk_pipeline` and
`effective_autoscale` after normalization instead of assuming the raw TOML is
the final policy.

## Plugins

External plugin packages are child directories of the configured root:

```toml
[plugins]
directory = "plugins"
bundled = []
strict = false
expected = []
```

For local authoring, permissive discovery may skip an ordinary broken plugin.
For a controlled deployment, enable strict mode and enumerate the complete
external-plus-bundled set:

```toml
[plugins]
directory = "plugins"
bundled = ["online-roster"]
strict = true
expected = ["online-roster", "my-plugin"]
```

Strict mode rejects stray entries, malformed packages, startup failures, and a
missing or unexpected id. See [`PLUGINS.md`](PLUGINS.md) for manifests,
permissions, APIs, reload boundaries, and deployment reporting. A plugin with
client bundles requires players to follow
[`SOLARIS_LOADER.md`](SOLARIS_LOADER.md).

## Reading `--check`

A successful check prints JSON and does not open the network listener. Important
fields are:

- `network.bind_address` and `network.port`: the requested listener endpoint;
- `operator_warnings`: security, access-control, world, and sidecar findings;
- `effective_chunk_pipeline`: automatically derived worker capacity;
- `effective_autoscale`: normalized mode, limits, and pressure policy;
- `discovered_plugins`: each plugin's `server_only`/`server_and_client`
  deployment, Loader platforms/permissions, and exact bundle artifacts.

Typical inspection with `jq`:

```sh
cargo run --quiet --bin mc-server -- --check --config server.toml |
  jq '{network, operator_warnings, effective_autoscale, discovered_plugins}'
```

A fresh-world warning is expected only when intentionally creating that world.
Do not start after a warning you do not understand.

## Dashboard

An optional first-party read-only dashboard is built in and disabled by
default:

```toml
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = 8080
```

Open `http://127.0.0.1:8080/` in a browser while the server runs. The page
polls `GET /stats` every two seconds and shows uptime/version, players, TPS
and tick latency percentiles (total plus per-stage), memory, autoscale limits
and decisions, chunk load/generate/stream counters, entity counts by
category, natural-spawn metrics, the last save report, network pressure,
loaded plugins, and the most recent warnings. All values come from existing
lock-free telemetry; polling never blocks the simulation.

The dashboard is unauthenticated. It only binds loopback unless
`allow_remote = true` explicitly acknowledges the exposure; for remote access
prefer an authenticated reverse proxy on a trusted network. `--check`
validates the endpoint and the loopback rule.

## Current operator boundaries

The alpha has no remote operator API or interactive server console. Operator
changes use the local `operator` CLI workflow above and take effect on the
next server start; the optional dashboard is read-only. Do not expose an
unrelated service expecting Solaris to secure it. Runtime diagnostics are
logs, in-game operator commands, the optional dashboard, and the validated
effective configuration.
