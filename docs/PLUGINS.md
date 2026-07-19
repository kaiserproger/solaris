# Lua Plugins

Solaris can load server-side Lua plugins from the directory configured in the
server TOML:

```toml
[plugins]
directory = "plugins"
```

The path is relative to the server process working directory. Omit the section
or `directory` to disable the plugin host.

## Package

Each plugin has its own directory:

```text
plugins/
`-- welcome/
    |-- plugin.toml
    `-- main.lua
```

`plugin.toml` declares the plugin identity, API version, event subscriptions,
player command roots, console command roots, and entity types it may spawn:

```toml
id = "welcome"
name = "Welcome"
version = "0.1.0"
api = "0.5.0"
events = ["player.joined", "player.chat"]
player_commands = ["hello"]
operator_commands = ["adminday"]
console_commands = ["time"]
spawn_entities = ["minecraft:pig"]
```

Plugin IDs must be unique. Invalid plugins are skipped without stopping the
server. `player_commands` entries must be non-empty lowercase ASCII literals
containing only `a-z`, `0-9`, `_`, or `-`. Duplicate entries in one manifest
are deduplicated. Declaring `player_commands` requires script API `0.3.0` or
newer. `operator_commands` uses the same root syntax but requires script API
`0.4.0`; it is visible and invokable only by server operators. A root cannot
appear in both lists. Older manifests, including API `0.3.0` manifests with
only `player_commands`, remain valid. A root is
limited to 64 ASCII bytes. At most 128 plugin roots, or 256 command-tree nodes,
may be active across the server. Registration is atomic: a plugin is skipped as
a whole if it would exceed a limit, if one of its roots is a Solaris built-in
command, or if an earlier plugin already claimed the root. Changes to plugin
files and command declarations take effect only after a server restart.

`spawn_entities` is an exact allow-list for `solaris.spawn_entity`. It requires
script API `0.5.0` or newer. Each entry is a unique, fully namespaced lowercase
resource identifier such as `minecraft:pig`, limited to 128 bytes. A plugin may
declare at most 32 entity types. Older `0.1.0` through `0.4.0` manifests remain
valid when `spawn_entities` is empty.

## Handlers

`main.lua` may define handlers for its subscribed events:

```lua
function on_player_joined(event)
    solaris.send_message(event.player_id, "Welcome " .. event.username)
end

function on_player_chat(event)
    if event.message == "day" then
        solaris.run_console("time set day")
    end
end

function on_player_command(event)
    solaris.send_message(
        event.player_id,
        "Hello " .. event.username .. "; arguments: " .. event.arguments
    )
end
```

| Event | Handler | Fields |
| --- | --- | --- |
| `server.started` | `on_server_started` | `name` |
| `server.stopping` | `on_server_stopping` | `name`, `reason` |
| `player.joined` | `on_player_joined` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z` |
| `player.left` | `on_player_left` | `name`, `player_id`, `reason` |
| `player.chat` | `on_player_chat` | `name`, `player_id`, `message`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z` |
| `player.command` | `on_player_command` | `name`, `player_id`, `context_verified`, `uuid`, `username`, `operator`, `x`, `y`, `z`, `root`, `arguments` |
| `server.tick` | `on_server_tick` | `name`, `tick` |

`player.command` is targeted to the one plugin that owns `root`; it is not a
broadcast subscription and must not be added to `events`. `arguments` is the
unparsed remainder after the root, with leading separator whitespace removed.
For player gameplay events, `context_verified` is `true` when `uuid`,
`username`, `operator`, and `x`/`y`/`z` are immutable server-authoritative
snapshots from the verified login profile, current accepted player pose, and
command permissions at publication time. Legacy Rust event constructors retain
their signatures but publish `context_verified = false`; `uuid`, `operator`,
and `x`/`y`/`z` are absent (`nil` in Lua). Their pre-existing joined/command
`username` field remains available for compatibility but is not verified
context. They do not provide a query API and never include the peer IP address.

## Commands

```lua
solaris.send_message(player_id, text)
solaris.broadcast(text)
solaris.disconnect(player_id, reason)
solaris.run_console(command)
solaris.spawn_entity(player_id, entity_type, x, y, z)
```

`run_console` is denied unless the first word of the command is listed in
`console_commands`. The current server executes the same command parser used by
stdin. Commands that require a player source are rejected.

`spawn_entity` creates exactly one entity only when `entity_type` is in the
plugin's `spawn_entities` declaration. The identifier must be fully namespaced
and the coordinates must be finite, with `x` and `z` within +/-30,000,000 and
`y` within +/-20,000,000. Invalid input and undeclared entity types fail
synchronously in Lua before entering the server command queue. Unknown server
registry types, a full or closed queue, shutdown, a stale player session, or a
canceled owner response are rejected once and logged by the server; Lua receives
no entity handle, promise, or later result event.

The player ID is only a session fence. The network adapter resolves the server
registry type before submission. The simulation owner then allocates and records
the entity and sends normal visibility updates through the reliable outbound
lane. If that player disconnects before owner execution, the spawn makes no
mutation and a later session cannot inherit it. Lua never receives a simulation
handle, session registry, entity ID, or numeric protocol type ID.

Active `player_commands` roots are sent in every player's command tree as
literal commands with an optional greedy argument string. Active
`operator_commands` roots are sent only to operators and are marked restricted.
They do not expose or grant any built-in administrator command. A forged
non-operator command to an operator root returns the normal permission-denied
response and is rejected before entering the Lua event queue. Unknown roots and
built-in permission checks continue through the normal server command parser.

## Isolation

Plugins run on one dedicated host thread, in one Lua VM per plugin. Server tasks
publish immutable events through a bounded queue and never wait for Lua. Lua
commands return through another bounded queue and execute outside Lua.

Each VM has fixed limits: 16 MiB of Lua memory, 100,000 instructions per load or
handler, and 32 commands per event. A failed or over-budget handler disables only
that plugin. The host exposes table, string, math, and UTF-8 libraries; filesystem,
network, process, package loading, and debug libraries are unavailable.

Event and command queues remain bounded and nonblocking. If the event queue is
full, an authorized recognized player command is dropped under the existing
backpressure policy and the server does not send an immediate unknown-command
response. Operator-root authorization happens before that queue check. If the
Lua host queue is closed, the root is no longer accepted.

Hot unload and reload are not supported. If a plugin disables after a handler
failure, all of its roots are removed from server admission before the host
processes another event. Players who connect afterward receive the updated
tree. Already connected clients may retain stale completion nodes until they
reconnect; invoking such a stale root is rejected as unknown because it no
longer has an active owner.

This API covers lifecycle, chat, disconnects, existing console commands, and
allow-listed entity spawning. Direct world, inventory, recipe, and
custom-content APIs are not implemented yet.
