# Lua Plugins

Solaris exposes one Lua plugin contract: API `0.6.0`. A manifest requesting any
other version is rejected. There is no legacy manifest or Lua API compatibility
path.

`mc-net` provides the `0.6.0` plugin-storage, zone, inventory-menu,
inventory/storage transaction, player-inventory transaction, player-teleport,
connected-player query, colony-record, villager-binding, and bounded
villager-order adapters. It also
publishes committed player block breaks and owner-targeted zone membership
transitions.

## Package And Manifest

The configured plugin directory contains one directory per plugin:

```text
plugins/
`-- currency-catalog/
    |-- config.toml     # optional operator configuration
    |-- plugin.toml
    `-- main.lua
```

```toml
id = "currency-catalog"
name = "Currency Catalog"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "player.zone_entered", "player.zone_exited", "inventory.menu.clicked"]
capabilities = ["storage", "inventory_menus", "inventory_storage_transactions", "zones"]
player_commands = ["catalog"]
operator_commands = ["catalogadmin"]
console_commands = ["time"]
spawn_entities = ["minecraft:pig"]
dependencies = [{ id = "economy", relation = "required" }]
permissions = ["solaris:catalog.read"]
```

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
empty table. `solaris.config()` returns a new recursive Lua table on every
call, so a plugin may mutate its local copy without changing later reads. Disk
changes after startup do not change the loaded snapshot; live reload,
environment interpolation, default merging, and cross-plugin reads are not
part of API `0.6.0`.

```lua
local config = solaris.config()
assert(config.currency.resource == "minecraft:emerald")
assert(config.catalog[1].price == 3)
```

Accepted TOML values are strings, signed 64-bit integers, finite floats,
booleans, arrays, and tables. Arrays become one-based Lua tables. TOML dates
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
| `zones` | `upsert_zone`, `remove_zone`, owned zone entry/exit events |
| `colonies` | `upsert_colony`, `bind_nearest_villager`, `set_villager_order` |
| `player_teleport` | `teleport_player` |
| `player_queries` | `list_online_players` |

An undeclared privileged call fails synchronously in Lua before it enters the
bounded command batch. Unknown capabilities reject the plugin during discovery.

## Events

`events` subscribes to broadcast events. All event values are immutable DTO
snapshots. `player.command` and every result/owner event below are targeted: the
host routes them to exactly the owning plugin and never broadcasts them. A
targeted event does not need a broad subscription to reach its owner.

| Event | Lua handler | Fields |
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
| `player.teleport_result` | `on_player_teleport_result` | `request_id`, `player_id`, `x`, `y`, `z`, `committed`, `failure` |
| `player.online_result` | `on_player_online_result` | `request_id`, `players`, `truncated` |
| `colony.record_result` | `on_colony_record_result` | `request_id`, `colony_id`, `accepted` |
| `colony.villager_binding_result` | `on_colony_villager_binding_result` | `request_id`, `colony_id`, `binding_token`, `binding_expires_at_tick` |
| `colony.villager_order_result` | `on_colony_villager_order_result` | `request_id`, `colony_id`, `order`, `accepted` |

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
first, including fallible inventory writes; only then may required Lua queue
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
into the bounded Lua queue, so victim disconnects, stale connection mirrors,
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

```lua
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
that id was pending. Timer changes are staged with the current Lua handler and
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

```lua
solaris.send_message(player_id, text)
solaris.broadcast(text)
solaris.disconnect(player_id, reason)
solaris.run_console(command)
solaris.spawn_entity(player_id, entity_type, x, y, z)
```

Plugins with `player_queries` may request one bounded point-in-time snapshot:

```lua
solaris.list_online_players("catalog-viewers", 64)
```

The optional limit defaults to 256 and must be between 1 and 256. The targeted
`player.online_result` contains a one-based `players` array sorted by
`player_id`; each entry has `player_id`, `context_verified`, `uuid`, `username`,
`operator`, `x`, `y`, `z`, and `dimension`. `truncated` is true when more live
sessions existed than fit the requested limit. Sessions whose outbound owner is
already closed are excluded. The values are immutable snapshots, not handles;
plugins must issue another query when they need a newer view.

Storage is scoped by the host-attached plugin identity. Lua does not pass a
plugin id and cannot forge one:

```lua
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
startup I/O fails the server bind with the typed storage startup error; Lua is
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

```lua
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
covers Lua admission, exact title/item/count content, stale-state rejection, a
normal predicted client click, targeted Lua delivery, a second subscribed
plugin proving non-delivery, and the owning plugin response.

The transaction adapter treats each inventory/storage request as one runtime
commit. Positive inventory `delta` grants a resource and negative `delta`
removes one; a delta cannot be zero or exceed 64 in magnitude. Each side must
be non-empty and have at most 16 unique resources or storage keys. Only main
inventory and hotbar slots participate. Unknown resources, insufficient items,
full output inventory, a disconnected player, stale storage versions, and
storage quota failures reject the whole request without changing either side.

```lua
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

```lua
solaris.inventory_transaction(player_id, request_id, {
    { resource = "minecraft:emerald", delta = -2 },
    { resource = "minecraft:apple", delta = 4 },
})
```

The delta list must contain 1 to 16 unique resources. A positive delta grants
the item and a negative delta removes it; zero and magnitudes above 64 are
rejected at the Lua boundary. Only slots 9 through 44 participate. The session
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

```lua
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

The process admits at most 4,096 zones, 256 zones per plugin, 16,384 tracked
players, and 262,144 memberships. A request beyond a bound is rejected without
partial mutation and logged by the production router. These bounds are server
admission limits, not operator-configured worker percentages.

Player teleports are same-dimension authoritative mutations:

```lua
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

Colonies are bounded records, not world/entity access:

```lua
solaris.upsert_colony("register-colony", "starter-colony", "Starter Colony",
    "minecraft:overworld", 0, 64, 0)
solaris.bind_nearest_villager("bind-player-7", "starter-colony", 0, 64, 0, 16)
solaris.set_villager_order("send-home", "starter-colony", binding_token, "home")
```

Colony ids follow the 64-byte id rule and names are at most 128 bytes. A binding
search radius must be finite, positive, and no greater than 64. The result token
is ephemeral; it is not an entity id, pointer, or durable villager capability.
`set_villager_order` accepts only `home` and `hold`. `home` uses the current
owned colony home and `hold` stops horizontal goal movement. Plugins cannot
choose arbitrary coordinates or speeds. There is deliberately no Lua API for
roles, general goals, pathing internals, memory, inventory, or direct entity
mutation.

Colony records are scoped by the host-attached plugin id and kept in a bounded
in-memory registry. The process admits at most 4,096 records and 256 records per
plugin. Replacing an owned record remains possible at capacity. A new record
beyond either bound returns `colony.record_result` with `accepted = false` and
does not mutate the registry. While event publication remains open, every
admitted upsert returns its correlated result only to the owning plugin; queue
closure or shutdown stops the router instead of fabricating delivery. The
registry is not durable, so plugins that need restart continuity must persist
their intent through plugin storage.

`bind_nearest_villager` requires a colony record owned by the admitted plugin.
The current single-world adapter accepts only `minecraft:overworld` colonies;
other dimensions return an unsuccessful targeted result without querying or
mutating entity ownership. Eligible requests run on a blocking endpoint outside
the async router worker, then ask the regional entity owner for an atomic claim.
No session snapshot scan is used. A successful claim returns a random 128-bit
lowercase hexadecimal token and its simulation-tick expiry. An absent villager
returns an unsuccessful targeted result. Invalid coordinates, a random token
collision, transient owner busy state, or global claim-capacity exhaustion also
return an unsuccessful result without shutting down the server. A closed or
failed owner, token generation failure, or result-queue closure stops the router
instead of fabricating delivery. A claim committed before publication failure
or forced task cancellation remains reserved until its normal simulation-tick
expiry; normal cooperative shutdown drains the active route.

The colony adapter retains a bounded mapping from each binding token to its
owning plugin, colony, and exact simulation-tick expiry. Expired entries are
purged from the current simulation tick; no wall-clock timer or polling loop is
involved. A foreign plugin receives `accepted = false` and cannot consume or
invalidate the owner's token. An owned `home` order resolves the colony's
current home and installs a server-owned follow-position goal at speed `0.3`;
`hold` installs the idle goal. Both mutations run through the blocking endpoint
and the journaled regional entity owner. Missing, expired, removed, non-villager,
or otherwise stale bindings return `accepted = false`. A broken owner or journal
stops routing. If result publication closes after the owner commits the goal,
the committed goal remains in effect; the router stops instead of pretending the
mutation was rejected. Temporary owner pressure also returns `accepted = false`,
but retains the unexpired token so the plugin may retry. Changing the colony to
another dimension rejects the order before owner mutation.

The shipped colony scaffold retains an accepted token only in Lua memory for
the active player session and reuses it for later `home` or `hold` updates.
It reports `Applied villager order ...` only after the targeted owner result is
accepted. If a cached token is rejected, the scaffold clears it and performs
one fresh binding attempt; a result from that attempt is never recursively
retried. Plugin storage records the bounded role and order intent, not the
ephemeral token or a fabricated entity-mutation result; `/colony status` labels
that field as stored intent. Disconnect cleanup drops the Lua-side token, while
the regional owner keeps its claim only until the documented simulation-tick
expiry. Durable entity handles, general roles, and work-order execution remain
outside API `0.6.0`.

## Isolation And Limits

Each plugin has one Lua VM on the dedicated host thread with a 16 MiB memory
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

The host wraps every Lua-emitted command in a bounded, one-shot admission ticket
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
the fields of `HostAttached` is not an adapter API. Lua exposes no filesystem,
network, process, debug, paths, locks, NBT, sessions, or entity pointers.

See [the contract examples](../examples/plugins/) for the configurable currency
catalog and the intentionally limited colony/villager scaffold.

`crates/mc-test-harness/tests/plugin_examples.rs` copies those exact shipped
files into an isolated plugin directory and runs them through the production
Lua host, server router, storage actor, regional owner, and wire client. The
catalog gate proves zone entry, menu contents, atomic purchase, insufficient
funds rejection, unchanged ledger, and refund. The colony gate proves command
registration, durable recruitment, initial `home`, a later accepted `hold`, and
the resulting durable status. It then removes the bound villager and proves
that rejected cached-token application causes one fresh bind and an explicit
no-villager result. Plugin-emitted readiness messages causally fence startup;
timeouts only fail missing packets. These are integration checks of the
examples; they are not vanilla-oracle or broad plugin-ecosystem readiness
evidence.
