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

Both authentication modes permit public binds. Offline mode derives identities
without Mojang authentication, so player and operator names can be impersonated.
An offline public bind emits a warning rather than preventing startup. Restrict
access to trusted players or an authenticated proxy. `allow_local_dev_operators`
remains forbidden on public binds. Authentication does not change the listener:
`127.0.0.1` is local-only even with `online_mode = true`; use `0.0.0.0` or the
appropriate interface for remote clients.

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
Removal revokes the whole profile when either its name or UUID matches, including
overlapping duplicate profiles that share an identity. It does not leave the
other identity authorized. Explicit identities in `[admin].operators` are a
separate source and must be removed there if they also grant the same access.

File mutations serialize the complete read–modify–write through a persistent
`ops.json.lock` sidecar (or `<configured-file>.lock`). Do not remove that sidecar
while managers run. The new JSON is written and synced in the same directory,
then atomically replaced and the directory synced. Cooperating CLI processes do
not lose each other's updates; external editors do not participate in this lock.
An error after replacement during directory sync has an uncertain durability
outcome: inspect the file rather than assuming the old version remains.

Management refuses symlink, multiply linked, and read-only mutation targets.
Existing Unix ownership and permission bits are preserved; extended ACLs and
xattrs are not explicitly copied. New files use private tempfile permissions.

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

Worldgen revision 20 makes rivers wider and deeper while retaining seeded width
variation, shallow banks and varying channel beds. It is not compatible with
revision-19 generated worlds: select a fresh `world_dir` to use the new terrain.
Existing chunks are not retroactively reshaped, and editing `world.json` to bypass
the revision check would mix incompatible terrain.

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
friendly_spawn_chunk_budget = 48
hostile_spawn_chunk_budget = 4
```

Natural admission is bounded globally: 10 ground animals, 20 aquatic mobs, and
70 hostiles. At most five of the aquatic population may be water creatures such
as squid; fish can use the remaining capacity. Ground admission permits at most
two animals in a chunk, spreading new animals across several chunks. These are
native limits, not configuration keys. Existing animals are not deleted.

Chunk budgets limit work per attempt, not the population. Spawning considers
the union of loaded client chunks within 128 blocks, even when the AI simulation
distance is smaller. Overlapping players do not multiply capacity. Support,
fluid, light, player-distance and collision checks remain in effect. An interval
of zero disables that category's attempts. Ground wander pauses are short;
injured animals preserve the initial impulse, then run for up to five seconds.

Cod, salmon, pufferfish and tropical fish use momentum-preserving, smoothed swim
steering and fish-specific water travel. Squid and glow squid use separate
pulse/coast movement rather than the fish controller. Their local navigation
checks known water and turns away from unsafe next positions; fresh swimmers do
not require a previous solid collision to receive terrain samples.
These changes use 26.1.2 bytecode evidence, not complete AI parity: schooling,
full vanilla pathfinding, squid flee/animation synchronization and exact fluid
height semantics remain outside this implementation. Other aquatic wanderers
retain their existing species behavior with direction continuity and corrected
yaw; they are not forced through fish navigation.

After a successful save, clean chunks outside retained client views are trimmed
to a 64-chunk warm cache. Loaded and dirty chunks are never discarded by this
trim. Old natural populations are preserved, but cannot keep growing past caps.

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
`target_tick_ms`, `target_first_chunk_ms`, `scale_down_after_seconds`, and
`scale_up_after_seconds`. The two durations default to 60 and normalize to at
least 60 seconds. Scaling requires **strictly more** than that duration of
continuous pressure or stable headroom, measured by a monotonic clock, not by
counting ticks or queue notifications. A break in the condition resets its
window; each adjustment starts a fresh window. View distance, throughput and
eligible deferred-work budgets change one unit per step, not by halving/doubling.
Recovery requires at least 20% tick headroom. Immediate bounded admission,
memory-pressure waiting and explicit shutdown/drain do not wait for autoscale.

Without explicit view-bound overrides, the upper bound follows
`[server].view_distance`: configuring 16 no longer starts at a profile cap of
10 before any overload. The `[chunk_pipeline]` values provide the initial
rates. Use `--check` to inspect `effective_chunk_pipeline` and
`effective_autoscale`. The old `scale_*_after_ticks` keys are removed; replace
them with the seconds-based keys rather than carrying short observation counts
into a new config.

## Plugins

External plugin packages are child directories of the configured root:

```toml
[plugins]
directory = "plugins"
strict = false
expected = []
```

For local authoring, permissive discovery may skip an ordinary broken plugin.
For a controlled deployment, enable strict mode and enumerate the complete
deployed set. Copy selected packages from `../solaris-default-plugins` first:

```toml
[plugins]
directory = "plugins"
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

The local console provides `help`, `status`, `profile`, `list`, world controls,
`save-all` and `stop`. `--no-console` retains plain stdin commands without the TUI.
Runtime logs are written to `logs/latest.log` and `logs/debug.log`, not to the
console. Files start fresh on launch; `RUST_LOG` filters the debug file.
`profile` atomically replaces `logs/profile.json` with a fresh diagnostic capture
on a blocking worker, not on the simulation tick. Run it once before reproducing
the workload and again afterwards; CPU/allocation deltas span those captures
(the first interval starts when the provider is created). Ordinary dashboard
polling does not perform this expensive capture. The console confirms RSS,
requested live Rust bytes, average CPU cores and capture duration.

- `process`: actual process RSS, anonymous/file/shared residency, Linux mapping
  totals and I/O. The older `memory.used_mb` remains the autoscaler pressure
  domain, which can be cgroup usage rather than process RSS.
- `allocations`: requested Rust live bytes and allocation/deallocation churn,
  measured by atomic counters around the existing system allocator.
  `component_reconciliation.owners` sorts observed owner estimates: block
  registry, published chunks, reusable lighting scratch, prepared frames,
  session views and entity tables. The unclassified remainder stays explicit.
- `resources`: block registry definitions/states/lookup keys; chunk block
  palettes, bit widths, biomes, heightmaps, light arrays, preserved NBT and other
  state; dirty/cache/prepared/queued populations and reusable worker capacities.
  Shared block/light payloads are deduplicated within the published chunk set;
  prepared-light reachable bytes are non-additive because arrays can be shared.
- `cpu`: process CPU and exclusive thread CPU attributed to chunk disk decode,
  generation, block encoding, heightmaps, lighting, light encoding, framing/
  compression, other preparation, simulation, saving and network execution.
  Async scopes measure each poll, excluding suspension. Inclusive execution wall
  time and `resources.lock_pressure_wall_time` are not CPU or additive totals.
  Per-thread CPU and runnable-wait deltas are in `process`; exited threads can
  be absent there but remain included in process CPU.

This is not a flamegraph or allocation-stack trace. Capacity estimates and
allocator counters are sampled sequentially, not transactionally; completed CPU
scopes can straddle interval boundaries. Native allocations bypassing Rust,
allocator page retention, older external snapshots and other unmeasured owners
are not falsely assigned to chunks. RSS minus requested Rust bytes is **not**
an exact retained/free-page measurement. Unsupported/unavailable counters are
null or explicitly unavailable. Operator-file changes still take effect on restart.
RSS can remain high after chunk owners release their allocations: the system
allocator may keep free pages for reuse. Ticket counts, resident chunk counts,
live allocations and RSS are different measurements. Region caching retains
small indexed readers and decodes requested chunks on demand rather than keeping
all decompressed region NBT. Neither this cache change nor a low tick percentile
proves a whole-server RAM target. Compare vanilla only with matching view and
simulation distances, workload, save state and memory metric.
For a chunk-disappearance report, reproduce with the intended view/simulation
distances and type exactly `blink` in ordinary chat (case-insensitive). It remains
normal chat and also records a `chunk_blink_marker` event with player/session,
pose and tick. Copy both logs and a newly requested `profile.json` **before
restarting**; an old profile is not a snapshot of the new log session.

The DEBUG target `solaris::chunk_visibility` records successful chunk/radius/
unload writes with session, generation and coordinates. `chunk_radius_sent`
includes previous/new radius and the last applied autoscale decision;
`chunk_unload_sent` distinguishes movement, client settings and runtime control.
`chunk_sent`, `chunk_send_invalidated` and `chunk_view_replay` distinguish ordinary
delivery, a post-write invalidation/requeue and explicit full-view replay.
Correlate coordinate histories around the marker; counters alone do not prove
duplicates. Successful writes are not proof of client rendering, and the last
autoscale reason is not necessarily the cause of a client-settings change.
Default debug logging includes these events; a restrictive `RUST_LOG` needs
`solaris::chunk_visibility=debug`. These logs contain player identities/positions.
The optional remote dashboard is read-only and unauthenticated.
