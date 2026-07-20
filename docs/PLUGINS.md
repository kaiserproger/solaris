# Lua Plugins

Solaris exposes one Lua plugin contract: API `0.6.0`. A manifest requesting any
other version is rejected. There is no legacy manifest or Lua API compatibility
path.

`mc-net` provides the `0.6.0` plugin-storage, zone, inventory-menu, and
inventory/storage transaction adapters. Colony and villager adapters do not
exist yet. Their example remains fail-closed.

## Package And Manifest

The configured plugin directory contains one directory per plugin:

```text
plugins/
`-- currency-catalog/
    |-- plugin.toml
    `-- main.lua
```

```toml
id = "currency-catalog"
name = "Currency Catalog"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "player.zone_entered", "inventory.menu.clicked"]
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

`capabilities` is an exact, duplicate-free list:

| Capability | Allows |
| --- | --- |
| `storage` | `storage_get`, `storage_cas`, `storage_delete` |
| `inventory_menus` | `open_inventory_menu`, `close_inventory_menu` |
| `inventory_storage_transactions` | `inventory_storage_transaction` |
| `zones` | `upsert_zone`, `remove_zone`, owned zone-entry events |
| `colonies` | `upsert_colony`, `bind_nearest_villager` |

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
| `server.tick` | `on_server_tick` | `tick` |
| `player.command` | `on_player_command` | player snapshot, `root`, `arguments` |
| `plugin.storage.get_result` | `on_plugin_storage_get_result` | `request_id`, `key`, `value`, `version`, `failure` |
| `plugin.storage.cas_result` | `on_plugin_storage_cas_result` | `request_id`, `key`, `applied`, `version`, `failure` |
| `plugin.storage.delete_result` | `on_plugin_storage_delete_result` | `request_id`, `key`, `deleted`, `version`, `failure` |
| `inventory.menu.clicked` | `on_inventory_menu_clicked` | player snapshot, `menu_id`, `slot`, `click` |
| `inventory.storage_transaction.result` | `on_inventory_storage_transaction_result` | `request_id`, `committed` |
| `player.zone_entered` | `on_player_zone_entered` | player snapshot, `zone_id` |
| `colony.record_result` | `on_colony_record_result` | `request_id`, `colony_id`, `accepted` |
| `colony.villager_binding_result` | `on_colony_villager_binding_result` | `request_id`, `colony_id`, `binding_token`, `binding_expires_at_tick` |

A player snapshot contains only `player_id`, `uuid`, `username`, `operator`,
`x`, `y`, and `z`, captured by the server at publication. It contains no
session, peer address, entity reference, or live query handle. An absent storage
record has `value = nil` and `version = nil`. An unsuccessful villager binding
has `binding_token = nil` and `binding_expires_at_tick = nil`.

## Commands

The existing bounded presentation commands remain available:

```lua
solaris.send_message(player_id, text)
solaris.broadcast(text)
solaris.disconnect(player_id, reason)
solaris.run_console(command)
solaris.spawn_entity(player_id, entity_type, x, y, z)
```

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

Zones are axis-aligned definitions, scoped by the host-attached plugin id:

```lua
solaris.upsert_zone("catalog-square", "minecraft:overworld", -8, 60, -8, 8, 100, 8)
solaris.remove_zone("catalog-square")
```

All six coordinates must be finite, within the existing script coordinate
limits, and ordered minimum-to-maximum on every axis. The zone adapter owns
membership tracking and publishes `player.zone_entered` only to the plugin that
owns the zone. It observes the initial player pose and every accepted absolute
movement; rejected movement cannot create an entry. Disconnect removes the
player immediately. Changing a zone keeps an existing membership when the
player remains inside, so an edit cannot repeat entry side effects.

The process admits at most 4,096 zones, 256 zones per plugin, 16,384 tracked
players, and 262,144 memberships. A request beyond a bound is rejected without
partial mutation and logged by the production router. These bounds are server
admission limits, not operator-configured worker percentages.

Colonies are bounded records, not world/entity access:

```lua
solaris.upsert_colony("register-colony", "starter-colony", "Starter Colony",
    "minecraft:overworld", 0, 64, 0)
solaris.bind_nearest_villager("bind-player-7", "starter-colony", 0, 64, 0, 16)
```

Colony ids follow the 64-byte id rule and names are at most 128 bytes. A binding
search radius must be finite, positive, and no greater than 64. The result token
is ephemeral; it is not an entity id, pointer, or durable villager capability.
There is deliberately no Lua API for villager goals, pathing, memory, inventory,
or direct entity mutation.

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
