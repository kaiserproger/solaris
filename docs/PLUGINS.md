# Luau Plugins

Solaris exposes one production Luau plugin contract: API `0.6.0`. A manifest
requesting any other version is rejected. There is no legacy manifest or Luau
API compatibility path.

The future full server/client addon platform is specified separately in
[`LUAU_ADDON_API_1_0_SPEC.md`](LUAU_ADDON_API_1_0_SPEC.md), with its parked
implementation backlog in
[`LUAU_ADDON_API_1_0_TASKS.md`](LUAU_ADDON_API_1_0_TASKS.md). Those tasks are
blocked until the scoped vanilla 26.1.2 parity gate is closed and the owner
explicitly activates the first addon-platform milestone; they do not describe
current API `0.6.0` behavior.

`mc-net` provides the `0.6.0` plugin-storage, zone, inventory-menu,
inventory/storage transaction, player-inventory transaction, player-teleport,
connected-player query, generic villager-binding, and bounded villager-goal
adapters. It also publishes committed player block breaks and owner-targeted
zone membership transitions. Colony identity, roles, orders, and persistence
remain plugin-owned Luau policy rather than Rust API concepts.

## Package And Manifest

The configured plugin directory contains one directory per plugin:

```text
plugins/
`-- basic-economy/
    |-- config.toml     # optional operator configuration
    |-- plugin.toml
    `-- main.lua        # strict Luau source
```

Every source is parsed and type-checked as `--!strict` Luau before it may claim
commands or receive events, even when the file omits the directive. The shipped
sources include `--!strict` explicitly. Solaris supplies a type-check-only
`solaris` host prelude, rejects diagnostics, then executes the accepted source in
one sandboxed Luau VM with bounded memory and interrupt fuel. A normal invalid
plugin is skipped; a plugin declaring startup worldgen or client content fails
server startup instead of silently changing the world/client contract.

External and server-embedded plugins can be selected together:

```toml
[plugins]
directory = "plugins" # optional external root
bundled = ["basic-economy", "online-roster"]
```

The available bundled ids are `basic-economy`, `colony-villager-scaffold`,
`geological-mines`, `land-claims`, `online-roster`, and
`settlement-prototype`. They are disabled unless explicitly listed. Duplicate
plugin ids across either source fail startup before command, Loader, or worldgen
metadata can diverge. Ore and settlement ownership conflicts remain fail-fast.

Every currently shipped bundled example is **Server-only** and accepts an
ordinary vanilla 26.1.2 client. The separate
`examples/loader-live-gate` fixture is **Requires Solaris Loader on client** and
exists for the Fabric/NeoForge/Forge compatibility matrix.

| Example | Deployment |
| --- | --- |
| `basic-economy` | **Server-only** |
| `colony-villager-scaffold` | **Server-only** |
| `geological-mines` | **Server-only** |
| `land-claims` | **Server-only** |
| `online-roster` | **Server-only** |
| `settlement-prototype` | **Server-only** |
| `loader-live-gate` | **Requires Solaris Loader on client** |

```toml
id = "basic-economy"
name = "Basic Economy"
version = "0.4.0"
api = "0.6.0"
events = ["server.started", "player.left"]
capabilities = ["storage", "inventory_menus", "inventory_storage_transactions", "zones"]
player_commands = ["economy"]
```

Optional startup-only worldgen declarations are also available:

```toml
[worldgen]
ore_profile = "geological_deposits"
settlement_profile = "plains_village_prototype"

[[worldgen.settlement_buildings]]
id = "smithy"
template = "plains_toolsmith"
role = "workplace"

[[worldgen.settlement_inhabitants]]
id = "smith"
kind = "villager"
building = "smithy"
job = "toolsmith"

[[worldgen.settlement_extensions]]
id = "work-orders"
building = "smithy"
```

Installing `examples/plugins/geological-mines` selects large deterministic
cross-chunk deposits and disables the vanilla ore pass for that world. Without
a declaration the ore profile remains `vanilla`.

Installing `examples/plugins/settlement-prototype` selects one bounded plains
village prototype. Solaris loads the vanilla fountain, small-house, and
toolsmith NBT templates from `data.vanilla_data_dir`, combines the declared
building templates at stable offsets, and uses the extracted vanilla village
spacing/separation/salt. Omitting `settlement_buildings` selects all three
prototype parts. Seed zero fixes the prototype near spawn; other seeds use
deterministic grassland placement. Missing template data fails startup instead
of substituting a Solaris-authored building.

Settlement descriptors are startup-only, immutable, and owned by the plugin
that declares the settlement profile. A plan has at most three uniquely
selected building templates, 16 named inhabitants, and 16 extension records;
all ids are lowercase bounded literals. Inhabitants and extensions must
reference a declared building. Extension ids are materialized as
`plugin-id:local-id`, so one plugin cannot claim another plugin's extension
namespace. The closed prototype vocabulary currently supports fountain,
small-house, and toolsmith templates; meeting-point, home, and workplace
building roles; villagers; and unemployed/toolsmith jobs.

Ore and settlement profiles may have different plugin owners. Two plugins
declaring the same profile kind fail startup instead of relying on directory
order. The ore profile and canonical settlement plan (owner plus every ordered
descriptor) are persisted in `solaris/world.json`; changing either requires a
fresh world directory so old and new chunks cannot mix authorities.
Declarations are resolved before pre-generation and give Luau no chunk,
generator, lock, or worker handle. An invalid/empty declaration or missing Luau
source fails startup. Unversioned vanilla Anvil imports reject plugin worldgen
profiles because Solaris does not generate missing chunks in imports.

The deterministic startup plan now covers per-building selection and roles,
inhabitant selection, job assignment, and bounded plugin-owned extension
records without giving Luau mutable worldgen callbacks. The plan is validated
before generation and its selected building parts directly determine the
composite template. Solaris extracts the templates' vanilla villager jigsaw
slots and persists the planned inhabitants as typed chunk markers. When such a
chunk is installed, a dedicated system-owned simulation command materializes
the villagers with plains type, declared profession, and level-one metadata.
The per-inhabitant claim is durable and independent of ambient-herd admission,
so a reload or later chunk installation cannot duplicate the planned resident.

### Client Content Manifest

A plugin may declare startup-only Solaris Loader bundles in `plugin.toml`:

```toml
[client]
schema = 1

[[client.bundles]]
id = "rich-content"
version = "1.2.3"
artifact = "client/rich-content.zip"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
size_bytes = 4096
loaders = ["fabric", "neoforge", "forge"]
content = ["blocks", "items", "screens", "assets", "interactions"]
permissions = [
  "register_blocks",
  "register_items",
  "open_screens",
  "load_assets",
  "send_interactions",
]
```

Schema 1 is closed and shared by all three loaders. Each content kind requires
its matching permission. A plugin may declare at most eight bundles; each
artifact is capped at 64 MiB, uses a relative canonical path, and carries a
lowercase 64-character SHA-256. The cache identity is
`plugin-id:bundle-id/version/sha256`, so changing bytes requires a new identity
even if an operator reuses a display version.

When at least one bundle is declared, Solaris sends the combined manifest
during Configuration. The client must acknowledge protocol 1, its exact
platform and loader version, all required permissions, and every cache identity
before the server accepts `AcknowledgeFinishConfiguration`. A server with no
client bundles sends no Solaris Loader payload and preserves the vanilla
configuration path.

`solaris --check` and startup discovery logs derive deployment requirements from
the validated manifest. For every plugin they report `server_only` or
`server_and_client`, the sorted supported-loader and permission sets, client
bundle identities (`id`, `version`, artifact path and SHA-256), each artifact
size, and the total artifact bytes. There is no duplicate deployment flag. When
a required Loader handshake fails, Solaris sends a Configuration-state
disconnect naming the supported loaders and required bundle identities before
closing the connection.

The Fabric, NeoForge, and Forge clients register these Configuration payloads
through their native 26.1.2 networking APIs and delegate the received bytes to
the same Java validator. On first contact with an exact normalized server
address and permission set, each adapter opens the native Minecraft confirmation
screen. The shared core stores the allow or deny decision in
`permissions.properties` under the Loader cache. Decisions are not shared
between server addresses, and a changed permission set prompts again. The cache
defaults to `~/.solaris/loader-cache`; `solaris.loader.cacheDir` overrides it.
The server gives this Loader-only Configuration exchange two minutes; unrelated
pre-Play phases retain their ten-second read timeout.

If an exact cache identity is absent or its file fails the declared size/hash,
the client requests that identity on `solaris:loader/request`. The server
streams only the matching plugin artifact in bounded
`solaris:loader/artifact` chunks. The client requires contiguous offsets,
stages in the final cache filesystem, verifies exact size and SHA-256, and uses
an atomic move before including the identity in `solaris:loader/ack`. Plugin
startup also rejects a missing, escaping, wrong-size, or wrong-hash source
artifact.

A denial emits no artifact request, creates no staging file, and disconnects
without acknowledgement. Once every cache file is verified, the client reads a
closed `solaris-client.json` index from the first ZIP entry. The implemented
index schema accepts owned `screens` (`id`, `title`, `body`, optional
`item_id`/`block_id`), one owned `blocks` entry (`id`, `model`, `name`), up to 128
owned `items` (`id`, `base_item`, `name`), and `assets`
(`id`, canonical `assets/...` path, exact SHA-256, and exact byte size), rejects
all undeclared archive entries, and bounds the activated registry to 64 screens,
one block, 128 items, 128 assets, and 64 MiB of asset bytes. A block requires
`register_blocks` and its exact verified owner model under
`assets/<namespace>/models/<path>.json`. Every item requires
`register_items`, a known `minecraft:*` base item, and its exact verified
`assets/<namespace>/items/<path>.json` definition. It also accepts up to 64 owned
`interactions` (`id`, `screen_id`, `label`, `payload`). An interaction must
reference a screen declared by the same bundle; one screen has at most eight
actions, labels are at most 64 bytes, and the UTF-8 payload is at most 4 KiB.
Fabric, NeoForge, and Forge publish the same immutable registry before
acknowledgement and retain it into Play. Denied, malformed, or unverified
bundles never publish content. A plugin
whose bundle declares
`screens` plus `open_screens` may call
`solaris.open_client_screen(player_id, "plugin-id:screen-id")`. Solaris routes
that Play payload only to the exact player session that completed the Loader
acknowledgement; vanilla, disconnected, closed, and unknown sessions are
rejected. Fabric, NeoForge, and Forge resolve the id only from the activated
registry belonging to the packet's exact originating connection and open the
bounded title/body screen on the client thread. All three clients clear the
registry on logout before another server connection can reuse the process.
Every verified asset path under `assets/<namespace>/...` is also published as
that exact Minecraft resource id through one transient required pack. The
client sends the Loader acknowledgement only after the pack reload exposes the
exact verified bytes. A close event from that same Configuration connection
removes the pack and reloads resources; a stale close cannot remove a newer
connection's pack.

An activated screen renders its declared interactions as buttons. Pressing one
sends `solaris:loader/interaction` only if the screen definition and originating
connection are still current. Solaris accepts the bounded action only from that
player's exact Loader-acknowledged Play session, requires an owner bundle with
`interactions` plus `send_interactions`, and targets only that owner plugin's
Luau `on_loader_interaction(event)` handler. The event fields are `player_id`,
`interaction_id`, and `payload`. Client payloads remain untrusted plugin input;
the namespace fence prevents one bundle from addressing another plugin.

When a screen references an activated item, Fabric, NeoForge, and Forge build
the same local vanilla stack after the verified resource pack reload, assign
the item's owner-namespaced Minecraft 26.1.2 `ITEM_MODEL` plus declared name,
and render it with the standard item widget. This presentation path does not
mutate the frozen item registry. The block-specific server grant is described
below; player-driven item use is not part of the current slice.

The block prototype pre-registers one `solaris_loader:loader_block` block/item
carrier before registry freeze on Fabric, NeoForge, and Forge. Once the verified
pack is visible, deterministic carrier blockstate/item definitions point to the
declared owner model. A screen referencing `block_id` renders that block through
the standard block item. For a block bundle, `solaris:loader/ack` also carries
the exact non-negative 26.1.2 runtime id of that pre-registered carrier state.
At startup Solaris reads the one owned block id from the first index entry of
the already size/SHA-verified plugin artifact. ACK validation binds that owner
id to the reported carrier state only for the exact acknowledged Play session;
a missing, unexpected, or non-VarInt state is rejected. This is not a
vanilla-block substitution. Solaris keeps one full, opaque, non-emitting
canonical server-owned state after the frozen vanilla state range in the
server block and light tables and projects it through the exact session's
mapping in both block updates and chunk palettes. Projected chunk frames are
not shared across sessions. The owning
host-attested plugin can place that exact canonical state with
`solaris.place_loader_block("plugin-id:block-id", x, y, z)`. Solaris rejects a
foreign or unknown block id and commits accepted coordinates through the
server-owned block-edit transaction, so world storage never contains a client
runtime id. The same exact owner can call
`solaris.grant_loader_block_item(player_id, "plugin-id:block-id", count)` with
`count` in `1..=64`. The target must be the exact live session that
acknowledged that block carrier. Solaris merges a canonical
`minecraft:paper` stack carrying the verified block name and
`solaris_loader:loader_block` `ITEM_MODEL` into the player's normal inventory,
persists it before publication, and leaves a full inventory unchanged.
When that exact stack is used on a block, only the live session that
acknowledged the carrier may resolve it to the canonical owner block. Solaris
then reuses normal survival placement validation and atomically commits the
world edit plus one-item debit through the canonical player persistence path.
Wrong-model, wrong-base, unacknowledged, stale-hand, and rejected placements
leave both world and inventory unchanged. Survival breaking that canonical
Loader state now replaces ordinary loot planning with the same named
`minecraft:paper` plus `solaris_loader:loader_block` presentation. The
authoritative item entity carries `CUSTOM_NAME` and `ITEM_MODEL` through wire
publication, entity persistence, partial claims, and the existing simulation
owner pickup/inventory commit; no Loader-specific direct inventory credit is
used. A missing ACK or a different canonical state cannot select this drop.
Multiple simultaneous block carriers remain a later slice.

Plugin ids use lowercase ASCII letters, digits, `_`, `-`, or `.`. Command roots
remain lowercase ASCII literals of at most 64 bytes. Plugin command roots are
globally exclusive, bounded to 128 roots, and cannot shadow a Solaris built-in.
`console_commands` and `spawn_entities` remain exact allow-lists. A resource
identifier must be fully namespaced lowercase ASCII and no more than 128 bytes.
`relation` is `required`, `optional`, or `load_before`.

Discovery reads at most 128 plugin directories. `plugin.toml` is capped at 64
KiB and `main.lua` at 1 MiB. The loader checks file metadata first and then
performs a capped streaming read, so a growing or sparse file cannot bypass the
limit. Plugin ids and versions are at most 64 bytes, display names at most 128
bytes, and every manifest string/list is bounded before the normalized manifest
allocates. A manifest may contain at most 64 events, 64 dependencies, 128
capabilities, 64 permissions, and 128 player or operator command roots.

## Plugin Configuration

`config.toml` is optional. The loader reads it once during discovery, before
the plugin registers commands or receives events. A missing file becomes an
empty table. `solaris.config()` returns a new recursive Luau table on every
call, so a plugin may mutate its local copy without changing later reads. Disk
changes after startup do not change the loaded snapshot; live reload,
environment interpolation, default merging, and cross-plugin reads are not
part of API `0.6.0`.

```luau
local config = solaris.config()
assert(config.currency.resource == "minecraft:emerald")
assert(config.catalog[1].price == 3)
```

Accepted TOML values are strings, signed 64-bit integers, finite floats,
booleans, arrays, and tables. Arrays become one-based Luau tables. TOML dates
and times are rejected. The file is capped at 64 KiB; nesting at 8 container
levels; every table or array at 128 entries; keys at 128 UTF-8 bytes; and
strings at 4096 UTF-8 bytes. Validation is eager and recursive. An invalid
configuration skips only that plugin before it can claim command roots.

`capabilities` is an exact, duplicate-free list:

| Capability | Allows |
| --- | --- |
| `storage` | `storage_get`, `storage_cas`, `storage_delete` |
| `inventory_menus` | `open_inventory_menu`, `close_inventory_menu` |
| `inventory_storage_transactions` | `inventory_storage_transaction` |
| `player_inventory` | `inventory_transaction` |
| `zones` | `upsert_zone`, `upsert_protected_zone`, `remove_zone`, owned zone entry/exit events |
| `villagers` | `bind_nearest_villager`, `set_villager_idle`, `move_villager_to` |
| `player_teleport` | `teleport_player` |
| `player_queries` | `list_online_players` |

An undeclared privileged call fails synchronously in Luau before it enters the
bounded command batch. Unknown capabilities reject the plugin during discovery.

## Events

`events` subscribes to broadcast events. All event values are immutable DTO
snapshots. `player.command` and every result/owner event below are targeted: the
host routes them to exactly the owning plugin and never broadcasts them. A
targeted event does not need a broad subscription to reach its owner.

| Event | Luau handler | Fields |
| --- | --- | --- |
| `server.started` | `on_server_started` | `name` |
| `server.stopping` | `on_server_stopping` | `name`, `reason` |
| `player.joined` | `on_player_joined` | player snapshot |
| `player.left` | `on_player_left` | `player_id`, `reason` |
| `player.chat` | `on_player_chat` | player snapshot, `message` |
| `player.block_broken` | `on_player_block_broken` | block player snapshot, `dimension`, `block_id`, `x`, `y`, `z`, `game_mode` |
| `player.block_placed` | `on_player_block_placed` | block player snapshot, `dimension`, `block_id`, `x`, `y`, `z`, `game_mode` |
| `player.item_crafted` | `on_player_item_crafted` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `dimension`, `item_id`, `count`, `craft_count`, `source`, `game_mode` |
| `player.item_picked_up` | `on_player_item_picked_up` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `dimension`, `item_id`, `count`, `source`, `game_mode` |
| `player.entity_killed` | `on_player_entity_killed` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `dimension`, `entity_id`, `entity_type`, `source`, `game_mode` |
| `player.entity_interacted` | `on_player_entity_interacted` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `dimension`, `entity_id`, `entity_type`, `hand`, `secondary_action`, `game_mode` |
| `player.died` | `on_player_died` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `dimension`, `game_mode` |
| `server.tick` | `on_server_tick` | `tick` |
| `plugin.timer` | `on_plugin_timer` | `name`, `timer_id`, `scheduled_tick`, `fired_tick` |
| `player.command` | `on_player_command` | player snapshot, `root`, `arguments` |
| `plugin.storage.get_result` | `on_plugin_storage_get_result` | `request_id`, `key`, `value`, `version`, `failure` |
| `plugin.storage.cas_result` | `on_plugin_storage_cas_result` | `request_id`, `key`, `applied`, `version`, `failure` |
| `plugin.storage.delete_result` | `on_plugin_storage_delete_result` | `request_id`, `key`, `deleted`, `version`, `failure` |
| `inventory.menu.clicked` | `on_inventory_menu_clicked` | player snapshot, `menu_id`, `slot`, `click` |
| `inventory.storage_transaction.result` | `on_inventory_storage_transaction_result` | `request_id`, `committed` |
| `player.inventory_transaction_result` | `on_player_inventory_transaction_result` | `request_id`, `player_id`, `committed`, `failure` |
| `player.zone_entered` | `on_player_zone_entered` | player snapshot, `zone_id` |
| `player.zone_exited` | `on_player_zone_exited` | player snapshot, `zone_id` |
| `zone.command_result` | `on_zone_command_result` | `zone_id`, `accepted` |
| `player.teleport_result` | `on_player_teleport_result` | `request_id`, `player_id`, `x`, `y`, `z`, `committed`, `failure` |
| `player.online_result` | `on_player_online_result` | `request_id`, `players`, `truncated` |
| `villager.binding_result` | `on_villager_binding_result` | `request_id`, `binding_token`, `binding_expires_at_tick`, `failure` |
| `villager.goal_result` | `on_villager_goal_result` | `request_id`, `goal`, `accepted`, `failure`, optional `x`, `y`, `z`, `speed` |

A gameplay-event player snapshot contains `player_id`, `uuid`, `username`,
`operator`, `x`, `y`, and `z`, captured by the server at publication. The
online-query entry described below additionally contains `context_verified` and
`dimension`. Neither shape contains a session, peer address, entity reference,
or live query handle. An absent storage record has `value = nil` and `version =
nil`. An unsuccessful villager binding has `binding_token = nil` and
`binding_expires_at_tick = nil`.

For block events, the immutable player pose is exposed as `player_x`,
`player_y`, and `player_z`; `x`, `y`, and `z` remain the integer block
coordinates. The other player snapshot fields are unchanged.

`player.block_broken` and `player.block_placed` are each published once after
the authoritative root block transition commits. `dimension` and `block_id`
are namespaced resource ids; `x`, `y`, and `z` are integer root-block
coordinates; `game_mode` is `survival` or `creative`. Placement reports the
actual final registry-backed root state. Door halves and stair-neighbour edits
do not create extra events. Bonemeal, hoe, bucket, cauldron, toggle, and plant
harvest interactions are not block-placement events.

`player.item_crafted` is published after the authoritative player-inventory or
crafting-table commit. `item_id` and `dimension` are namespaced resource ids;
`count` is the total output count and `craft_count` is the number of recipe
applications represented by the event. Recipe-book max crafting publishes one
aggregate event. `source` is `inventory` for the 2x2 player grid and
`crafting_table` for the 3x3 table; `game_mode` is `survival`, `creative`, or
`adventure`. Preview refreshes, drag distribution, cursor mismatch, missing
ingredients, full output inventory, no-op clicks, and rejected owner
preconditions publish nothing. Window-0 may accept a lagging client `state_id`
when the asserted cursor and current owner precondition still match; that is a
real committed craft and does publish the event.

`player.item_picked_up` is published only after the simulation owner has
atomically claimed the entity and credited the player inventory. `count` is the
exact credited amount, including a partial world-stack pickup. `source` is
`item_entity` for a world item or `arrow` for a grounded arrow; `game_mode` is
`survival`, `creative`, or `adventure`. XP orbs, crafting, container transfers,
and plugin inventory transactions are separate operations and never publish
this event. Full inventory, pickup delay, owner block, stale or concurrent
claims, invalid selected slots, dead players, and spectators publish nothing.
Item pickup readiness is indexed by its exact simulation tick and pushes a
candidate notification to nearby sessions even after the item has stopped
moving; it does not depend on a polling loop or guessed elapsed time. Hidden
campfire outputs enter this index only after their world-journal acknowledgement
and entity publication, so an aborted output commit cannot publish a pickup or
duplicate the item.

`player.entity_killed` is published once after a direct player-melee attack
commits the target's lethal entity transition and the attacker's survival and
inventory costs. `entity_id` is the server entity id from that committed
target, `entity_type` and `dimension` are namespaced resource ids, and `source`
is currently `melee`. Nonlethal or hurt-resistant attacks, stale attacker
costs, spectators, unreachable or missing targets, repeated attacks against
the already-dying entity, arrows, explosions, environmental damage, and
non-player damage publish nothing. Projectile attribution can extend this
event only when its owner carries an exact player identity through the lethal
commit; plugins must not infer it from nearby players or timing.

`player.entity_interacted` represents an accepted right-click gesture, not a
claim that a vanilla side effect occurred. The session owner accepts only a
reachable, alive, server-owned living entity for a live non-Spectator player;
`entity_id`, `entity_type`, player pose, dimension, game mode, hand, and
`secondary_action` come from that accepted snapshot. `hand` is `main_hand` or
`off_hand`; `game_mode` is `survival`, `creative`, or `adventure`. Missing,
nonliving, dying, dead, unreachable, or non-finite interactions publish
nothing. The normal feed, shear, or unsupported-interaction path completes
first, including fallible inventory writes; only then may required Luau queue
admission wait for capacity. Queue closure cannot roll back or reject the
already completed vanilla path. Plugins may use this event to open an NPC menu
or start a dialogue, but must not infer feeding, shearing, trading, or another
vanilla mutation from the gesture alone.

`player.died` is published once after the simulation owner accepts a live-to-
dead player survival transition, including the same atomic inventory drop and
XP reset. The common fall, contact block, starvation, hostile, projectile, PvP,
and operator damage paths use that transition. The owner snapshots the event
into the shared committed-gameplay push outbox before any fallible client
write. One async worker forwards immutable death and direct-melee-kill events
into the bounded Luau queue, so victim disconnects, stale connection mirrors,
and packet-write failures cannot erase or rewrite an accepted death. Nonlethal
or shield-blocked damage, stale owner state,
unsupported Creative/Spectator damage, repeated damage against an already-dead
player, and respawn publish nothing. The first contract deliberately omits
killer and damage-source fields because those facts are not yet carried
consistently through every death source; plugins must not infer them from timing
or nearby entities.

An aborted break, stale precondition, rejected mutation, repeated break of air,
blocked placement, or empty-hand placement publishes nothing. Required
gameplay-event delivery waits for an exact bounded-queue capacity notification,
so an admitted event keeps FIFO order without polling or guessed time. Closing
the plugin queue cannot roll back an already committed world mutation;
publication reports failure and the normal block result still reaches the
client. Subscribed `server.tick` telemetry remains nonblocking and can be
coalesced under pressure. The latest monotonic tick is retained for host timer
progress, but intermediate tick callbacks are not guaranteed. The
committed-gameplay FIFO guarantee applies inside that outbox; concurrent tick
events and player-command producers do not form a global causal order with it.
Do not use `server.tick` as a completion fence for a committed gameplay event.

## Simulation Timers

Plugins schedule one-shot host-local callbacks in simulation ticks:

```luau
local scheduled_tick = solaris.schedule_timer("catalog-refresh", 20)
local removed = solaris.cancel_timer("catalog-refresh")

function on_plugin_timer(event)
    assert(event.name == "plugin.timer")
    assert(event.fired_tick >= event.scheduled_tick)
end
```

`timer_id` uses the normal lowercase script-id grammar and is at most 64 bytes.
`delay_ticks` must be an integer from 1 through 630,720,000. Each plugin may
retain at most 256 pending timers. Scheduling an existing id replaces its
deadline without consuming another slot; cancellation returns `true` only when
that id was pending. Timer changes are staged with the current Luau handler and
commit only when it returns successfully.

`on_plugin_timer` is host-local and does not require `plugin.timer` or
`server.tick` in the manifest event list. A plugin receives at most eight due
timer callbacks for each pushed simulation tick. Due timers are ordered by
scheduled tick and then timer id; an earlier callback may cancel a later timer
that is due on the same tick. Remaining due timers stay pending until the next
pushed tick. All timer callbacks and an optional subscribed `on_server_tick`
handler share one 100,000-instruction budget and one 32-command batch for that
input tick.

Timers use the monotonic simulation tick, never wall-clock time, polling, or a
guessed delay. Queue pressure can make a callback late but cannot make it early:
`fired_tick >= scheduled_tick`. Timers are in memory only and disappear on
server restart or plugin disable. A successful handler commits timer changes
before its outbound command batch is routed; later command-queue rejection does
not roll those timer changes back.

## Commands

The existing bounded presentation commands remain available:

```luau
solaris.send_message(player_id, text)
solaris.broadcast(text)
solaris.disconnect(player_id, reason)
solaris.run_console(command)
solaris.spawn_entity(player_id, entity_type, x, y, z)
solaris.place_loader_block(block_id, x, y, z)
solaris.grant_loader_block_item(player_id, block_id, count)
```

Plugins with `player_queries` may request one bounded point-in-time snapshot:

```luau
solaris.list_online_players("catalog-viewers", 64)
```

The optional limit defaults to 256 and must be between 1 and 256. The targeted
`player.online_result` contains a one-based `players` array sorted by
`player_id`; each entry has `player_id`, `context_verified`, `uuid`, `username`,
`operator`, `x`, `y`, `z`, and `dimension`. `truncated` is true when more live
sessions existed than fit the requested limit. Sessions whose outbound owner is
already closed are excluded. The values are immutable snapshots, not handles;
plugins must issue another query when they need a newer view.

Storage is scoped by the host-attached plugin identity. Luau does not pass a
plugin id and cannot forge one:

```luau
solaris.storage_get(request_id, key)
solaris.storage_cas(request_id, key, expected_version, value)
solaris.storage_delete(request_id, key, expected_version)
```

`request_id` is a lowercase ASCII id up to 64 bytes. A key is a non-empty
string up to 128 bytes; a value is a non-empty string up to 4096 bytes.
`expected_version` is a storage version returned by `storage_get`; `nil` means
the record must be absent. The storage adapter emits exactly one targeted result
after a committed read or mutation, and owns conflict outcomes and revision
allocation. Reads carry either both `value` and `version` or neither; successful
compare-and-swap and delete results carry a version. `failure` is `nil` for a
normal absent record, stale precondition, or durable success. It is
`"unavailable"` when the server has no persistent world and
`"durability_failed"` after the storage actor encounters a definite pre-append
write failure. Failure results carry no value/version and report mutations as
not applied. A synchronization error after a complete append has an unknown
durability outcome: the actor fail-stops without claiming that request failed,
and startup resolves the CRC-valid transaction frame and its durable result
outbox.

Storage is durable below `world/solaris/plugin-storage-v1`, isolated by the
host-attached plugin id, and has no legacy schema. The single storage actor has
a 256-command queue; it permits at most 4,096 live records per plugin, 64 MiB
of live values total, and a 128 MiB CRC-framed journal. Each successful
standalone mutation frame contains the admitted plugin id, request id, request
fingerprint, transaction revision, state transition, and targeted result
identity. The frame is appended and `sync_all`ed before memory changes. Result
publication is then followed by a separately synced delivery-ack frame. Until
that ack is replayed, the standalone result remains in the durable outbox and
is delivered again on startup. Reusing the same plugin/request identity with
identical content reuses the original transaction and version without repeating
the mutation; substituted content is rejected.
Malformed, oversized, and checksum-invalid journals fail closed. An incomplete
final frame is truncated only back to the verified frame prefix and synced;
compaction writes a synced temporary journal, renames it atomically, then syncs
the parent directory.

With a persistent world configured, malformed journal data or plugin-storage
startup I/O fails the server bind with the typed storage startup error; Luau is
not left live with storage silently disabled. Without a persistent world,
non-storage plugin behavior remains available and every admitted storage request
receives the targeted `unavailable` result. A definite durability failure closes
the actor command receiver, then consumes the failed request and every command
already queued behind it in FIFO order into one awaited targeted failure result
each. For an unknown post-append sync outcome, the current admitted ticket is
consumed into the durable transaction identity for startup replay; queued
requests are consumed into explicit failure results. Later submissions either
receive the same explicit failure after that drain or stop command orchestration
if their queue is closed during shutdown.

The inventory adapter owns menus after admission. Plugins describe fixed
display slots but do not receive container, slot-stack, NBT, or click-packet
state:

```luau
solaris.open_inventory_menu(player_id, menu_id, title, {
    { slot = 0, resource = "minecraft:apple", count = 1, label = "Apple" },
})
solaris.close_inventory_menu(player_id, menu_id)
```

Menu ids use the same 64-byte id rule, titles and labels are at most 128 bytes,
and a menu has at most 54 unique slots. `click` is one of `primary`, `secondary`,
`shift_primary`, or `shift_secondary`. The connected player's ordered reliable
session lane carries open and close commands. The active window rejects stale
state, empty/player-inventory slots, unsupported click modes, and forged
container ids with an authoritative content resync; focused classifier tests
cover those reject branches. Accepted fixed-slot clicks publish
`inventory.menu.clicked` only to the plugin that opened the menu. A wire test
covers Luau admission, exact title/item/count content, stale-state rejection, a
normal predicted client click, targeted Luau delivery, a second subscribed
plugin proving non-delivery, and the owning plugin response.

The transaction adapter treats each inventory/storage request as one runtime
commit. Positive inventory `delta` grants a resource and negative `delta`
removes one; a delta cannot be zero or exceed 64 in magnitude. Each side must
be non-empty and have at most 16 unique resources or storage keys. Only main
inventory and hotbar slots participate. Unknown resources, insufficient items,
full output inventory, a disconnected player, stale storage versions, and
storage quota failures reject the whole request without changing either side.

```luau
solaris.inventory_storage_transaction(player_id, request_id,
    { { resource = "minecraft:apple", delta = 1 } },
    { { operation = "cas", key = "coins:player", expected_version = 4, value = "7" } }
)
```

Storage mutations use `operation = "cas"` or `operation = "delete"`; both use
the same expected-version semantics as the standalone commands. The storage
actor prepares every key first, holds the canonical player-state lock while it
appends and syncs one CRC-framed storage batch, then replaces the inventory and
publishes one ordered reliable authoritative snapshot. Concurrent inventory
operations therefore cannot observe or interleave half of a successful runtime
transaction. A per-session lifetime gate makes disconnect either reject a
captured-but-not-started request or wait for an already-started commit before
the disconnected player state becomes saveable. Every storage record changed
by the batch receives the same revision.

The storage journal and vanilla playerdata are not yet one crash-recovery log.
If the process dies, or `sync_all` returns an unknown outcome after a complete
batch append, startup can replay the storage half while the last playerdata save
still contains the old inventory. The actor fail-stops on that uncertainty and
does not publish a guessed result. This is a documented crash-atomicity gap, not
a runtime atomicity claim; closing it requires a player-inventory recovery intent
in the durable transaction record.

The separate player-inventory API performs one atomic main-inventory and
hotbar mutation without touching plugin storage:

```luau
solaris.inventory_transaction(player_id, request_id, {
    { resource = "minecraft:emerald", delta = -2 },
    { resource = "minecraft:apple", delta = 4 },
})
```

The delta list must contain 1 to 16 unique resources. A positive delta grants
the item and a negative delta removes it; zero and magnitudes above 64 are
rejected at the Luau boundary. Only slots 9 through 44 participate. The session
owner resolves and plans every delta against one canonical player-state
snapshot before replacing the inventory, so unknown resources, insufficient
input, and a full output inventory cannot leave a partial mutation. A successful
commit publishes one authoritative inventory snapshot on the player's ordered
session lane.

`player.inventory_transaction_result` is targeted to the issuing plugin and
must be correlated by `request_id`. Success sets `committed = true` and
`failure = nil`. Rejections use `player_unavailable`, `runtime_unavailable`,
`unknown_resource`, `insufficient_resource`, or `inventory_full`. The exact
session-lifetime gate orders commit against disconnect. A server without a
world runtime returns `runtime_unavailable` before entering the session commit.
This API is independent of durable plugin storage; it does not claim a joint
crash transaction with plugin records.

Zones are axis-aligned definitions, scoped by the host-attached plugin id:

```luau
solaris.upsert_zone("catalog-square", "minecraft:overworld", -8, 60, -8, 8, 100, 8)
solaris.remove_zone("catalog-square")
```

All six coordinates must be finite, within the existing script coordinate
limits, and ordered minimum-to-maximum on every axis. The zone adapter owns
membership tracking and publishes `player.zone_entered` and
`player.zone_exited` only to the plugin that owns the zone. It observes the
initial player pose and every accepted absolute movement. Each event carries
the authoritative pose after that movement. A mixed transition publishes all
exits before entries, with each group ordered by plugin id and zone id.
Rejected, stale, and membership-preserving movement publishes nothing. Zone
removal and disconnect are silent cleanup, not player movement events. Changing
a zone keeps an existing membership when the player remains inside, so an edit
cannot repeat entry side effects.

Every admitted `upsert_zone` or `remove_zone` publishes one targeted
`zone.command_result`. `accepted = true` includes an idempotent no-op;
`accepted = false` means the registry did not apply the command. A plugin must
not announce protection before receiving the accepted result.

The process admits at most 4,096 zones, 256 zones per plugin, 16,384 tracked
players, and 262,144 memberships. A request beyond a bound is rejected without
partial mutation and logged by the production router. These bounds are server
admission limits, not operator-configured worker percentages.

## Ownership Routing

Luau commands never contain region keys, leases, epochs, locks, sockets, or
worker handles. The server resolves ownership after admitting the bounded DTO:

- entity spawn enters the simulation owner;
- villager binding and orders enter the current regional entity owner;
- menus, teleports, and standalone player-inventory transactions enter the
  target player's ordered session lane;
- plugin storage enters its serial durable actor.

A standalone `player_inventory_transaction` completes only after the exact
session owner plans against its live inventory and updates the durable player
mirror. A missing or dropped owner command returns `player_unavailable` without
mutation; an unavailable world runtime returns `runtime_unavailable`. Standalone
owner commands and compound inventory/storage transactions share one internal
session gate, so compound planning cannot overtake an earlier owner command. The
compound `inventory_storage_transaction` remains a separate typed coordinator
with an internal player-lifetime fence because its durable storage mutation and
inventory mutation must never publish separately. Neither path exposes its
coordination mechanism to Luau. There is no generic mutable-world transaction or
coroutine suspension API.

## Shipped Economy And Claims

`examples/plugins/basic-economy` uses one configurable physical item, such as
emeralds or gold ingots, as currency. Entering its configured cuboid opens a
server-owned inventory shop; `/economy` opens the same shop manually. A primary
click removes currency, grants the product, and advances the durable refund
ledger in one `inventory_storage_transaction`. A secondary click atomically
refunds only purchases recorded by this shop. Insufficient currency, a full
inventory, stale storage, or a concurrent purchase rejects the whole mutation.
`config.toml` documents the currency item and labels, zone, and products beside
the values an operator edits. Player-to-player payment, auctions, and multiple
simultaneous currencies are intentionally outside this basic plugin.

`examples/plugins/land-claims` provides `/claim status`, `/claim create`, and
`/claim remove`. Claims cover one whole chunk in the configured dimension and
vertical range, persist in one versioned storage record, and allow removal by
the owner or an operator. API `0.6.0` player command snapshots do not expose a
dimension, so every command maps to the configured dimension; the shipped
plugin is restricted to the current single-dimension runtime.

Protection is a generic `zones` capability, not knowledge of this shipped
plugin in the Rust server. Any plugin may register an actor-or-operator policy:

```luau
solaris.upsert_protected_zone(
    "home", "minecraft:overworld", owner_uuid,
    min_x, min_y, min_z, max_x, max_y, max_z
)
```

The zone id is opaque and scoped to the calling plugin. `mc-script` validates
and normalizes the allowed actor UUID into the typed zone DTO; `mc-net`
evaluates only that policy. It never matches a plugin id or parses a zone id. Ordinary
`solaris.upsert_zone(...)` zones remain membership-only. The claims plugin
decides which chunks exist, who owns them, how they persist, and when their
policies are inserted or removed. It waits for the targeted zone result before
reporting success and rolls its storage CAS back when registration fails.

Protection covers direct break/place, right-click block interactions including
containers and buckets, living-entity interaction at the target position, and
explosion block damage. Player actions use the authoritative actor check, and
every chest/furnace click rechecks all backing block positions so a policy
created after opening still denies mutation. Explosion planning takes one
immutable generic zone-protection snapshot after claiming due explosions and
before the world lock; it does not copy zones on idle ticks or lock the registry
per candidate block. Random fire ticks use the same immutable snapshot before
planning one bounded adjacent burn into common fuel; protected targets are not
mutated and no zone lock enters the random-tick candidate loop. This is the
baseline mutation/protection path, not the complete vanilla fire material and
odds table. Direct lever/button power can extend or retract one normal piston
and move one common propertyless full block. Its base/head/destination edits are
one atomic group and consume the ambient protection snapshot in both direct
interaction and scheduled button-release planning; one protected position
rejects the whole group. Sticky pistons, multi-block chains, slime/honey, and
moving-block animation are not part of this baseline.

Player teleports are same-dimension authoritative mutations:

```luau
solaris.teleport_player("warp-home", player_id, 40, 70, 1)
```

The request id follows the 64-byte script-id rule. Coordinates must be finite
and within the existing script coordinate limits. The API deliberately has no
dimension argument; cross-dimension transfer is not part of API `0.6.0`.

The router sends the request through the connected player's reliable session
lane. A pending vanilla position confirmation rejects the request without
mutation as `teleport_pending`. A missing, disconnected, cancelled-before-
commit, or stale session returns `player_unavailable`. A closed or failed
simulation owner returns `runtime_unavailable`.

Success means the simulation owner committed the exact pose. It does not mean
the client confirmed the teleport, received every destination chunk, or
completed a socket write. After commit, the connection coordinator clears
active and delayed breaking, pending item use, and shield use; installs a new
pending teleport id; sends the position synchronization packet; replans the
chunk center; and observes zone membership at the committed pose. Cancellation
after owner commit cannot turn the targeted result into a failure.

`player.teleport_result` is delivered only to the plugin that issued the
request. It echoes the exact request/player/coordinates, sets `committed` from
the owner outcome, and uses `failure = nil` on success. Zone transition and
teleport-result events come from separate producers and have no relative-order
guarantee; plugins must correlate the teleport result by `request_id` instead
of using a zone event as its completion fence.

Villager control is an engine primitive, not a Rust-owned colony model:

```luau
solaris.bind_nearest_villager("bind-player-7", 0, 64, 0, 16)
solaris.move_villager_to("send-home", binding_token, 0, 64, 0, 0.3)
solaris.set_villager_idle("hold-position", binding_token)
```

Request and binding ids follow the 64-byte script-id rule. A binding search
radius must be finite, positive, and no greater than 64. A movement target must
use finite bounded coordinates and a finite speed in `(0, 4]`. The result token
is ephemeral; it is not an entity id, pointer, durable capability, region key,
or ECS reference.

`bind_nearest_villager` asks the regional entity owner to atomically claim the
nearest alive exact `minecraft:villager` inside the radius. No session snapshot
scan is used. A successful claim returns a random 128-bit lowercase hexadecimal
token and its simulation-tick expiry. The targeted result uses `failure =
"not_found"` when no eligible villager exists and `failure = "busy"` for
transient owner/capacity pressure. A closed or failed owner or result-queue
closure stops the router instead of fabricating delivery. A claim committed
before publication failure remains reserved until its normal simulation-tick
expiry.

The adapter retains only the mapping from each token to its host-attested plugin
owner and exact simulation-tick expiry. It contains no colony id, home, role,
order, settlement record, or other domain state. Expired entries are purged from
the pushed simulation tick; no wall-clock timer or polling loop is involved. A
foreign plugin receives `failure = "binding_unavailable"` and cannot consume or
invalidate the owner's token.

`move_villager_to` installs a validated follow-position goal and
`set_villager_idle` installs the idle goal through the journaled regional entity
owner. Missing, expired, removed, non-villager, or otherwise stale bindings
return `failure = "binding_unavailable"`; temporary owner pressure returns
`failure = "busy"` while retaining the unexpired token. If result publication
closes after the owner commits the goal, the committed goal remains in effect
and the router stops instead of pretending the mutation was rejected.

The shipped colony scaffold owns all colony vocabulary in Luau. Its
`config.toml` defines colony identity, display name, dimension, home, zone,
roles, accepted orders, limits, and home speed. Plugin storage owns the durable
colony metadata and per-player role/order intent. Rust receives only the generic
zone plus villager binding/goal requests. The plugin maps its `home` order to
`move_villager_to` and `hold` to `set_villager_idle`, retains an accepted token
only in Luau memory, retries one typed transient/stale failure, and clears its
session state on disconnect. Durable entity handles, pathing internals, villager
inventory/memory access, and complete colony gameplay remain outside API
`0.6.0`.

## Isolation And Limits

Each plugin has one Luau VM on the dedicated host thread with a 16 MiB memory
limit, 100,000 instructions per load or handler, and at most 32 commands per
event. Event and command queues are bounded and nonblocking. One invocation's
command batch enters the host queue atomically or not at all. On queue
saturation or closure, the host calls `on_command_batch_rejected(result)`
directly with `reason = "queue_full"` or `reason = "queue_closed"` and the
exact `command_count`; that callback cannot emit another command. A failed
handler disables only that plugin.

Shutdown publishes `server.stopping` before closing event admission. Calls that
start after that fence receive `ScriptQueueError::Closed`; events admitted
before it remain in the bounded queue. The host drains those events, then drops
its command producer. The server drains commands until that producer closes,
so commands emitted by the accepted stopping event are not lost. A shutdown
timeout may report a stuck host as failure, but elapsed time is never treated as
successful drain evidence.

The host wraps every Luau-emitted command in a bounded, one-shot admission ticket
before it crosses the script boundary. A production router must use this exact
sequence:

1. Receive the raw command with `ScriptBoundary::recv_command`.
2. For `HostAttached`, immediately consume it with
   `ScriptBoundary::accept_host_command` before any side effect.
3. Route only the returned `AdmittedScriptCommand`; rejection means no mutation
   and no result publication.
4. Build storage, transaction, colony, and binding results with the matching
   consuming method on `AdmittedScriptCommand`.
5. For an accepted owning `OpenInventoryMenu` or `UpsertZone`, use
   `into_open_inventory_menu` or `into_upsert_zone` and retain the returned
   `ScriptPluginTarget` for later click or entry events.

A ticket records the exact plugin and exact request. A cloned ticket can be
accepted once, request substitution consumes and rejects it, and public code
cannot construct an arbitrary targeted result. Directly matching and trusting
the fields of `HostAttached` is not an adapter API. Luau exposes no filesystem,
network, process, debug, paths, locks, NBT, sessions, or entity pointers.

See [the contract examples](../examples/plugins/) for the configurable
item-currency economy, land claims, `/who` inventory roster, and the
intentionally limited colony/villager scaffold.

`crates/mc-test-harness/tests/plugin_examples.rs` copies those exact shipped
files into an isolated plugin directory and runs them through the production
Luau host, server router, storage actor, regional owner, and wire client. The
catalog gate proves zone entry, menu contents, atomic purchase, insufficient
funds rejection, unchanged ledger, and refund. The same wire client invokes the
shipped `/who` command and proves that a fresh authoritative online-player
result becomes a server-owned inventory menu with the connected player's name
and dimension. A focused Luau test proves that command-batch rejection releases
the requester's pending slot and that the longest valid dimension cannot exceed
the menu-label bound. The colony gate proves command
registration, durable recruitment, initial `home`, a later accepted `hold`, and
the resulting durable status. It then removes the bound villager and proves
that rejected cached-token application causes one fresh bind and an explicit
no-villager result. Plugin-emitted readiness messages causally fence startup;
timeouts only fail missing packets. These are integration checks of the
examples; they are not vanilla-oracle or broad plugin-ecosystem readiness
evidence.

The same suite routes the exact economy and claim Luau files through the real
host. It proves zone and command entry, atomic item-currency purchase and
refund, durable claim CAS and zone registration, then uses two real wire
clients to prove a stranger cannot break or place inside the owner's claimed
chunk.
