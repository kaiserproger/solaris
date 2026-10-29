# Operating Solaris

The current binary is `solaris-v2` (`mc-edge` + `mc-core`). Start from
[`example.toml`](../example.toml). This guide covers the native runtime after
[ADR 0014](decisions/0014-v1-core-retirement.md), not the published historical
v0.0.7 preliminary binary.

```sh
cargo run -p mc-edge --bin solaris-v2 -- --config example.toml
# Direct Cargo-built binary (release packaging may name it `solaris`):
solaris-v2 --config server.toml
```

`--config` defaults to `server.toml`. Unknown sections/keys warn and are ignored;
a warning is not proof that a legacy setting took effect. There is no `--check`,
offline `operator` CLI, TUI/profile command or `check-world` command. Stdin is
enabled unless `--no-console` is supplied.

## Vanilla content cache

Solaris does not distribute Minecraft data. Derive a complete cache from your
licensed server bundles with JDK 25 (`JAVA`/`JAVAC` may select it). A cache
joins two releases: the 26.3 bundle supplies every protocol identity (block
states, items, entities, the per-state light/mining/explosion/map-colour
reports and the exact RegistryData the client receives); the 26.1.2 bundle
supplies the gameplay data pack (worldgen, loot, recipes, advancements, tags,
structures), whose JSON grammar Solaris's loaders read. 26.3 rewrote those
grammars; every name in the 26.1.2 pack exists in 26.3.

```sh
solaris-v2 content import --from /path/to/26.3/server.jar \
  --gameplay-from /path/to/26.1.2/server.jar --cache /path/to/cache
# Alternative: download both pinned artifacts, verifying size and SHA-1.
solaris-v2 content import --version 26.3 --download --cache /path/to/cache
solaris-v2 content verify --cache /path/to/cache
```

Either `--from` or `--gameplay-from` may be omitted; that bundle is then
downloaded. The 26.1.2 bundle's `version.json` is kept as
`gameplay_version.json` and checked at startup.

Import stages, validates and publishes the cache; a failed derivation preserves
the previous cache. Verify is offline and checks the same content used by
startup, not the complete server/plugin deployment. Serving never downloads
implicitly. Missing or incomplete content fails before world/listener startup.

Selection order: `[data].vanilla_data_dir`, `SOLARIS_CONTENT_CACHE`,
`$XDG_DATA_HOME/solaris/content/26.3` (otherwise
`~/.local/share/solaris/content/26.3`), then `data/vanilla` in the working
directory and checkout ancestors. An explicit invalid override fails without
silently choosing another source. `--cache` affects only that offline command.

## Client protocol compatibility

Solaris serves Minecraft Java 26.3, protocol 777. Any client may send a Status
ping: the reply advertises the supported release name and protocol so its
server-list entry is shown as incompatible when appropriate. A Login handshake
on an unsupported protocol receives vanilla's
`multiplayer.disconnect.incompatible` disconnect rather than a silent TCP
close. Sessions of the previous release (26.1.2, protocol 775) are translated
at the edge as described in ADR 0015.

Native block-entity identities follow the 26.3 registry, not the legacy
client's ids: 49 types cover 191 block variants. Beds no longer have block
entities and use vanilla's popped-on-piston-push reaction. Poplar signs,
hanging signs and shelves, plus potent sulfur, use their 26.3 identities.
Other inventory/opaque block entities retain Solaris's conservative piston
refusal rather than risking their contents.

### Retaining 26.1.2 clients after upgrading to 26.3

Protocol 775 clients need a **separate, intact 26.1.2 content cache**. Keep
the deployment's existing cache before importing 26.3; do not overwrite the
old cache. It must include the datapack, `reports/packets.json`,
`reports/registries.json`, `reports/blocks.json` and captured
`reports/registry_network_nbt`, not just a version file or jar.

The edge finds it in this order:

1. `SOLARIS_LEGACY_CONTENT_CACHE` (an explicit path).
2. A sibling release directory: canonical `.../content/26.3` uses
   `.../content/26.1.2`.
3. Otherwise, the canonical path with `-26.1.2` appended: `data/vanilla`
   uses `data/vanilla-26.1.2`.

For a custom layout, set the environment in the server's normal launch
environment (or its service configuration), for example:

```sh
export SOLARIS_LEGACY_CONTENT_CACHE=/srv/solaris/content/26.1.2
```

The server logs `26.1.2 (protocol 775) clients enabled` when loading succeeds.
A missing or unusable old cache logs the reason and disables 775 logins;
they receive the incompatible-version disconnect. Canonical 777 clients
continue to use the 26.3 cache. The rc.1 `content import` command imports
26.3 only: retain the old deployment cache rather than trying to recreate it
with that command. A fresh 26.3-only installation, including native-client
singleplayer, does not need the legacy cache unless it will admit 775 clients.

Legacy clients receive their own registry/tag snapshot. New block states,
items and entity types are shown as named 26.1.2 substitutes (for example,
cinnabar as granite and a sulfur cube as a slime), not their shifted numeric
ids. This is protocol compatibility, not identical rendering or mechanics for
new content; see [ADR 0015](decisions/0015-multiversion-protocol-translation.md).

The 14 new 26.3 item components have no 26.1.2 registry entries. For legacy
viewers their complete, validated payloads are consumed and omitted, including
nested item patches; native inventory custody is unchanged. Shared components
and the following stacks remain translated normally. Unknown or malformed
payloads still fail closed rather than desynchronizing the rest of the packet.

Decorated-pot block entities use 26.3's named `sherds` record: optional
`back`, `left`, `right` and `front` item templates, rather than a list of
item identifiers. Existing native worlds migrate the older list on read;
stored contents and template components are preserved. Chunk and individual
block-entity updates use the native record. Only the 775 presentation boundary
converts decorations back to its ordered identifier list (missing sides render
as brick); this does not rewrite native custody.

## Supported configuration

```toml
[server]
name = "Solaris"
motd = "Solaris native server"
max_players = 20
view_distance = 8
simulation_distance = 6
# Vanilla `accepts-transfers`: admit players another server transferred here.
accepts_transfers = false
# Vanilla `pause-when-empty-seconds`: stop world simulation after this many
# idle seconds; zero or negative never pauses (vanilla accepts -1).
pause_when_empty_seconds = 60
# Vanilla enable-command-block execution gate (creative operator placement only).
enable_command_block = false
# Opt-in per-tick input log for replay from a checkpoint (default off).
input_log = false
# Overworld chunk density on the GPU, batched across chunk workers (build with
# `--features gpu`; without it or a hardware adapter the CPU generates).
worldgen_gpu = false

[network]
bind_address = "127.0.0.1"
port = 25565

[data]
world_dir = "world"
seed = 42
# vanilla_data_dir = "/path/to/cache"

[auth]
online_mode = true

[proxy]
# none (default) or velocity modern forwarding.
mode = "none"
# Contents of Velocity's forwarding.secret; required for velocity mode.
# secret = ""

# Omit to run without component packages.
# [plugins]
# directory = "plugins"

[anti_cheat]
# Paper-like per-Play-packet-id bucket; set either packet value to 0 to disable.
packet_rate_per_second = 500
packet_burst = 1000
# Unsupported, non-descending survival movement samples; 0 disables the check.
max_airtime_ticks = 80

[mob_griefing]
# Finer block-damage switches under the vanilla `mobGriefing` gamerule, which
# remains the master gate. All default true (vanilla 26.1.2 behaviour).
creeper_block_damage = true
ghast_block_damage = true
wither_block_damage = true
villager_farming = true
sheep_grazing = true

[block_log]
# Attributed block/container history behind `/co` (see "Block log and rollback").
enabled = true
# Prune records older than this; 0 keeps them until max_records.
retention_days = 30
# Per-dimension cap; the oldest records go first.
max_records = 200000
```

Relative world, content and plugin paths resolve against the process working
directory, not the TOML directory. Access-list files such as `ops.json` live
beside the selected config. Missing sections use native defaults: loopback TCP
25565, world `world`, seed 0, view distance 8, simulation distance 6, max players
20 and offline authentication. Use explicit operator-controlled values.

`view_distance` is the server's upper bound, not a forced client radius. Each
connection uses the smaller of its requested view distance and this limit
(minimum two chunks, without exceeding the server limit). Changing the client's
render distance during Play updates its cache radius and streams or forgets
chunks without reconnecting. World residency unions those per-player views,
the simulation radius and the existing extra chunk margin; explicit tickets
and dirty-chunk pins can keep additional chunks resident without sending them.

`pause_when_empty_seconds` matches vanilla's `pause-when-empty-seconds`
(1.21.2+): while no player is online the world stops advancing, so mob AI,
random ticks, redstone, weather and the daylight clock cost nothing. Network
traffic, console commands and saves still work, and the entering pause performs
one checkpoint (vanilla's `autoSave`). The first join resets the countdown and
resumes continuous ticking, including while only a single player is present;
the counter counts players in every dimension. Set it to 0 to keep simulating
an empty server.

`input_log = true` writes, per executed tick, the inputs that tick consumed to
`<world_dir>/input-log/<start_tick>-<unix_ms>.jsonl` (one file per start): player
inputs after the drain, synthetic leaves, debug spawns and remote chunk
tickets, as JSON lines (header, one `Tick` line per non-empty tick, `End` on a
clean stop). A dedicated writer thread behind a bounded queue flushes without
fsync, so nothing blocks the tick path; a stalled disk drops records and leaves
a `Gap` marker, and a crash can lose the unflushed tail. The log contains
everything players send: text typed into an *unlocked* session (chat, plugin
commands under other names) is written as sent, so treat the files as
sensitive. Withheld by marker: `login`/`register`/`changepassword` commands, and
all text (chat, command, sign, book edit, rename, dialog click) of a session the
`require_plugin_login` gate still holds locked. Plugin commands, wall-clock
reads and live chunk-load timing are not inputs of this log; a world copy taken
at the log's start checkpoint can be replayed only by the test-only replayer
today. Format and limits: `crates/mc-core/src/input_log.rs`.

`/gamerule mob_griefing false` still disables every mob block change at once.
The `[mob_griefing]` switches narrow that further per behaviour without
touching player-caused damage: a disabled explosion still hurts entities and
still emits its client event, it only stops removing blocks. Vanilla 26.1.2
registers only the master `mobGriefing` gamerule, so these finer switches are
server configuration rather than `/gamerule` names.

The old `[admin]`, `[simulation]`, `[autoscale]`, `[chunk_pipeline]`, worldgen-mode,
per-package grants/expected roster and reload knobs are not supported settings.
Do not carry them forward expecting old behavior. Use the source `EdgeConfig`
and its key list when adding new deployment settings.

## Anti-xray

```toml
[anti_xray]      # both default to true; the End has no ores
overworld = true # coal/iron/copper/gold/redstone/lapis/diamond/emerald ores
nether = true    # nether gold/quartz ore and ancient debris
```

Engine mode 1 (Paper style): an ore whose six neighbours are all opaque solid
blocks is sent to clients as the surrounding rock (`stone`, `deepslate` below
y=0 and for deepslate ores, `netherrack` in the Nether), so a modified client
cannot read it out of chunk data. An ore touching air, fluid, glass or any
other non-opaque block is sent as it is. When a block change exposes a hidden
ore the server sends its real state at once, so legitimate mining is unchanged.
The replacement is made once per chunk in the shared chunk cache and the frame
is shared by every viewer; a neighbouring chunk's change re-encodes the border
chunk for the next viewer. Chunks next to an unloaded chunk are not hidden on
the unloaded side. It hides ores from a client reading chunk data only: ores
exposed by a cave or tunnel stay visible, as in vanilla.

## Far terrain (LOD)

```toml
[lod]
enabled = true             # answer native-client tile requests
max_distance = 8192        # blocks from the player's last position
max_inflight_tiles = 4     # per connection, queued or in progress
workers = 1                # blocking tile threads for all connections
share_terrain_keys = false # opt-in: send the generation program (reveals the seed)
```

Only the native Solaris client (Loader platform `native`) is told about far
terrain and may ask for tiles; vanilla clients never see it. A tile is 16x16
column samples spaced 2 to 64 blocks. A stored chunk answers with its top
block as of its last stored image (later edits are not shown); a column that
was never generated answers with the generator's cave-free router surface and
biome (`TerrainGenerator::lod_column`), sampled on the server at the tile's
spacing, so far terrain costs server CPU in proportion to the never-generated
area players look at. Requests beyond `max_distance`, above the in-flight
bound or not fitting the connection's world queue are dropped; the client asks
again later and caches what it got. `workers` bounds the CPU spent.

With `share_terrain_keys = true` the server instead sends each native client
the far-terrain program (`mc_worldgen::vanilla_density::program`): the
materialized terrain and climate noise octaves, the flattened router code and
the biome parameter list, about the cost of one tile. The client then asks only
for stored columns and generates the rest itself on its GPU. The program
contains neither the level seed nor the positional seed, but each octave is
the output of a per-noise xoroshiro128++ stream whose state is the positional
seed XOR a public hash of the noise name. Inverting one stream yields the
positional seed (and with it aquifers and ore veins); one more inversion
yields the level seed, which places ores, structures and slime chunks.
Neither step was shown infeasible (bounded SAT attempts only timed out), so
treat enabling it as publishing the world seed to every native player.

## Distant Horizons

`[distant_horizons]` enables the independent Java Play custom-payload service
on `distant_horizons:msg`, DH network protocol **16** only. It is separate
from native `[lod]`; enabling DH never enables terrain-key/program sharing.
The initial Minecraft endpoint is **26.1.2 + DH 3.3.3**. MC 26.2/DH 3.3.3
and MC 26.3/DH 3.3.4 require the corresponding Minecraft presentation profile;
protocol-16 schema agreement alone is not a client compatibility pass.
Fabric and NeoForge real-client acceptance is **not yet established**.

The server advertises the optional channel, then negotiates DH config/level
identity only when a client speaks DH. A screenshot of already-visited
terrain proves nothing about this service: an unsupported server still lets
DH build local LODs from vanilla chunks the client actually received.

```toml
[distant_horizons]
enabled = false       # default off; plain Java play is unaffected
radius_chunks = 128   # chunk units; accepted range 1..=128
# Per-player request rate 2/sec, 2 outstanding sections per player,
# 8 pending sections globally, on one shared bounded LOD worker lane
# with the existing [lod] workers (never a second oversubscribed pool).
# Per-player pending DTO budget 4 MiB; shared encoded DH cache 64 MiB.
# Per-player bandwidth 500 KB/s; global 2000 KB/s.
# The bounded 64 KiB player burst admits one fully framed 32 KiB fragment:
# Minecraft compression/envelope overhead can expand already-compressed DTOs.
```

Terrain is sampled from real native block/biome/light columns at presentation
time; the server never sends the seed, the compiled density program, or
seed-derived noise tables, and level/world keys are opaque and never
seed-derived. Only player-visible material (the same anti-xray policy as
vanilla presentation) may enter a DTO. The DH radius is separate from
`view_distance`/`simulation_distance`; the gate pins vanilla render distance
to at most 8 chunks.

Rollout path: stop the world and pregenerate the initial radius through the
normal stopped-world `world pregenerate --radius 2048` (block units, plus
boundary alignment/lighting halo), then start Solaris; DH blobs build lazily
from that native data. Unsupported DH clients get no LOD service while
ordinary Minecraft play keeps working.

Requests use authoritative player position, active dimension and world
border, not client-provided generation centers. Coarse requests receive
DH's split-required error; the client requests detail-6 leaves and builds
its own hierarchy. Each leaf contains 64×64 real block/3-D-biome/light
columns, serialized as format 1 with Zstd compression mode 4. Native
coverage errors never silently become generated terrain or air. Sampling
absent coverage is off-tick and read-only, without simulation residency.

Committed resident and unflushed evicted edits take precedence over durable
images. Content publication invalidates derived cache entries; timestamp
requests return no payload only for unchanged terrain. Timestamps are pinned
per content revision in-process; restart or metadata-cache eviction may
conservatively resend unchanged terrain. Negotiated real-time updates use
split frames followed by the level-keyed update descriptor. They cover the
negotiated area even before a warm client requests its already-cached leaves;
the dirty queue stays bounded at 8192 leaves. A missed publication invalidates
the bounded current update area rather than only this connection's request
history. Disconnect,
respawn and dimension changes invalidate pending work and queued old-epoch
fragments. World identity is opaque, preserved across restart and changed
with a fresh native world.

Ordinary generation/sync requests and dirty live updates alternate when both
are pending, sharing the same two outstanding slots and 2/sec request bucket.
The existing bounded deferred-request queue retains normal requests across
their turn; continuous building cannot indefinitely starve initial LOD sync.

Known client limitation: DH 3.3.3 may not propagate material-only block swaps
(unchanged geometry) to parent LODs. Its parent merge compares compacted raw
datapoint ids without detecting a changed material mapping
(`FullDataSourceV2.java:660`, `FullDataUpdatePropagatorV2.java:254`).
The live-edit gate therefore changes both geometry and material: it extends
the gold landmark from Y=200 to Y=232 using emerald, with daylight frozen.
No synthetic light/version bits are sent to work around this upstream defect.

### Real-client gate prerequisites

The fail-closed graphical profile is
`python3 -m tools.harness run dh-gate --platform fabric` (repeat with
`neoforge`). Prerequisite discovery alone is never a pass. Exact DH publisher URLs,
sizes, SHA-512/SHA-1 hashes for all three releases, baseline loader versions
and the client prerequisite contract are in
[`tools/harness/dh-artifacts.json`](../tools/harness/dh-artifacts.json).
The profile checks both publisher hashes, not just a filename or size.

Provide the pinned jar as `SOLARIS_DH_JAR`, a worktree-built candidate as
`SOLARIS_SERVER_BINARY`, Java 25, Xvfb, Python Pillow and the real DH scenario driver in
sibling `solaris-loader` (`SOLARIS_LOADER_ROOT` selects the owned checkout).
The driver uses a private copy at `.analysis/prism-26.1.2/minecraft` inside
that checkout, never the owner's Prism instance. It mints fresh local MCP
bearer tokens per client; no external credential file is required.
Fabric is pinned to loader 0.18.5/API 0.149.1+26.1.2; NeoForge to 26.1.2.76.
Client/loader dependency manifests and hashes remain part of the evidence.
Missing prerequisites and unsuccessful scenario commands fail closed.
The scenario must compare independently clean DH caches with server DH
disabled/enabled from the same camera, vanilla distance at most 8 chunks,
DH radius 128, and a never-visited landmark 1024 blocks away. Required
receipts include actual request/DTO/render completion, vanilla-vs-DH packet
coordinates, screenshots, surface-edit/reconnect/dimension/reset checks and
a bounded adversarial pass. `dh-scenario.json`, `dh-packet-evidence.json`
and the harness `result.json` retain actual outcomes, not intended coverage.
The fixed-camera gate also checks actual upper-pillar pixels: no gold in the
DH-off control, gold in the cold DH-on frame, then emerald after the live edit.
A decoded leaf or replacement VBO can precede parent aggregation and is not
by itself a rendered-edit pass.

## Packet and movement anti-cheat

Play packet IDs in the implemented prefix (`0x00` through `0x44`) have their own
token buckets. All other IDs share one overflow bucket: changing an unknown ID
cannot renew its burst or grow per-connection limiter storage beyond 70 buckets.
The default accepts 500 packets per second with an initial burst of 1000 for each
bucket, then sends a clear disconnect if a bucket is exceeded. This protects
packet floods without pooling unrelated normal client traffic; set either packet
setting to zero only when an upstream boundary owns this protection.

The native tick also tracks survival players who repeatedly report an
unsupported position without descending. After `max_airtime_ticks` it first
corrects the player to the last supported/descending position, then disconnects
with “Flying is not enabled on this server”. Creative/spectator flight, elytra,
levitation, slow falling, water, climbables, vehicles, upward knockback and
slime blocks reset or bypass this check. Set `max_airtime_ticks = 0` only when
another authoritative movement check replaces it.

## Network bind and port

Clients connect to `host:port`. `127.0.0.1` is local-only; an interface address
accepts clients on that interface; `0.0.0.0` listens on all IPv4 interfaces.
Choose an unused port and independently configure firewall/NAT for that TCP port.
Authentication does not alter listener exposure. Offline identities can be
impersonated, including operator names; restrict offline servers to trusted
players or a correctly authenticated network boundary.

## Velocity proxies and transfers

```toml
[server]
accepts_transfers = false

[proxy]
mode = "velocity"  # or "none"
secret = "..."     # the proxy's forwarding.secret
```

With `proxy.mode = "velocity"` the listener trusts only Velocity modern
forwarding: it sends the login plugin request on `velocity:player_info`, accepts
one signed answer, verifies its HMAC-SHA256 signature against `proxy.secret`,
and adopts the forwarded UUID, name, skin properties and client address for the
session. Access lists, `/ban-ip` and the dashboard therefore see the real player
address instead of the proxy's. A direct client, a wrong secret, a mismatched
forwarded name or an unknown forwarding version is disconnected during Login.
`auth.online_mode` must stay `false` in velocity mode because the proxy
authenticates the accounts; the configuration refuses to load otherwise. The
handshake hostname is not an identity source, so bind the listener to a private
interface and expose only the proxy port.

`/transfer <hostname> [port] [players]` (operator level 3) sends the vanilla
clientbound Transfer packet to the named players (the executing player when none
are named); the client drops the session and reconnects to `hostname:port`,
default port 25565. Incoming transfers arrive with handshake intent `Transfer`
and are refused with `multiplayer.disconnect.transfers_disabled` unless
`server.accepts_transfers = true`, matching vanilla's `accepts-transfers`.

## Server console and access control

Stdin lines, with or without a leading `/`, use the native command parser and
admin path at operator level four. Feedback goes to stdout; tracing goes to
stderr, filtered by `RUST_LOG`. There is no separate TUI or automatic
`logs/profile.json` diagnostic capture.

The console source has no player entity/inventory. Relative world coordinates
and nearest selectors originate at world spawn; executor-dependent commands
need an explicit player/selector. Player-only component commands report their
normal admission error rather than inventing a console player.

Entity selectors support type filters on `@s`, multiple score objectives in
one `scores={...}` filter, repeated positive or negated NBT compound filters,
and empty `tag=` / `tag=!` tests for entities with no tags / at least one tag.
NBT and tag tests use the entity's available serialized data; `/tag` itself is
not implemented.

Examples in the running console:

```text
help
list
op PlayerName
deop PlayerName
save-all
backup
stop
```

V2 uses vanilla `ops.json` beside the config. There is no standalone
`operator add|remove|list` command or local-dev operator configuration.
Vanilla command behavior and permission levels are owned by the native admin
path. `stop` requests normal shutdown and a final durable native checkpoint;
SIGINT/SIGTERM also checkpoint on exit. `--no-console` disables stdin admission
entirely and leaves the HTTP dashboard independent.

Access lists are vanilla files beside the selected config: `ops.json`,
`banned-players.json`, `banned-ips.json` (temporary rows carry the vanilla
`expires` stamp) and `whitelist.json`. `/whitelist on|off|add <player>|remove
<player>|list|reload` and `/banlist players|ips` require operator level 3.
`whitelist.json` stays a plain vanilla array of `uuid`/`name` rows, so an
existing file can be dropped in unchanged. The enforcement toggle persists in
`whitelist.enabled` beside it (`true` or `false`); `/whitelist on` and `off`
rewrite it. `[server] whitelist` (or imported `white-list`) initializes this
state only when the file is absent, so a restart preserves console changes.
Malformed or unreadable enforcement state refuses startup rather than disabling
admission. Bans take precedence; operators bypass the whitelist as on vanilla,
but bypass the player cap only with `bypassesPlayerLimit` in `ops.json`.
`/whitelist reload` re-reads a hand-edited `whitelist.json`; `/whitelist add`
resolves a live or already-known player, or an offline-mode name.

`/kick <players> [reason]`, `/ban <profiles> [duration] [reason]` and
`/pardon <profiles>` accept player selectors such as `@a`; selectors resolve
only online players, while literal profile names can also target known offline
players (or offline-mode names). `/ban-ip` accepts an IPv4 address or an online
player name; `/pardon-ip` accepts an IPv4 address, not a player name.
`/ban` keeps vanilla's argument order and adds one optional token: a duration
such as `30m`, `2h30m`, `1d` or `1w`. Without it the
ban is permanent; with it the row's vanilla `expires` stamp makes the ban expire,
the login refusal shows the expiration line, and `/banlist` stops listing it.
Anything that does not parse as a duration is the first word of the reason, so
vanilla `/ban <player> <reason>` is unchanged. `/mute <player> [duration]
[reason]` and `/unmute <player>` use `muted-players.json` in the same directory:
a muted identity cannot send ordinary chat or native communication commands
(`/me`, `/msg`, `/reply`, `/tell`, `/w`, `/teammsg`, `/tm`, `/say`, `/tellraw`, `/title`); other command rights
are unchanged. Refused messages receive a private "You are muted." notice.
For the console, native `/msg` remains available even when a plugin reserves
that root for player commands; player-side plugin reservations are unchanged.
`/warn <player> [reason]` delivers one private warning line to an online player
and keeps no record.

Chat and native communication commands share a sender-owned spam counter:
20 points per input, one point removed per tick, and disconnection at 200
points (vanilla's chat-spam threshold). The threshold-crossing input is refused
before broadcast, and queued inputs cannot continue broadcasting after the
sender is disconnected. This protects other players' reliable queues from a
single sender's burst. Operators share this communication limit; console
commands are not charged.

### Importing `server.properties`

TOML remains the canonical configuration. Startup reads `server.properties`
beside the selected `--config` TOML when present. To select another import, set
`[properties] file = "migration/server.properties"` (relative to the TOML).
An explicitly selected missing file is an error. The import is never rewritten
and is not a second live configuration authority: precedence is **explicit TOML
leaf → imported supported property → Solaris default**. `/whitelist on|off`
persists access-list state separately, as described above. World paths retain
their normal process-working-directory semantics.

The reader supports Java properties comments, `=`/`:`/whitespace separators,
continuations, escaped delimiters and `\\uXXXX` (including surrogate pairs).
File bytes use Java `Properties.load(InputStream)` Latin-1; use Unicode escapes
for non-ASCII text. Duplicate property keys use the last value. Every unsupported
key warns by name, without printing its value; invalid supported values fail
startup. TOML overrides also warn by key.

| Vanilla property | Solaris TOML key |
| --- | --- |
| `motd` | `server.motd` |
| `max-players` | `server.max_players` |
| `view-distance`, `simulation-distance` | `server.view_distance`, `server.simulation_distance` |
| `accepts-transfers`, `pause-when-empty-seconds` | `server.accepts_transfers`, `server.pause_when_empty_seconds` |
| `white-list` | `server.whitelist` (initial persisted policy) |
| `enable-command-block` | `server.enable_command_block` |
| `server-ip`, `server-port` | `network.bind_address`, `network.port` |
| `online-mode` | `auth.online_mode` |
| `level-name`, `level-seed` | `data.world_dir`, `data.seed` |
| `enable-rcon`, `rcon.port`, `rcon.password` | `rcon.enabled`, `rcon.port`, `rcon.password` |
| `enable-query`, `query.port` | `query.enabled`, `query.port` |
| `resource-pack`, `resource-pack-sha1` | `resource_pack.url`, `resource_pack.sha1` |
| `require-resource-pack`, `resource-pack-prompt`, `resource-pack-id` | `resource_pack.required`, `resource_pack.prompt`, `resource_pack.id` |

Empty `server-ip` means `0.0.0.0`. Non-numeric seeds use Java's UTF-16 string
hash; an empty seed leaves Solaris's default rather than choosing a random seed.
Importing `level-name` does **not** import Anvil data: migrate the stopped world
explicitly before serving it. Unsupported gameplay properties do not become
gamerules; inspect warnings and the [migration guide](MIGRATION.md).

### RCON and UDP Query

Both services are disabled by default. `[rcon] enabled = true` requires a
nonempty `password`; `port` defaults to `25575`. `bind_address` defaults to
`127.0.0.1`, deliberately independent of Minecraft's listener. RCON sends its
password and full level-four console commands **unencrypted**: use a trusted
network or SSH tunnel, never an exposed Internet listener. `--no-console`
only disables stdin, not explicitly enabled RCON.

Authenticated RCON commands take the same admin/native console route as stdin,
serialized with stdin and other RCON clients. Replies contain command feedback;
native replies wait for the tick's reliable outbox to drain. The console-only
`backup` command reports admission immediately and logs its eventual result to
stdout, just as stdin does. Plugin-deferred feedback is not a synchronous RCON
completion receipt. Framing is bounded to 4096-byte request bodies; output is
split into UTF-8-safe 4096-byte frames. At most 16 RCON clients are active and
incomplete/idle reads and writes have a 30-second deadline.

`[query] enabled = true` starts GameSpy4 UDP Query on `network.bind_address`,
with `query.port = 25565` by default (separate from the TCP port). Basic stat
reports MOTD, map, player counts and game address; full stat adds version and
the player roster. Full-stat data is cached for five seconds. Source/session-bound
challenges expire within 60 seconds; requests are globally bounded to 128/second.
Query publicly reveals the player roster when enabled. Neither service scans
the world or adds native per-tick work. Bind failures refuse startup.

### Server-offered vanilla resource packs

Configure `[resource_pack] url`, optional `sha1` (40 hexadecimal digits),
`required` (default false), `prompt` (a JSON text-component string), and optional
`id` (UUID). Empty URL disables offering. Only HTTP(S) URLs are accepted; the
client downloads the ZIP, so serve it from a client-reachable URL. Without an
explicit ID, the URL determines a stable UUID. Example:

```toml
[resource_pack]
url = "https://example.org/server-pack.zip"
sha1 = ""
required = true
prompt = '{"text":"This server uses a custom resource pack."}'
```

The edge offers the pack during Configuration and waits for the matching
terminal client status before Finish Configuration. Accept/download progress
alone does not satisfy a required pack. Decline, invalid URL, download/reload
failure or discard disconnect a required-pack client; optional failures allow
joining. Unknown pack IDs and malformed statuses refuse the connection.
Negotiation is bounded to 120 seconds and 32 packets (the enclosing login
deadline may be shorter). Required-pack policy trusts the client's protocol
acknowledgement, not a proof of rendered assets. This is independent of Solaris
Loader bundle negotiation; it works without a Loader installation.

Both unsigned and signed-command packet forms enter the same permission, mute
and native spam policy. Command text keeps its 32,767-unit wire bound rather
than chat's 256-unit bound. `/msg`, `/tell` and `/w` support named recipients in
other dimensions; `/teammsg` (`/tm`) delivers only to authenticated members of
the sender's scoreboard team, including other dimensions.

Chat remains **unsigned system chat** (`enforces_secure_chat = false`).
Signed-command metadata and chat-session updates are bounded and decoded, but
their signatures/certificates are not trusted or advertised as authenticated
chat sessions. This is not secure-chat verification or reportable signed-chat
delivery. Online login authenticates the connection's identity, not each chat
message's signature.

Login reserves player capacity atomically before Configuration; pending joins
and backpressured retirement retain their reservation. Access-list policy is
checked again before native Join. A connection owns its cleanup before any
Join submission, and its Leave waits for native input capacity rather than
being dropped. Retired connections cannot recreate pre-registration stashes.

Handshake, Login and Configuration share one absolute 45-second deadline,
in addition to individual read and write deadlines. Play sends a fresh
keepalive challenge every 15 seconds when none is outstanding, accepts only
its matching echo, and closes after 30 seconds without that reply. Ordinary
traffic and partial inbound frames do not renew that challenge.

With `require_plugin_login`, a pending claimant receives an empty, temporary
presentation at a fresh spawn, not the saved inventory, vitals, XP, position or
operator advertisement. The saved native record stays untouched until a
trusted auth plugin unlocks that exact session. Multiple pending claimants can
authenticate; the first verified claim receives saved custody and other
pending claims are retired. An authenticated incumbent cannot be displaced.
Temporary claims are excluded from player persistence and the authenticated
roster, cannot activate pressure plates or travel dimensions, and cannot ask
for native LOD terrain. LOD distance follows committed position, not inbound
movement proposals.


Connection admission has two optional per-address limits under `[network]`:
`connection_throttle_ms` (minimum gap between two accepted connections from one
address) and `max_connections_per_ip` (concurrent cap). Both default to `0`
(disabled) and a refused connection is closed before the handshake.
When accepting a connection exhausts file descriptors or socket buffers while
sessions are open, the listener logs a warning and pauses admission until one of
those sessions ends; existing sessions and the core keep running. The same failure
with no open session, or any other listener error, stops the server with the
error. A single aborted or reset pending connection is skipped.

## Announcements, tick control and `/locate`

`/tellraw <targets> <component>`, `/title <targets> clear|reset|title|subtitle|
actionbar <component>|times <in> <stay> <out>` and `/bossbar add|remove|list|set|
get ...` (operator level 2) use vanilla 26.1.2 syntax: components are SNBT (JSON
text is valid SNBT), `selector` and `score` components resolve against the
sender (`nbt` components are refused), `title` times accept `d`/`s`/`t` suffixes,
and feedback uses the vanilla translation keys. Targets may live in any dimension.
Custom boss bars are one global registry (ids are `resource_location`), shown only
to the players listed with `bossbar set <id> players ...`; a listed player gets
the bar again on join. They are saved through the tick WAL and the native
checkpoint (like the scoreboard), so they survive restart; the vanilla
`custom_boss_events.dat` export is not wired.

`/tick query|rate <1-10000>|freeze|unfreeze|step [<time>|stop]|sprint <time>|stop`
(operator level 3, as vanilla) drives `tick_control`. `rate` changes the tick
period (`1000/rate` ms) and tells clients through the ticking-state packet;
`sprint` runs ticks back to back, then reports `ticks per second` to the player who
started it; `query` shows the target and the average busy time plus p50/p95/p99
over the last 100 ticks. `freeze` holds the autonomous world: non-player entities,
random and scheduled block updates, redstone, fluids, spawning, weather and the day
clock stop, `/time query gametime` stands still, while players still move, build,
chat and load chunks; `step` runs whole ticks. The WAL tick number keeps advancing
while frozen, so absolute-tick deadlines (mob timers, scheduled block ticks) are
not paused and fire on `unfreeze`. The rate, freeze and sprint state is session
state, never saved.

`/locate structure|biome|poi <id|#tag>` (operator level 2) answers asynchronously
from a worker thread; the tick never waits for the search and a second `/locate`
while two are pending (or eight are queued) is refused. Structure searches read
saved native/imported start metadata on that worker. Covered chunks without a
start suppress procedural candidates; resident metadata overrides checkpoint
metadata. Only uncovered chunks contribute immutable worldgen plans, without
generating chunks. Villages use `village_sites_in_region` within 100 chunks;
non-village starts use `structure_sites_in_region` within 256 chunks and retain
their underground Y. Native start metadata retains actual bounds separately
from the locate anchor. Corrupt/unsupported start metadata fails closed.

With local vanilla content the classic Overworld catalog includes strongholds,
normal/mesa mineshafts, desert/jungle pyramids, igloos with optional basements,
monuments, warm/cold ocean ruins, outposts with cages, ruined portals,
shipwrecks, swamp huts, mansions and buried treasure. Counts/spacing/biomes and
template content come from local assets; older room-based structures use
Solaris-authored layouts. Dungeon cavities are admitted against generated local
blocks; wells, fossils and geodes follow bounded placed-feature attempts.
All template loot tables retain their unopened table id and seed until the
runtime has first-open context; generation never rolls exploration maps.
Ocean wrecks and ruins use the solid seabed height; beached wrecks use the
water-inclusive surface. The authored monument is capped below sea level rather
than reproducing vanilla's full monument placement. Mineshaft loot is an
unopened seeded chest minecart on rails, claimed once like other generated
entities.
These remain partial families: complete rotations/processors and room
assemblers, stronghold biome relocation, the jungle piston puzzle and full
monument/mansion variations are not implemented by the classic catalog.

Eye targets and deferred exploration-map searches share the locator queue.
Map callbacks run in the native tick, not on the worker; skip-existing searches
exclude referenced starts and persisted issued-map destinations, not every
generated chunk. Registry suggestions do not imply generation. Biomes use the
generator's 3D noise biome on vanilla's 32/64-block grid out to 6400 blocks.
POIs are read from the loaded chunks within 256 blocks, with vanilla's POI-type
block lists. The tab-complete structure list comes from the local vanilla data;
boss-bar id suggestions are empty because the registry lives on the tick thread.

## Datapacks and command blocks

Solaris loads directory packs from `<world_dir>/datapacks/<pack>/` at startup.
Each pack needs `pack.mcmeta` with a `pack` object. Packs are ordered by directory
name; the lexically later resource overrides the earlier one. This is not the
vanilla enabled/disabled pack-order UI. ZIP files, symlinks, worldgen/custom
dimension resources and unimplemented resource kinds fail startup with an
explicit error, rather than silently falling back to built-in content.

Supported resources (singular 26.1.2 directory names):

- `data/<namespace>/function/<path>.mcfunction`: native commands, blank lines and
  `#` comments. Macros and leading `/` are refused.
- `data/<namespace>/tags/function/<path>.json`: `values`, nested `#tag` entries,
  optional `{ "id": "...", "required": false }` entries and `replace`.
  `minecraft:load` runs on installation/reload; `minecraft:tick` runs each
  simulation tick. Cycles and excessive expansion are rejected.
- `data/<namespace>/recipe/<path>.json`: serializers already supported by the
  native recipe parser. An existing id is replaced in place, preserving display
  indices; new ids are appended. Recipe changes are startup-only.
- `data/<namespace>/loot_table/blocks/<block>.json`: plain `minecraft:block`
  tables, one `minecraft:item` entry per pool, supported roll counts and
  `minecraft:set_count`. Empty block tables suppress drops. Other conditions,
  entry types and loot domains are refused; this is not arbitrary loot scripting.

Operator level 2 commands: `/function <id|#tag>`,
`/schedule function <id|#tag> <time> [append|replace]`,
`/schedule clear <id|#tag>`, `/datapack list [enabled]`, `/reload`.
Time uses ticks by default or `t`, `s`, `d`; zero/negative delays are rejected.
Schedules use native checkpoint/WAL post-images and survive restart.
`/reload` compiles off the tick thread, then swaps functions/tags at a tick
boundary. Changed recipe/loot resources require restart: reload refuses the
whole candidate and retains the old pack set. Load errors also retain the old set.
Functions invoked from any vanilla dimension retain their explicit command
source; schedules and load/tick tags run from the overworld server source.

Each root command/function invocation gets its own
`minecraft:max_command_sequence_length` budget, shared by nested functions and
`execute` branches rather than by unrelated commands in the same tick. Function
calls (including empty/tag-member calls), execute modifier stages and command
leaves consume that budget. `minecraft:max_command_forks` bounds the contexts
produced by a modifier stage; reaching the limit aborts that stage before its
command leaves run. Function recursion uses an explicit stack, not the Rust call
stack. Excess work is aborted with feedback. Nested cross-dimension/function
dispatch also has a 64-frame stack bound.
At most 65536 due schedules are processed per tick. Pack compilation is limited
to 16 MiB of text, 16384 resource/directory entries and 32 nesting levels.
No filesystem reads or whole-world scans are added to the tick path.

With `[server] enable_command_block = true`, unconditional impulse, chain and
repeating command blocks execute through the native command owner. Ordinary
placement requires creative mode and operator permission. Configure a placed
block using, for example:

```text
/data merge block 0 80 0 {Command:"time add 1",auto:1b}
/data get block 0 80 0
```

Operator level 2 `/data get entity <target> [path] [scale]` and
`/data get block <x> <y> <z> [path] [scale]` read native entity and block-entity
owners, including live inventory/timer overlays, without editing state or generating loot.
Feedback uses vanilla SNBT. Paths accept dotted/quoted keys, signed list/array indices,
`[]`, and compound filters (`Inventory[{Slot:0b}].count`). A path must select exactly
one value. Numeric results are floored (after optional scale); unscaled strings
return UTF-16 length and collections return size. Root queries return 1.
Current vanilla keys include `active_effects`, `equipment`, and furnace
`cooking_time_spent`; absent optional fields are errors, not invented defaults.
`/data merge` still writes only command-block `Command` and `auto`; entity/storage
modification and other block-entity writes are not supported.
Impulse blocks respond to
redstone rising edges; repeating blocks use the durable scheduled-block queue;
chains follow facing, with a bounded traversal and once-per-tick execution.
Relative coordinates resolve at the command block, not world spawn.
NBT updates and scheduled work use the native store and tick WAL. Client editor
packets, conditional execution/success counts, comparator result output and
command minecarts are not implemented. Conditional blocks are not executed as
unconditional blocks. Other map-making commands such as `/clone` and `/place`
remain unsupported.

Mechanics reference: [world simulation and scheduled work](https://github.com/kaiserproger/minecraft-how-it-works/blob/main/server-how-it-works-book/05-world-simulation.md).

`/execute` carries executor, position, rotation, dimension and feet/eyes anchor
without moving the executing entity. Supported modifiers: `as`, `at`,
`positioned <pos>|as <entities>`, `rotated <rotation>|as <entities>`,
`facing <pos>|entity <entities> feet|eyes`, `anchored`, `align`, `in`.
Conditions: `if|unless entity`, block id/properties (not block tags/SNBT), and
score comparisons or `matches` ranges. `run` dispatches the supported native
command set with the original permission level. Local `^` coordinates honor
rotation/anchor; relative `~` and selector origins honor source position.
Cross-world execution uses the shared `dimensions_travel` commit authority,
not direct edge mutation. `store result|success score <holders> <objective>`
captures native `data get`, `time query`, `gamerule` query, `worldborder get`,
`scoreboard players get`, and terminal entity/block/score conditions. Supported
mutating producers are `kill`, `summon`, `give`, `clear`, `effect give/clear`,
`teleport`/`tp`, `experience add/set`, `scoreboard players set/add/remove/reset/operation`,
`setblock`, `fill`, and the restricted command-block `data merge block` surface.
The existing command owners return typed outcomes: target counts, removed-item
counts, changed-block counts, or resulting score values as appropriate. Failures
store zero; a successful zero-valued result still stores success 1. Callbacks
follow each execute branch and update the native scoreboard's journal/broadcast
owner, including cross-world native dispatch at the shared tick boundary.
Unsupported producers fail explicitly before executing; entity/block/storage/bossbar
store destinations are not supported. Absent commands such as `tag`, `clone` and
generic `data modify` remain absent, not fabricated result producers.
Conditional block-region comparison (`blocks`)
and general NBT execute predicates remain gaps. Eye anchors use native player
pose heights or the local entity-dimensions contract scaled to body height.
The edge advertises greedy argument tails for these roots, with native syntax
validation; rich vanilla subcommand suggestions are not implemented.

Additional mechanics reference: [scheduled ticks and neighbor updates](https://github.com/kaiserproger/minecraft-how-it-works/blob/main/server-how-it-works-book/11-core-mechanics-step-by-step.md#5-scheduled-block-ticks).

## Nether generation

Fresh Nether chunks use a deterministic three-dimensional cavern field below
the bedrock roof, with a lava sea below Y=32 and all five Nether biomes. Crimson
and warped nylium, huge fungi, stems, roots and lights, soul-sand/soil floors,
and basalt/blackstone surfaces are generated on cave floors rather than on the
roof heightmap. Density corners are sampled on a fixed 4×8×4 lattice.

With the imported vanilla content cache, Nether quartz/gold and both ancient
debris passes read the vanilla ore count, height, size and exposure data.
Ancient debris never replaces an exposed terrain cell. Structure starts use
the imported random-spread sets: connected Solaris-authored fortress bridges
and rooms supply blaze spawners, wart gardens and chest loot; bastions use the
existing jigsaw solver and the imported pools/processors/templates. Nether
fossils and ruined portals use imported templates. Overlapping pieces and
fungus caps are clipped into each requested chunk from deterministic halo
anchors, independently of generation order. Structure searches consume these
same start decisions.

This is vanilla-like, not an exact-seed terrain/structure oracle. Fortress
piece diversity and structure-specific natural spawn overrides, and the
complete ruined-portal terrain/aging processor pass remain incomplete.
Bastion inhabitants are emitted as persistent generation markers; materializing
them and activating ordinary spawners belong to the native tick, not to the
generator. Existing native chunks are not regenerated.

## Forced chunks and ender-pearl loaders

`/forceload add <x z> [<x z>]`, `/forceload remove <x z> [<x z>]|all` and
`/forceload query [<x z>]` (operator level 2) are vanilla 26.1.2
`ForceLoadCommand`: block-column coordinates (`~` allowed), at most 256 chunks
per command, vanilla translation keys, and per-dimension state: the command acts
on the dimension of the player who runs it (the console source is the
overworld; `/execute in` supplies an explicit dimension). A forced chunk is a persisted ticket
(`minecraft:forced:<x>:<z>`, restored after restart and written to Anvil
`chunk_tickets.dat` on export) with no player required: like vanilla's level-31
`forced` ticket it keeps the chunk plus a two-chunk ring loaded and the chunk
plus a one-chunk ring block-ticking, so a forced farm keeps running with nobody
nearby. Entities simulate only in the central chunk unless a player's simulation
distance independently activates the neighboring chunk.

Thrown ender pearls hold a vanilla `ender_pearl` ticket (same extents, 40-tick
timeout refreshed while the pearl lives, never persisted) on their current chunk
while the owning player is online in any dimension, so a pearl keeps its
destination chunks loaded and flying. Pearls have no age limit. A pearl whose
owner is offline is vanilla's "saved with the owner": the native entity stays in
the checkpoint but is parked (it holds no ticket and does not fly, land or age)
until the owner joins again. One owner's pearls hold at most 32 distinct ticket
chunks at once, so a pearl cannon cannot demand an unbounded loaded area; excess
pearls stay frozen outside simulation. Classic bubble-column stasis chambers
need bubble-column entity physics, which Solaris does not simulate yet.

## Native mount and lectern inventories

Crafting-table grids are player-owned, not block storage: ordinary clicks keep
the table open and closing it refunds the grid and cursor to that player.

Sneak-use a tamed adult horse, donkey, mule, llama or camel to open its inventory;
the inventory key also opens it while riding. Saddle/body equipment and chested
donkey, mule or llama cargo remain owned by the entity and survive native saves.
Anvil entity export writes current mount cargo, chest attachment and llama
strength. Imported `Items` are removed from opaque metadata so withdrawals cannot
reappear on export/reimport, and newly deposited component-bearing items survive.
Mount and container-vehicle clicks honor plugin reservations on player inventory.
A refused extraction, quick move or drop preserves player inventory, cursor and
entity cargo, and returns an authoritative menu resynchronization.
Lecterns accept writable and written books, expose page navigation and Take Book,
and provide page-dependent comparator output plus a two-tick page-turn pulse.
`/setblock` and fill placement accept block-entity SNBT, including container `Items`;
supplied data replaces the target's native storage, including when the block state
itself is unchanged. Position and block-entity identity come from the placement.
`/clone` supports replace, masked and block-predicate filtered copies, with normal,
force and move overlap modes. Region comparisons use `execute if|unless blocks`
with `all` or `masked`. Regions must be loaded and obey the block-modification limit.
Block-entity copies use the command-data snapshot; live state held outside that
snapshot (such as crafter, lectern and campfire state) is not fully covered.

## Block log and rollback

A CoreProtect-style log records who changed what: player, mob (`#creeper`,
`#tnt`, `#enderman`, ...), `#fire` and plugin (`#plugin:<id>`) block changes with
the block's previous state and block-entity contents, plus player container
transactions per slot. Natural changes (fluids, growth, decay, redstone,
pistons) and console/command edits are not recorded. Operators (level 3) use:

- `/co inspect` (`/co i`): toggle the inspector; clicking a block prints its
  history and leaves the block untouched.
- `/co lookup [u:<players>] [t:<time>] [r:<radius>] [page]` (`/co l`): newest
  first, ten per page. `u:` takes comma-separated names or `#sources`, `t:`
  takes `30s`, `15m`, `1h30m`, `2d`, `1w`, `r:` is a square around you in x/z
  over all heights.
- `/co rollback u:<players> t:<time> [r:<radius>]` (`/co rb`) reverts the
  matching changes newest first in one tick; `/co restore` (`/co rs`) re-applies
  rolled-back changes oldest first. `t:` is required; without `u:` the radius
  defaults to 10.

Rollback is one fenced native command: every cell of the selection must be
loaded or nothing changes, the change count is bounded by the
`max_block_modifications` gamerule, and a record whose block type (or container
slots) someone else changed afterwards is skipped and counted, never
overwritten. A rolled-back chest gets its recorded contents back, like
CoreProtect; items the griefer already took stay in their inventory, and items
a broken container spilled stay on the ground. Rolled-back
records stay listed with `(rolled back)` and a second rollback skips them.
Commands and records are per dimension: run them in the dimension you inspect.

Records ride the tick WAL and the checkpoint stores them in the `block_log`
table of `native/state.redb`, so they survive restarts, hot backups and restores
(Anvil export does not carry them). Table rows name blocks and items rather than
protocol ids, so they carry across a game-version upgrade; a row naming a block
or item the new version lacks stops startup with its dimension and sequence,
like any other unsupported native state. Memory and table rows are bounded by
`[block_log]` retention; `enabled = false` stops recording but keeps existing
history queryable until it expires.

## Native worlds and Anvil interchange

Serving persists native `native/state.redb` and tick WAL, not Anvil region files.
Player, world and plugin effects recover at the same tick boundary. An
acknowledged operation is not an fsync guarantee: an unflushed tail may roll back
consistently after a crash. Corrupt/incompatible saved state fails closed.
Retain the same seed/content/deployment configuration and back up alpha worlds
before changing binaries or world identity. Never bypass identity validation by
hand-editing saved metadata.

Stop source worlds before conversion. Destinations must not exist; conversion
keeps the source unchanged and emits a receipt:

```sh
solaris-v2 --config server.toml world import-anvil \
  --source /srv/anvil-world --destination /srv/native-world --receipt /srv/import.json
solaris-v2 --config server.toml world export-anvil \
  --source /srv/native-world --destination /srv/exported-world --receipt /srv/export.json
```

Native Java 26.3 worlds (`DataVersion` **5023**) are accepted in addition to the
existing through-26.1.2 import range. Intermediate development/snapshot versions
remain refused. The offline reader normalizes only schema-typed block states:
26.3 `id`/`properties` become the native reader's `Name`/`Properties`; default-state
identifier strings and mixed palette wrappers become homogeneous native compound
palettes. Similarly named opaque/custom-data fields are untouched.
Chunk/entity/player envelopes,
section packing, inventories, positions and modeled saved-data fields retain
their existing shapes. The local 26.3 `DataFixers` registrations identify the
block-state change at 5006, pot template changes at 4996, and terrain-status
change at 5013; full chunks are unaffected by the status merge.

Ordinary exports remain **4790 / 26.1.2**. A record is promoted to **5023** only
when it retains a native 26.3 chunk origin or actually contains new component/
named-pot-template shapes. Promoted records use 26.3 `id`/`properties` block
states and float blending heights (including the float no-value sentinel),
not merely a newer version stamp. `level.dat` and the receipt identify
the highest emitted record version: `Version.Name = "26.3"` / `Version.Id = 5023`
if promotion was necessary, otherwise `"26.1.2"` / 4790. Unchanged saved-data
codecs retain their 4790 writer; deferred opaque files retain original bytes.

All 14 new 26.3 item components are admitted by `/give` and creative ingress
and retained in native inventory snapshots: `attack_animation`,
`interact_animation`, `block_transformer`, `villager_food`, `sulfur_cube_content`,
`compostable`, `cooking_fuel`, `brewing_fuel`, `mob_visibility`,
`provides_pottery_pattern`, `sign_text_front`, `sign_text_back`, `waxed`, and
`cushion/color`. This is lossless component custody, not a claim that native
gameplay implements each component's effect. The `unbreakable={}` item component
is also admitted and preserved in inventory and on the wire; native durability
wear does not yet respect it. `pot_decorations` stores optional
`back`/`left`/`right`/`front` item templates, including counts and nested patches;
absent sides remain absent rather than being rewritten as explicit brick faces.
Cracked-pot drops keep each present template's count and component patch; missing
optional sides do not create brick drops. Offline template traversal also accepts
identifier shorthand and mixed-list NBT wrappers without changing their custody.
Creative ingress uses the vanilla untrusted stack codec: each added outer
component has its own VarInt byte length. Ordinary inventory publications and
nested item templates keep their normal, unframed component codec.
`charged_projectiles` uses vanilla's 1024-template storage/wire bound rather than
the three-projectile count produced by ordinary Multishot gameplay.

### Interchange preservation and limits

A successful receipt means the offline conversion published a complete destination,
not that native gameplay implements every retained vanilla field. Inspect
`known_gaps`, `normalizations`, `opaque_retention`, and `upgrades` before replacing
a server. Keep the original world and receipts. Region compression, palette order,
compound order, omitted defaults and `LastUpdate` are normalized, not byte-identical.

| Data | Interchange custody |
|---|---|
| Block states, including waterlogged/facing/other properties | Registry state identity; unknown states/properties refuse import. Missing properties inherit the registered default. Empty palettes, missing multi-state packed data and out-of-range palette indices refuse decoding rather than replacing terrain. |
| Three-dimensional biomes | Every 4×4×4 cell per section is retained by biome identifier, independent of palette order. |
| Heightmaps and lighting | Named heightmap arrays and section/boundary sky/block light arrays are retained. Native block edits may invalidate/recompute light and heightmaps; unchanged-world conversion is the preservation case. |
| Chests/barrels, furnace/smoker/blast furnace, hoppers, brewing stands | Inventory/components and owned timers are typed. Unowned block-entity metadata (including `CustomName`, `lock`, `components`) is retained alongside the native container and merged without resurrecting old inventory or consumed loot tables. Retention of a lock is not enforcement of that lock during native gameplay. Copper-chest loot is materialized at admission, not an untouched loot-table round trip. |
| Signs (front and back), banners, spawners, shulkers, lecterns, beehives, campfires, jukeboxes and other block entities | Complete NBT retained by position unless a native gameplay mutation replaces it; storage/trial/conduit payload validators may refuse unsupported forms. This is interchange custody, not proof of every block entity's gameplay parity. |
| Villagers (offers/gossip), item frames, armor stands, paintings, minecarts, boats and other entities | UUID and shared fields are decoded, species-specific/unowned fields retained; native hydration validates supported species payloads. Unsupported forms fail with file/chunk/entity UUID/type context. Wire IDs are reminted and are not vanilla identities. Duplicate UUID bodies are dropped deterministically with explicit receipt losses and separate original-NBT artifacts. |
| Player inventory/equipment, ender chest, XP, effects, spawn, attributes, recipes | Native-owned fields are mapped; unowned player fields are retained. Recipe-book recipes are refreshed from native custody while other book fields survive. Saved attribute NBT retention does not imply native attribute behavior. Absent-dimension login/respawn fallback is recorded as a preservation loss. |
| Advancements/statistics | Owned JSON is imported/exported semantically; orphan advancement documents remain opaque. Statistics-only UUID files can create native offline-player records. |
| World rules, time, weather, border, dragon fight, wandering trader | Typed metadata is refreshed from recovered native state. 26.1.2 stores several formerly `level.dat` fields in separate saved-data files. Unowned envelope fields are retained; deferred old saved data conflicts with an active native owner fail export. |
| Maps, scoreboards/teams, raids | Maps are upgraded/retained as saved data; scoreboard/team and raid state have typed native owners. Each score retains its `Locked` flag: newly created scores are locked, `/scoreboard players enable` unlocks a trigger score, and `/trigger` consumes that permission and re-locks it. Unknown saved-data envelope fields are merged by record identity, not list position. |
| POI, structure starts/references | POI files remain byte-exact opaque files, not native POI authority. Full-chunk structure metadata is retained. Empty protochunk regeneration may discard structure starts/references and records the loss. |
| Scheduled ticks | Native queues retain identity, priority and remaining delay. Empty-fluid no-ops, foreign-chunk work and exact duplicates are normalized and counted; delays outside the Anvil integer range fail export. |
| Protochunk statuses | Non-full chunks with terrain or owned block-entity/tick payload are stored with their status. All-air non-full chunks without owned payload are omitted for native regeneration; counters and loss records identify discarded fields. Solaris generation is not seed-equivalent to vanilla generation. |
| Other `data/*.dat` and auxiliary files | Explicit allowlist only; unknown active files, custom dimensions/worldgen descriptors and executable datapacks fail closed. Known opaque saved data, player backups, icons and resource packs are retained subject to a 16 MiB aggregate conversion-extras record limit. |

Native-only plugin/settlement state and block-log history have no vanilla runtime
equivalent; Anvil is not a backup format for these domains. Use native backup/restore
when their recovery is required. Running either server between conversions changes
its authoritative state; comparing those worlds is not an unchanged-NBT round trip.

The headless command-fixture gate is `python3 -m tools.harness.anvil_interchange
--binary /path/to/solaris-v2 --jar /path/to/server-bundler.jar --eula-file
/path/to/already-accepted/eula.txt --cache /path/to/content --output /new/artifact/path`.
It retains the source, conversion receipts, normalized NBT differences (including
added fields), command feedback, and Java/native logs. It separately checks an
unchanged import/export and a native edit/export → Java open/save → reimport.
It does not accept legal terms or claim real-client/gameplay coverage.
Supply the official 26.1.2 bundler JAR, not its extracted inner server JAR (which
requires a separate library classpath).

Anvil is a compatibility format, not live fallback or dual-write storage.
Earth WorldPlan preparation is not ported. Simulation tickets are not an
offline generator: using them advances simulation and persists ticket
ownership; use [`world pregenerate`](#offline-chunk-pregeneration) instead.

A worldgen revision bump refuses existing worlds (`identity mismatch`). To keep a
stopped world, back it up and run `solaris-v2 --config server.toml world
upgrade-worldgen`: it rewrites only the stored revision (refusing any other
difference), leaves generated chunks as they are and applies the new generator to
chunks generated afterwards, with a hard seam at the old explored border. See
[Village generation](VILLAGE_GENERATION.md#keeping-a-world-across-a-revision-bump).

## Offline chunk pregeneration

`world pregenerate` fills the native store with overworld terrain before players
connect, so a joining player reads chunks from disk instead of waiting for
worldgen. Stop the server first: the command opens the world exactly like
serving (same plugin-applied registry, village plans and plugin worldgen rules),
holds the exclusive world-directory lease for the whole run and refuses a live
world. A missing world directory is created as a fresh world.

```sh
# 512-block radius around the world spawn
solaris-v2 --config server.toml world pregenerate --radius 512
# explicit block center for a planned hub; negative coordinates are ordinary
solaris-v2 --config server.toml world pregenerate --radius 256 --center=-1024,2048
```

`--radius` is a block radius on both axes: `512` covers 65x65 chunks, and the
command fails closed above 4,194,304 chunks. Progress lines report completed,
generated and already-stored chunks once per second. Chunks are written as
immutable native baselines in batches of 64 using the same generation workers
and light bake as the live world, so a pregenerated chunk is what a join would
have generated. Chunk positions the store already covers are skipped: an
interrupted run resumes, and a rerun only generates what is still missing.
Only the overworld is pregenerated; the Nether and End keep generating on
demand. There is no in-game or console pregeneration command.

## World size, pruning and compaction

Chunk baselines and chunk side-state rows in `native/state.redb` are stored
zstd-compressed with a content checksum: light arrays, heightmaps, biomes and
palettes are highly repetitive. A pregenerated overworld chunk costs about 3.7 KB
of rows (5.5 KB of file) instead of 126 KB (255 KB of file: redb rounds large
values up to power-of-two blocks). Rows written by older builds stay readable as
plain NBT; the next save of that chunk, or `world compact`, rewrites them
compressed. A packed row that fails its length, frame or checksum validation is
refused like any other corrupt native record.

These commands need the server stopped: they take the exclusive world lease and
refuse a running world. Back the world up first.

```sh
# rewrite plain rows compressed, then return free pages to the file system
solaris-v2 --config server.toml world compact
# what would a prune forget? (writes nothing)
solaris-v2 --config server.toml world prune --keep-radius 4096 --dry-run
# forget untouched chunks outside 4096 blocks of the world spawn, then compact
solaris-v2 --config server.toml world prune --keep-radius 4096
solaris-v2 --config server.toml world prune --keep-radius 2048 --center=-1024,2048 --max-inhabited-ticks 600
```

`world prune` removes overworld chunks outside the `--keep-radius` block radius
(around `--center`, default the world spawn) whose baseline the Solaris generator
created (pregeneration, generation during play) and that still equal it: no
overlay section (edited block), no persistent entity row (consumed generated-slot
tombstones included), no side-state difference, and an inhabited clock of at most
`--max-inhabited-ticks` (default `0`: never inhabited). Everything else is kept:
edited, inhabited or entity-holding chunks, and every baseline that is not
recorded as generator-made. That includes all chunks imported from Anvil, even
with an inhabited clock of 0, because the generator cannot recreate them, and every
chunk stored by a build before generator provenance was recorded (such worlds
prune nothing; the report counts them as "kept not generator-made"). A pruned
chunk is generated afresh with the current generator the next time anyone visits
it, like any chunk the store never held; after `upgrade-worldgen` that is new
terrain. The command opens and closes the world once first, so a crash's journal
tail is folded into the store before any chunk is judged, then compacts the file
and prints the file size before and after. The Nether and the End are not pruned.

## Native backup and restore

The offline commands need the server stopped: they use the same exclusive
world-directory lease as startup, also lock the native database and refuse live worlds:

```sh
solaris-v2 backup --world-dir /srv/solaris/world --out /backups/world-2026-10-15
solaris-v2 restore --from /backups/world-2026-10-15 --world-dir /srv/solaris/restored
solaris-v2 --config restored-server.toml
```

The backup contains native database, WAL/journal and other world files with a
versioned SHA-256 manifest; lease files are excluded. Restore verifies the full
inventory and hashes before writing world data. The target must be empty or
absent, separate from the backup and exclusively leased. Missing/changed/extra
files, incompatible manifests and symlinks are refused. Retain TOML, plugin
artifacts and access lists separately.

### Hot backup of a running server

Set a destination and type `backup` in the server console (not available to
in-game players; refused while `backup_dir` is unset or `--no-console` is used):

```toml
[data]
backup_dir = "/srv/solaris/backups"
```

The server forces a native checkpoint, then its single database writer copies
`native/state.redb` and the contiguous tick-WAL batches appended since into
`backup_dir/backup-<UTC>-tick<N>/` with the same manifest as the offline
command. `N` is the last tick the copy recovers; players keep playing and no
tick is paused. The copy runs off the tick behind earlier native writes, so the
next periodic checkpoint and generated-chunk saves wait until it finishes (WAL
appends continue; dirty state stays resident meanwhile). The copy is assembled
in a hidden `.backup-<UTC>.partial` directory and renamed only after the
manifest is durable; a failed backup leaves nothing behind. Restore it with
`solaris-v2 restore --from <backup> --world-dir <new>` or point `world_dir`
directly at a copy of the backup directory: opening replays the WAL to tick `N`,
exactly as crash recovery does. It covers only world state (database and WAL);
retain TOML, plugin artifacts and access lists separately. Backups are never
pruned automatically.

## Plugins

```toml
[plugins]
directory = "plugins"
```

Each child is a deployable package with `plugin.toml`, `plugin.wasm`, optional
`config.toml` and declared resources. The directory is an exact deployment:
malformed entries, duplicate ids/command roots, failed startup and unsupported
declarations refuse startup. Selecting it authorizes manifest-declared ordinary
capabilities. There are no current strict/expected/grants or hot-reload settings.
A configured directory that does not exist yet (the first start from
`example.toml`) is created empty and logged; the server then runs without
plugins. Deploying an auth-capable package such as `solaris-auth` locks every
new session until the package unlocks it, regardless of `[auth]
require_plugin_login`.

Manifest `before-build` hooks register in deterministic package-id order with
fail-closed policy. Unsupported hooks, including `before-damage`, fail startup.
Server-only packages accept vanilla clients; client bundles require the matching
Loader and player permission approval. See [PLUGINS.md](PLUGINS.md) and
[SOLARIS_LOADER.md](SOLARIS_LOADER.md). Production builds do not need sibling
plugin source or compile guests at startup.

### Land claims and anti-grief

Deploy the standard-pack `solaris-towns` package to protect land. A player runs
`/town create <name>`, stands in a chunk and runs `/town claim`; each claimed chunk
becomes a protected zone the **native** server enforces without any hook, for every
player not on the zone's allow-list (the town's leader, officers and members, plus
trusted guests). Operators are always admitted. Inside a protected zone a
non-listed player cannot break or place blocks, empty buckets, open containers
(chests, barrels, furnaces, shulker boxes), light fire or TNT, interact with or
leash entities, ride or open entity inventories, or damage players and every
non-hostile entity (so animals, villagers, item frames and armor stands are safe;
hostile mobs stay fair game). Explosions never destroy a claimed block and fire
never spreads into one, whoever caused them. Leaders and officers manage guests
with `/town trust <player|uuid>` and `/town untrust <player|uuid>`
(`maximum_trusted_per_town`, at most 16); the leader can open doors, trapdoors,
fence gates, buttons and levers to everyone with `/town flag use on` (`off`
reverts). `/town info [town]` shows members, claims, trusted guests and the door
flag. Zone state is part of the native checkpoint/WAL; a plugin that rewrites a
zone replaces its allow-list atomically.

### Death graves

Deploy the standard-pack `graves` package to keep a death's drops in the world
instead of scattering them. The package declares the `death_graves` capability,
and that declaration is what makes the **native** death path act: with
`keepInventory` off, the exact dropped stacks move into a barrel placed at the
death spot in the same tick, journaled with the world. Placement skips cells
inside another package's protected zone, and stacks that the 27 slots cannot hold
are dropped beside the grave, so a full inventory loses nothing. Experience is
retained by the player; Solaris spawns no death XP orbs. With `keepInventory` on,
no grave is created. Removing the package returns deaths to vanilla drops.

The plugin protects the grave for its owner through the native zone owner and
after `public_after_seconds` (default 300) removes that protection so anyone may
open it. Operators are always admitted. At most one package may declare
`death_graves`; two make startup refuse the deployment.

Custody is verifiable on the saved world without starting the server. Stop it
normally (console `stop`), then read the exact cells, container slots, stored
player inventories and stored item entities:

```sh
solaris-v2 --config server.toml world probe --source WORLD --receipt probe.json \
  --block X,Y,Z
```

The receipt is PASS only when every step completed; it is refused when the
journal holds complete batches after the checkpoint (start and stop once more to
fold them) and the receipt must lie outside the world directory. The tracked
harness profile `graves` runs the whole scenario end to end: real protocol
death, clean stop, probe, then fail-closed assertions that the barrel at the
journalled `grave-<x>-<y>-<z>` cell holds exactly the given stacks, that the
stored player inventory keeps no copy, and that no matching item entity exists.

```sh
python3 -m tools.harness run graves -- --plugins-dir DEPLOYED_PACK
```

## Dashboard

The default-off dashboard is read-only and unauthenticated. It exposes JSON
at `/stats`, `/tps` (rolling TPS plus p50/p95/p99 MSPT), `/profile`, and
`/load-metrics`, and a Prometheus text scrape at `/metrics`.

```toml
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = 8080
allow_remote = false
# Optional: scrape with `Authorization: Bearer <token>`.
metrics_bearer_token = "replace-with-a-secret"
```

Open `http://127.0.0.1:8080/metrics` while serving. The scrape includes TPS,
tick p50/p95/p99, the latest per-phase timings, online players, loaded chunks,
entity count, resident memory, network bytes, reliable drops, and skipped pump
ticks. A configured token protects only `/metrics`; retain the loopback bind or
put the JSON dashboard behind an authenticated reverse proxy. Non-loopback
addresses require `allow_remote = true`; otherwise the dashboard is not started
and a warning is logged.

Operators with permission level 2 may run `/tps` or `/mspt` in-game. Both
report the dashboard's rolling tick window; this is observation only and does
not alter tick scheduling or simulation distance.


### Minecraft Server Management Protocol

Solaris exposes the vanilla 26.1.2 MSMP subset as JSON-RPC 2.0 over a
Bearer-authenticated WebSocket. It is disabled by default and refuses startup
without a token:

```toml
[msmp]
enabled = true
bind_address = "127.0.0.1"
port = 25585
bearer_token = "replace-with-a-long-secret"
allow_remote = false
```

The endpoint implements `rpc.discover`, player list/kick, allowlist, bans,
operators, status, save, stop, and system-message service methods. Keep it on
loopback or terminate TLS at an authenticated reverse proxy; this plaintext
endpoint is intentionally not exposed by default.
## Evidence boundaries

Use [AGENT_TOOLING.md](AGENT_TOOLING.md#harness-entrypoint) for registered
validation profiles. Historical preliminary-release instructions, regional-owner
benchmarks, multi-journal recovery receipts and prior console profile captures
retain their original meaning; they are not current operator commands or proof
of native-client acceptance. Maturity remains **draft**.
