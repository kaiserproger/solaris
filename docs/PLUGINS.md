# Luau Plugins

Solaris exposes one current Luau plugin contract: API `0.6.0`. A manifest
requesting any other version is rejected; there is no legacy API or manifest
compatibility path. Plugin packages run on the server in sandboxed strict Luau.
They are not Fabric, NeoForge, Forge, Bukkit, or Paper server mods.

This guide has two layers:

- **Operators:** use [deployment](#operator-deployment),
  [client requirements](#deployment-server-only-or-loader-required), and
  [check/debugging](#check-and-debugging) first.
- **Authors:** start with [package and manifest](#package-and-manifest),
  [capabilities](#permissions-and-capabilities), [events](#events), and
  [commands](#commands). Storage, menus, transactions, zones, and lifecycle
  details remain in the corresponding reference sections below.
- **Players:** follow
  [Solaris Loader installation](SOLARIS_LOADER.md) only when the operator's
  selected plugins require client bundles.

The replacement server/client addon design is part of
[`ARCHITECTURE.md`](ARCHITECTURE.md#one-addon-contract-server-and-client).
This reference describes current API `0.6.0`, not the unimplemented replacement.

`mc-net` currently provides plugin storage, zones, server-owned inventory menus,
inventory/storage and player-inventory transactions, same-dimension teleports,
connected-player queries, durable resident handles, bounded world/entity
mutations, and committed gameplay events. Domain policy remains in Luau. For
example, colony identities, roles, orders, and persistence are plugin-owned;
plugins do not receive Rust world, entity, region, lock, socket, or scheduler
handles.

## Operator deployment

An external plugin is one child directory below `[plugins].directory`. Copy the
complete package, then validate it before startup:

```toml
[plugins]
directory = "plugins"
strict = true
expected = ["my-plugin"]
```

```sh
solaris --check --config server.toml
solaris --config server.toml
```

When running from source, use `cargo run --bin mc-server --` in place of
`solaris`. `strict = true` is the production-shaped mode: every filesystem entry
must be a valid package, every selected plugin must pass compile/startup, and
`expected` must exactly match the final deployed plugin id set. Keep
`strict = false` only for local authoring where skipping an ordinary broken
package is intentional.

Deploy selected package directories before enabling strict validation:

```toml
[plugins]
directory = "plugins"
strict = true
expected = ["basic-economy", "online-roster", "my-plugin"]
```

### Standard plugin pack

Beyond the demonstration examples, Solaris ships an optional first-party
**standard plugin pack** under
[`../../solaris-default-plugins/`](../../solaris-default-plugins/): `solaris-permissions`,
`solaris-essentials`, `solaris-economy`, `solaris-towns`, and `solaris-audit`.
All five are independent server-only API 0.6 packages. Installation is explicit;
the core server installer never enables them automatically. On Linux:

```sh
git clone https://github.com/kaiserproger/solaris-default-plugins.git
git -C solaris-default-plugins checkout 2d51ae5559cdd5b7cbab32888ef2b11d931e3e6e
bash solaris-default-plugins/install.sh --directory "$PWD/plugins"
```

Pass package names after the directory to select a subset. Stop the server
first; the script refuses existing packages and never edits `server.toml` or
overwrites configuration. Set `[plugins].directory` to the destination, enable
`strict`, include every deployed id in `expected`, then run
`solaris --check --config server.toml`. The pinned revision above is the
compatible alpha-4 package snapshot; review changes before selecting another. The script needs
Bash 4+ and GNU coreutils; manual package copying works on other platforms.

Integration coverage lives in
`crates/mc-test-harness/tests/plugin_standard_pack.rs`. Scope decisions,
limitations, and intentional omissions are recorded in
[`../../solaris-default-plugins/standard-pack/README.md`](../../solaris-default-plugins/standard-pack/README.md).
The remaining shipped examples are demonstrations; read their `plugin.toml`,
`config.toml`, `main.lua`, and README before enabling them.

### Deployment: server-only or Loader-required

Deployment is derived from the validated manifest; there is no separate flag:

- **`server_only`:** no `[client].bundles` are declared. An ordinary vanilla
  Minecraft 26.1.2 client can connect.
- **`server_and_client`:** at least one client bundle is declared. Every joining
  player needs the Solaris Loader adapter for a supported platform and must
  approve the exact requested permission set. A client without the required
  handshake is disconnected during Configuration rather than shown substituted
  content.

Run `--check` to see each plugin's derived `deployment`, `supported_loaders`,
`permissions`, `client_bundles`, and `total_artifact_bytes`. Give affected
players [`SOLARIS_LOADER.md`](SOLARIS_LOADER.md); do not tell all players to
install Loader when the selected set is entirely server-only.

### Lifecycle and reload boundary

Discovery, manifest validation, command ownership, worldgen selection, and
client bundle selection occur at startup. API `0.6.0` has no filesystem watcher.
On Unix, `SIGHUP` prepares and atomically replaces Luau generations only when
the server started and remains configured with `plugins.strict = true`.

A safe reload requires the same ordered plugin identities and player command
roots, worldgen contributions, and client bundle contracts. Changing any of
those requires a full server restart; changing a worldgen contribution also
requires a fresh world directory under the persisted world contract. A reload
re-reads package `config.toml`, constructs every candidate VM, runs candidate
`server.started` handlers with staged output, and swaps only after the whole
candidate is admitted. A rejected reload leaves the current generation active.
Runtime-local timers/state start fresh after a successful reload; durable plugin
storage does not.

### Check and debugging

A successful check emits JSON without binding the server. A useful focused view
is:

```sh
solaris --check --config server.toml |
  jq '{operator_warnings, discovered_plugins}'
```

For every discovered plugin, confirm the id, deployment, supported loaders,
permissions, bundle hashes/sizes, and expected-set membership. In strict mode,
malformed packages, stray entries, duplicate ids or command roots, unknown API
versions/capabilities/events, Luau diagnostics, startup traps, missing client
artifacts, and expected-set drift fail check/startup.

At runtime, inspect the server log for `Luau plugin discovered`, reload reports,
handler/batch rejection diagnostics, and the final host exit report. A handler
trap or budget/admission failure disables only that plugin and unregisters its
command roots; it is not a successful degraded state to ignore. Reproduce author
issues with the smallest package and run the focused host tests when developing:

```sh
cargo test -p mc-script
cargo test -p mc-test-harness --test plugin_examples
```

The Loader-required fixture and real-client commands are documented in
[`../examples/loader-live-gate/README.md`](../examples/loader-live-gate/README.md).

## Author quickstart

Create `plugins/hello/plugin.toml`:

```toml
id = "hello"
name = "Hello"
version = "0.1.0"
api = "0.6.0"
events = []
capabilities = []
player_commands = ["hello"]
```

Create `plugins/hello/main.lua`:

```luau
--!strict
function on_player_command(event: any)
    solaris.send_message(event.player_id, "Hello, " .. event.username .. "!")
end
```

For this isolated development deployment, set `plugins.directory = "plugins"`,
`plugins.strict = true`, and `plugins.expected = ["hello"]` in `server.toml`.
Run `solaris --check --config server.toml`, start the server, join with a vanilla
client, and run `/hello`. The reply must name that player. No Loader or
capability is needed for this bounded message command.

Use explicit local types for your own state and validate operator configuration
once at load. The current type-check-only prelude declares `solaris` as `any`;
strict checking catches errors in your Luau, but does **not** statically prove
host method names, argument shapes, or event DTO fields. The host validates
those at invocation. `--check` executes loading/startup, not every future
handler: exercise commands, rejected inputs, result callbacks, and restart
recovery against the server.

Declare only events and capabilities actually needed. Correlate asynchronous
results by `request_id`; returning from a host call means submission, not
successful world mutation. Use the matching result event before reporting
success. Keep durable domain state in plugin storage rather than globals;
globals and timers disappear on restart or replacement. Use simulation timers
and events instead of per-tick scans or polling.

After changing only reloadable source/configuration, a strict Unix deployment
can use the documented SIGHUP replacement. New command roots, plugin identity,
client content, or startup worldgen/rules require restart; see
[lifecycle](#runtime-lifecycle-and-diagnostics). Back up data and review schema
changes before deployment. Do not copy fixture or documentation directories
into the strict plugin root.

## Complete host API index

These are the functions currently registered on `solaris` for API `0.6.0`.
Signatures, bounded record fields, result events, and failure semantics follow
in the linked sections. The [event table](#events) lists callbacks; callbacks
are not additional callable host functions.

| Area | Functions | Contract |
| --- | --- | --- |
| Configuration | `config` | [Configuration](#plugin-configuration) |
| Timers | `schedule_timer`, `cancel_timer` | [Simulation timers](#simulation-timers) |
| Messaging | `send_message`, `broadcast`, `disconnect`, `send_custom_payload` | [Commands](#commands) |
| Entity mutations | `spawn_entity`, `damage_entity` | [Commands](#commands) |
| Storage | `storage_get`, `storage_cas`, `storage_delete` | [Commands](#commands) |
| Durable batch storage | `storage_batch_cas`, `storage_scan`, `operation_status` | [Commands](#commands) |
| Owned inventory | `query_owned_inventory`, `transfer_owned_items`, `reserve_inventory_items`, `inventory_reservation_status`, `release_inventory_reservation` | [Owned item transfers and reservations](#owned-item-transfers-and-reservations) |
| Menus | `open_inventory_menu`, `close_inventory_menu` | [Commands](#commands) |
| Inventory | `inventory_transaction`, `inventory_storage_transaction` | [Commands](#commands) |
| World and players | `set_world_time`, `set_block`, `list_online_players` | [Commands](#commands) |
| Zones and teleport | `upsert_zone`, `upsert_protected_zone`, `remove_zone`, `teleport_player` | [Gameplay adapters](#shipped-economy-and-claims) |
| Persistent residents | `claim_resident`, `spawn_resident`, `query_residents`, `release_resident`, `set_resident_pois` | [Durable residents](#durable-residents) |
| Resident work and orders | `assign_resident_work`, `cancel_resident_work`, `issue_resident_order`, `cancel_resident_order`, `demobilize_resident` | [Resident work and squad orders](#resident-work-and-squad-orders) |
| Settlement sites | `list_settlement_sites`, `query_settlement_site`, `reserve_resident_site`, `release_resident_site`, `survey_site` | [Settlement sites and staged construction](#settlement-sites-and-staged-construction) |
| Staged construction | `prepare_structure`, `advance_structure`, `pause_structure`, `cancel_structure`, `structure_status`, `bind_warehouse` | [Settlement sites and staged construction](#settlement-sites-and-staged-construction) |
| Loader views | `open_client_view`, `present_client_view`, `close_client_view`, `begin_client_selection`, `cancel_client_selection` | [Declarative views](#declarative-views-view-actions-and-world-selection) |
| Loader presentation | `play_client_sound`, `stop_client_sound` | [Client content](#client-content-manifest) |
| Loader blocks | `place_loader_block`, `grant_loader_block_item` | [Commands](#commands) |

`rules.lua` is a separate startup data contract, not another runtime host
namespace. No durable resident handle, physical worker/order API or
settlement-contract operation is available merely because it appears in a
proposal; the schema-2 declarative view API exists but no shipped package
declares `[client]`, so it stays unused.

## Package And Manifest

The configured plugin directory contains one directory per plugin:

```text
plugins/
`-- basic-economy/
    |-- config.toml     # optional operator configuration
    |-- plugin.toml
    |-- rules.lua       # optional startup-only native rule plan
    `-- main.lua        # strict Luau source
```

Every source is parsed and type-checked as `--!strict` Luau before it may claim
commands or receive events, even when the file omits the directive. The shipped
sources include `--!strict` explicitly. Solaris supplies a type-check-only
`solaris` host prelude, rejects diagnostics, then executes the accepted source in
one sandboxed Luau VM with bounded memory and interrupt fuel. In permissive development mode, an ordinary invalid plugin is skipped; a plugin
declaring startup worldgen or client content still fails server startup instead
of silently changing the world/client contract. Production should use strict
deployment mode: every filesystem entry must be a valid plugin directory, every
plugin must finish host startup, and the discovered external id set must
match the configured expected set exactly.

All production plugins are deployed through the external directory:

```toml
[plugins]
directory = "plugins" # optional external root
strict = true
expected = ["basic-economy", "online-roster", "my-external-plugin"]
```

`expected` is valid only when `strict = true`. Duplicate expected ids, missing
plugins, unexpected plugins, malformed packages, stray non-directory entries,
player-command registration conflicts, and Luau compile/startup failures reject
`--check` and normal server startup. The expected set covers the complete
discovered deployment. Keep `strict = false` only for local iteration where
skipping an ordinary broken package is intentional.

Core no longer embeds first-party packages or accepts `plugins.bundled`.
Their source authority is the independent `solaris-default-plugins` repository.
Duplicate plugin ids fail startup before command, Loader, or worldgen metadata
can diverge. Ore and settlement ownership conflicts remain fail-fast.

Every currently shipped first-party example is **Server-only** and accepts an
ordinary vanilla 26.1.2 client. The separate
`examples/loader-live-gate` fixture is **Requires Solaris Loader on client** and
exists for the Fabric/NeoForge/Forge compatibility matrix.

| Example | Deployment |
| --- | --- |
| `basic-economy` | **Server-only** |
| `geological-mines` | **Server-only** |
| `land-claims` | **Server-only** |
| `online-roster` | **Server-only** |
| `solaris-settlements` | **Server-only** (v1; Loader views deferred) |
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
ore_profile = "realistic_deposits"
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

Installing `../solaris-default-plugins/geological-mines` selects large deterministic
cross-chunk deposits under the canonical `realistic_deposits` profile and
disables the vanilla ore pass for that world. Without a declaration the ore
profile remains `vanilla`. Manifests must use the canonical `realistic_deposits`
name; changing the ore profile changes the persisted world contract.

The `colony-villager-scaffold` and `settlement-prototype` packages were removed
by the §9 clean cutover; `../solaris-default-plugins/solaris-settlements`
replaces both with one server-side package. It deliberately declares **no**
`[worldgen]` selector (v1 is server-side only), so installing it does not change
the ore or settlement world contract. Instead it opts into the catalogue-driven
runtime by declaring both `world_sites` and `structure_operations` in
`required_features` **and** shipping an authored `structures/` directory next to
its manifest — see
[Blueprint authoring reference](#blueprint-authoring-reference). Startup refuses
a deployed set in which two packages claim that profile. The legacy
config-driven `[worldgen] settlement_profile = "plains_village_prototype"`
declaration below remains the fallback selection for a deployed set that ships
no catalogue package.

The legacy prototype declaration is startup-only, immutable, and owned by the
plugin that declares it. A plan has at most three uniquely
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

### Startup rules written in Luau

An optional `rules.lua` is executed during plugin preparation, separately from
`main.lua`. It receives the package's `config.toml` as `config` and returns one
table. It has no `solaris` runtime API, filesystem, chunk, or worker access.
The validated result is materialized into native spawning and terrain rules
before spawn generation. Luau is not called per entity tick or generated block.
Only one installed plugin may own this plan.

```lua
--!strict
return {
    placement = {
        land_spacing = 4,
        water_attempts = 16,
        water_depth = 2,
    },
    trees = {{
        biomes = {"minecraft:plains", "minecraft:sunflower_plains"},
        spacing = 97,
        density_threshold = 0.0,
    }},
    clay = {
        rarity = 3,
        radius_min = 2,
        radius_max = 3,
        max_water_depth = 8,
    },
}
```

- `placement`: land candidate separation is 1–4 blocks; water placement tries
  1–32 candidate columns and selects a position at most 1–16 blocks below the
  column's highest water block at or below sea level, stopping at a water gap.
- `trees`: at most 64 declarations, each naming 1–64 biomes. `spacing` is a
  positive deterministic candidate-selection divisor, not a distance in blocks;
  smaller values admit more candidates. `density_threshold` is finite and
  between −1 and 1. Existing support, biome, and tree-shape checks still apply.
- `clay`: positive candidate `rarity` divisor; disk radii satisfy
  `1 <= radius_min <= radius_max <= 3`; water depth is bounded to 1–32 blocks.
  Deposits replace supported shallow underwater sediment, not arbitrary blocks.
- Optional `spawning` contains at most 64 `{biome, groups}` declarations.
  Listed groups replace the corresponding groups for that biome; omitted groups
  and unlisted biomes retain their existing rules. Group names are `creature`,
  `monster`, `water_ambient`, and `water_creature`. Each group accepts at most
  32 entries shaped as `{entity = "minecraft:cow", min = 2, max = 4, weight = 8}`;
  counts satisfy `1 <= min <= max <= 6`, weights are 1–10,000, and duplicate
  biome/entity declarations and unknown fields are rejected.
  Counts are requested pack sizes, not permission to bypass admission: physical
  placement and existing runtime caps still apply (at most six passive and three
  hostile admissions per chunk in the current herd-candidate consumer).

At least one rule category is required. Type checking, the existing plugin
memory limit, instruction fuel, and host-event deadline also apply to this
startup VM. Invalid startup scripts fail even in permissive deployment mode.
Native materialization additionally validates available entities, tree biomes,
and required clay blocks.

The resolved plan's fingerprint is stored in `solaris/world.json`. Changing,
adding, or removing rules requires a fresh Solaris world; Anvil imports reject
them. `SIGHUP` refuses a changed resolved plan, while source formatting alone
does not change its fingerprint. There is no live rule mutation API.

### Client Content Manifest

A plugin may declare startup-only Solaris Loader bundles in `plugin.toml`:

```toml
[client]
schema = 2

[[client.bundles]]
id = "rich-content"
version = "1.2.3"
artifact = "client/rich-content.zip"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
size_bytes = 4096
loaders = ["fabric", "neoforge", "forge"]
content = ["blocks", "items", "views", "view_actions", "assets"]
permissions = [
  "register_blocks",
  "register_items",
  "present_views",
  "send_view_actions",
  "load_assets",
]
```

Schema **2** is closed and shared by all three loaders; schema-1 bundles fail
closed and no parallel schema-1 decoder exists. Each content kind requires its
matching permission pair: `blocks`/`register_blocks`, `items`/`register_items`,
`views`/`present_views`, `view_actions`/`send_view_actions`,
`assets`/`load_assets`, `world_previews`/`present_world_previews`,
`world_selection`/`send_world_selection` and `sounds`/`play_sounds`.
`entity_presentations`/`present_entities` stay unimplemented and unadvertised
until a vanilla renderer gap is proven. A plugin may declare at most eight
bundles; each artifact is capped at 64 MiB, uses a relative canonical path, and
carries a lowercase 64-character SHA-256. The cache identity is
`plugin-id:bundle-id/version/sha256`, so changing bytes requires a new identity
even if an operator reuses a display version.

When at least one bundle is declared, Solaris sends the combined manifest
during Configuration. The client must acknowledge Loader wire protocol **3**,
its exact platform and loader version, all required permissions, and every cache
identity before the server accepts `AcknowledgeFinishConfiguration`. A server
with no client bundles sends no Solaris Loader payload and preserves the vanilla
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
closed `solaris-client.json` index (schema 2) from the first ZIP entry. The
index is a closed object of `screens`, `world_previews`, `blocks`, `items`,
`assets` and `sounds`; unknown fields, unknown widget types and out-of-bound
geometry fail activation. The implemented widget set is `paged_table`, `tabs`,
`input_number`, `input_text`, `select_enum`, `resource_panel`, `action_button`
and `world_marker`; there is no HTML/JS, arbitrary Java, filesystem access or
client-side scripting. Screens declare one of the six kinds `settlement`,
`construction`, `economy`, `garrison`, `army` or `hud`. The index accepts one
owned `blocks` entry (`id`, `model`, `name`), up to 128 owned `items` (`id`,
`base_item`, `name`), up to 64 `world_previews` (`id`, `blueprint_id`, 64-hex
`content_hash`, one quarter-turn `rotation`, `size_x/y/z` at most 64, and at
most 65,536 local `blocks`), and `assets` (`id`, canonical `assets/...` path,
exact SHA-256, exact byte size). It rejects all undeclared archive entries and
bounds the activated registry to 64 screens, 64 previews, one block per bundle
(eight total), 128 items, 128 assets, 64 sounds and 64 MiB of asset bytes. A
block requires `register_blocks` and its exact verified owner model under
`assets/<namespace>/models/<path>.json`; every item requires `register_items`
and its exact verified `assets/<namespace>/items/<path>.json` definition. Up to
64 owned `sounds` entries contain only `id` and require `sounds`/`play_sounds`
plus a same-bundle verified `assets/<owner>/sounds/<id-path>.ogg`.

Fabric, NeoForge, and Forge publish the same immutable registry before
acknowledgement and retain it into Play. Denied, malformed, or unverified
bundles never publish content. Every verified asset path under
`assets/<namespace>/...` is also published as that exact Minecraft resource id
through one transient required pack. The client sends the Loader
acknowledgement only after the pack reload exposes the exact verified bytes. A
close event from that same Configuration connection removes the pack and
reloads resources; a stale close cannot remove a newer connection's pack.

### Declarative views, view actions and world selection

A plugin whose bundle declares `views` plus `present_views` owns a bounded,
server-authoritative view instance per player:

```lua
solaris.open_client_view(request_id, player_id, "plugin-id:showcase", model)
solaris.present_client_view(player_id, view_instance_id, expected_revision, model)
solaris.close_client_view(player_id, view_instance_id)
solaris.begin_client_selection(request_id, player_id, view_instance_id,
    view_revision, "plugin-id:place", constraints)
solaris.cancel_client_selection(player_id, selection_context_id)
```

`model` is a closed table of `page`, `page_count`, at most 64 `rows`
(`cells`), at most 16 typed `fields` (exactly one of `number`/`text`/`selected`
each), at most 16 `actions` (`action_id`, `enabled`, optional `label` and
`deny_reason`), at most 16 `tabs`, at most 16 `resource_entries`
(`id`/`have`/`need`), at most 16 `markers` (`marker_id`, optional
`selection_token`, `action_id`, `formation`, `radius`) and an optional `reason`.
Every string has an explicit byte bound, every number must be finite, and the
encoded message is at most 64 KiB. `open_client_view` mints an opaque instance
id and revision bound to the exact live session, the plugin owner, the verified
view definition, the action whitelist and the presented typed field schema.
`present_client_view` CAS-replaces the model and revision and invalidates prior
actions and selection contexts. `close_client_view` from either side deletes the
instance and its contexts; delivery into a closed instance is refused. The
plugin learns the instance id and revision from `on_client_view_opened(event)`,
and selection context ids from `on_client_selection_started(event)`.

Server-to-client wire **3** messages travel on `solaris:loader/view`:
`open_view { view_instance_id, revision, view_id, title, model }`,
`present_view { view_instance_id, revision, model }` and
`close_view { view_instance_id }`. Client-to-server messages travel on
`solaris:loader/view_action`: `view_action { view_instance_id, view_revision,
action_id, action_sequence, fields[], selection_token? }`,
`cancel_selection { selection_context_id }` and the key-driven
`view_request { request_kind }` with `request_kind` in `{settlement, army}`.
`view_request` is admitted only by the authenticated session, the plugin that
owns that player's declared view of that kind and its `present_views` grant; no
owner, no permission or no declared view of that kind opens nothing and
publishes no event. The owner receives `on_loader_view_request(event)`.

A `view_action` is admitted only after the server re-reads the instance,
revision, the owner's granted `view_actions`/`send_view_actions` pair and the
presented typed field schema: a stale revision, a closed instance, a foreign
owner, a disabled action, a substituted field id/type or a client-minted
price/quantity/actor is refused. `action_sequence` dedup is scoped to
(session, view instance) and is not an operation id; a plugin that needs
exactly-once creates its own durable operation id. The owner receives
`on_loader_view_action(event)` with the instance id, revision, action id,
sequence, typed fields and echoed selection token.

A world selection is armed by the server, never requested by the client for
itself. `begin_client_selection` issues an opaque context bound to the exact
session, owner, instance, revision, action, constraints and a simulation-tick
expiry. The context id reaches the client only through a marker's
`selection_token` of the next presented model; `begin_client_selection` also
requires `world_selection`/`send_world_selection`. One accepted point consumes
the context, and re-sending a consumed context returns the same admission
result with no new effect. View replacement, close, disconnect and permission
revocation invalidate contexts. The derived target/position proof is produced
server-side from the validated fields after the client's point is checked
against authoritative pose, line of sight, range, claims and affiliation. A
marker whose `preview_id` does not resolve to a verified `world_previews` entry
of the same bundle blocks preview/confirm instead of approximating.

**No shipped package declares `[client]` today.** The whole view surface is
implemented and unit-tested in core but unused: nothing Loader-facing becomes
enabled for a package until a package opts in. The corresponding Loader side is
`solaris-loader` (wire 3, schema 2); there is no server-only fallback for a
package that requires it.

A plugin can play or stop its activated owner sound for one player:
A plugin can play or stop its activated owner sound for one player:

```lua
solaris.play_client_sound(player_id, "plugin-id:bell", {
    volume = 1.0, -- optional, finite 0..1
    pitch = 1.0, -- optional, finite 0.5..2
    -- position = { x = 0.5, y = 64.0, z = 0.5 }, -- optional world position
})
solaris.stop_client_sound(player_id, "plugin-id:bell")
```

Without `position`, the one-shot is listener-relative and does not attenuate
with distance. A position uses Minecraft's native linear distance attenuation.
Both modes respect the player's master sound volume. Repeated plays may overlap;
stop ends all playing instances of that same sound id for the target player,
never another owner's sound. There are no loops, moving sources, or playback
completion events. Disconnect stops Loader playback; reconnect does not resume it.

Both commands pass through `ScriptBoundary`, require the caller's namespace and
declared `sounds`/`play_sounds`, and route only to a live acknowledged Loader
session. The client independently requires that exact connection and activated
sound definition. The Play channel `solaris:loader/sound` uses protocol **3**:
big-endian `u16 protocol`, `u8 mode` (`0=stop`, `1=personal`, `2=positioned`),
`u16` byte length and UTF-8 sound id (at most 128 bytes); play adds `f32 volume`
and `f32 pitch`; positioned play adds `f64 x`, `y`, `z`. The total is at most
165 bytes. Invalid modes, lengths, UTF-8, non-finite/out-of-range parameters,
truncation, and trailing bytes cannot produce playback.

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
not shared across sessions. The owning host-attested plugin can request that
exact canonical state with
`solaris.place_loader_block(request_id, "plugin-id:block-id", x, y, z)`.
Solaris rejects a foreign/unknown block id or invalid world position before
mutation, commits accepted coordinates through the server-owned block-edit
transaction, and then publishes required targeted
`loader.block_placement_result`; world storage never contains a client runtime
id. The same exact owner can call
`solaris.grant_loader_block_item(request_id, player_id, "plugin-id:block-id", count)`
with `count` in `1..=64`. The target must be the exact live session that
acknowledged that block carrier. Solaris merges a canonical `minecraft:paper`
stack carrying the verified block name and `solaris_loader:loader_block`
`ITEM_MODEL` into the player's normal inventory, persists it before publication,
leaves a full inventory unchanged, and returns required targeted
`loader.item_grant_result` with semantic success/failure.
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
`spawn_entities` remains an exact allow-list. Cross-domain console execution is
not part of the plugin API; gameplay mutations use typed commands/results instead.
A resource identifier must be fully namespaced lowercase ASCII and no more than 128 bytes.
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

### Permissions and capabilities

Server-side `capabilities` authorize privileged Luau host calls. They are
separate from Loader bundle `permissions`, which a player approves for client
content. Declaring `storage`, for example, does not grant client filesystem or
network access; declaring `load_assets` in a client bundle does not let Luau
read arbitrary server files. Both lists are closed, duplicate-free, and checked
before activation. An undeclared server capability makes its privileged call
fail synchronously; an unknown capability rejects discovery.

`capabilities` is an exact, duplicate-free list:

| Capability | Allows |
| --- | --- |
| `storage` | `storage_get`, `storage_cas`, `storage_delete` |
| `storage_batches` | `storage_batch_cas`, `storage_scan`, `operation_status`; also requires `required_features = ["storage_batches"]` |
| `inventory_transfers` | `query_owned_inventory`, `transfer_owned_items`, `reserve_inventory_items`, `inventory_reservation_status`, `release_inventory_reservation`; also requires `required_features = ["inventory_transfers"]` |
| `inventory_menus` | `open_inventory_menu`, `close_inventory_menu` |
| `inventory_storage_transactions` | `inventory_storage_transaction` |
| `player_inventory` | `inventory_transaction` |
| `zones` | `upsert_zone`, `upsert_protected_zone`, `remove_zone`, owned zone entry/exit events |
| `persistent_residents` | `claim_resident`, `spawn_resident`, `query_residents`, `release_resident`, `set_resident_pois`; also requires `required_features = ["persistent_residents"]` |
| `resident_work` | `assign_resident_work`, `cancel_resident_work`; also requires `required_features = ["resident_work"]` |
| `resident_orders` | `issue_resident_order`, `cancel_resident_order`, `demobilize_resident`; also requires `required_features = ["resident_orders"]` |
| `world_sites` | `list_settlement_sites`, `query_settlement_site`, `reserve_resident_site`, `release_resident_site`, `survey_site`; also requires `required_features = ["world_sites"]` |
| `structure_operations` | `prepare_structure`, `advance_structure`, `pause_structure`, `cancel_structure`, `structure_status`, `bind_warehouse`; also requires `required_features = ["structure_operations"]` |
| `player_teleport` | `teleport_player` |
| `player_queries` | `list_online_players` |
| `entity_damage` | `damage_entity` |
| `world_time` | `set_world_time` |
| `custom_payload:<namespace:path>` | `send_custom_payload` on that channel, owned `player.custom_payload` delivery |

An undeclared privileged call fails synchronously in Luau before it enters the
bounded command batch. Unknown capabilities reject the plugin during discovery.

## Runtime Lifecycle And Diagnostics

Each accepted plugin owns one sandboxed Luau state while all plugin states are
scheduled serially on the dedicated `solaris-luau-host` thread. A handler trap,
instruction/memory/wall-clock budget failure, batch-rejection callback failure,
or host command-admission rejection disables only that plugin. Its player command
roots are unregistered before the host advances to later events; other plugins
continue from the same bounded FIFO.

Runtime disablement is retained as a typed host diagnostic rather than existing
only as a transient log line. Joining `LuaHost` returns `LuaHostExitReport` with
the startup-loaded count, count still enabled at host exit, every disabled plugin id, the
disable stage (`handler`, `batch_rejection_handler`, or `command_admission`), a
UTF-8-safe diagnostic capped at 4 KiB, and the host exit reason. The composition
root logs that report during shutdown. Normal server shutdown drains admitted
script work, publishes the required `server.stopping` event, closes event
admission, and therefore ends the host with `event_queue_closed`; command-queue
closure or unavailable command authority is reported as a non-normal lifecycle
exit instead of being indistinguishable from a clean stop.

API `0.6.0` has a narrow prepared replacement boundary through `LuaHost::reload`.
The caller first builds a complete `PreparedLuaPlugins` candidate; reload rejects
changes to ordered plugin identity/player-command roots, worldgen contributions, or
client bundles because those remain restart-only contracts. The accepted request enters
the same host-input FIFO as ordinary events. Every candidate Luau state is constructed
before swap, then every subscribed candidate `on_server_started` handler runs under the
normal bounded host/instruction/memory/wall-clock rules while its output remains staged.
The host reserves capacity for the complete staged command set and validates host
admission before commit. Only then does it replace command-root ownership in one write,
swap the plugin generation, and publish the staged startup commands. Compile,
`server.started`, command-queue/admission, or command-ownership failure before that
point leaves the current generation and its ownership intact.

Previous-generation fault diagnostics are returned in `LuaReloadReport`; runtime-local
state such as timers starts fresh in the new generation. Host commands already emitted
before the reload barrier remain valid committed output. Once a reload request is
admitted to the host queue it is commit intent; cancelling the caller does not cancel
the host-owned attempt.

On Unix, `mc-server` uses `SIGHUP` as the explicit production reload trigger. It
re-reads the configured TOML on a blocking worker, requires that the server originally
started with `plugins.strict = true` and that the current file still has strict mode,
then reruns the normal external-package discovery and `plugins.expected` validation
before calling `LuaHost::reload`. File preparation and the host replacement are awaited
without pausing the network server future. Other server configuration fields remain the
startup snapshot: SIGHUP applies only the validated plugin replacement. A server with no
Luau host or one started in permissive plugin mode logs a rejection instead of treating
SIGHUP as a successful reload. Non-Unix builds have no SIGHUP trigger.

There is no filesystem watcher, polling cadence, or live client-bundle/worldgen swap;
those restart-only contracts are rejected by the replacement boundary.

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
| `player.custom_payload` | `on_player_custom_payload` | `player_id`, `phase` (`configuration` or `play`), `channel`, binary-safe `payload` |
| `player.client_brand` | `on_player_client_brand` | `player_id`, `brand` |
| `plugin.storage.get_result` | `on_plugin_storage_get_result` | `request_id`, `key`, `value`, `version`, `failure` |
| `plugin.storage.cas_result` | `on_plugin_storage_cas_result` | `request_id`, `key`, `applied`, `version`, `failure` |
| `plugin.storage.delete_result` | `on_plugin_storage_delete_result` | `request_id`, `key`, `deleted`, `version`, `failure` |
| `operation.result` | `on_operation_result` | `request_id`, optional `operation_id`, `state`, optional `revision`/`failure`, typed `payload` |
| `inventory.menu.clicked` | `on_inventory_menu_clicked` | player snapshot, `menu_id`, `slot`, `click` |
| `inventory.storage_transaction.result` | `on_inventory_storage_transaction_result` | `request_id`, `committed` |
| `player.inventory_transaction_result` | `on_player_inventory_transaction_result` | `request_id`, `player_id`, `committed`, `failure` |
| `player.zone_entered` | `on_player_zone_entered` | player snapshot, `zone_id` |
| `player.zone_exited` | `on_player_zone_exited` | player snapshot, `zone_id` |
| `zone.command_result` | `on_zone_command_result` | `zone_id`, `accepted` |
| `player.teleport_result` | `on_player_teleport_result` | `request_id`, `player_id`, `x`, `y`, `z`, `committed`, `failure` |
| `world.time_set_result` | `on_world_time_set_result` | `request_id`, `world_time`, `committed`, `failure` |
| `world.block_set_result` | `on_world_block_set_result` | `request_id`, `dimension`, `block_id`, `x`, `y`, `z`, `applied`, `failure` |
| `loader.block_placement_result` | `on_loader_block_placement_result` | `request_id`, `block_id`, `x`, `y`, `z`, `placed`, `failure` |
| `loader.item_grant_result` | `on_loader_item_grant_result` | `request_id`, `player_id`, `block_id`, `count`, `granted`, `failure` |
| `entity.spawn_result` | `on_entity_spawn_result` | `request_id`, `player_id`, `entity_type`, `x`, `y`, `z`, `spawned`, `failure` |
| `entity.damage_result` | `on_entity_damage_result` | `request_id`, `entity_id`, `amount`, `damaged`, `health`, `killed`, `failure` |
| `player.online_result` | `on_player_online_result` | `request_id`, `players`, `truncated` |

`player.custom_payload` is targeted like `player.command`: the host routes it
only to the plugin that owns `channel`, never broadcasts it. A handler must
declare `custom_payload:<namespace:path>` for that exact channel to receive
the event and to call `send_custom_payload` on it. `phase` is
`configuration` for payloads sent before the play transition and `play`
afterwards. `payload` is a binary-safe Luau string up to the fixed
32768-byte host bound; unknown channels and larger bodies are rejected
before event retention. Channel ownership is exclusive: a second plugin
claiming the same channel fails registration, and reload/disable releases
the owner's routes. `player.client_brand` carries the client's
`minecraft:brand` payload (`player_id`, `brand`) with no capability
required.

The `solaris:loader/` channel namespace is reserved for Loader control traffic.
Raw payload manifests and command admission reject it. Use the typed Loader
APIs, which enforce bundle permissions and resource ownership.

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
solaris.send_custom_payload(player_id, channel, payload)
solaris.send_message(player_id, text)
solaris.broadcast(text)
solaris.disconnect(player_id, reason)
solaris.spawn_entity(request_id, player_id, entity_type, x, y, z)
solaris.damage_entity(request_id, entity_id, amount)
solaris.place_loader_block(request_id, block_id, x, y, z)
solaris.grant_loader_block_item(request_id, player_id, block_id, count)
```

`spawn_entity` is a typed entity mutation, not fire-and-forget presentation. The
plugin must allow-list the exact namespaced type in `spawn_entities`; the host checks
that declaration before the request can leave the bounded Luau batch. `request_id`
uses the normal 64-byte script-id grammar, `player_id` is the actor session whose
simulation fence authorizes the spawn, and the position uses the existing finite
script coordinate bounds.

After host admission the router resolves the type against the active server entity
registry and submits the spawn through the actor-fenced simulation owner. Success
publishes targeted `entity.spawn_result` with the original `request_id`, actor
`player_id`, type and position, `spawned = true`, and `failure = nil`. A type that
was declared by the plugin but is absent from the active server registry returns
`unknown_entity_type` without enqueueing a simulation mutation. A stale/missing actor
returns `actor_unavailable`; owner queue pressure returns `busy`; a closed, stopped,
shutting-down, timed-out, or unavailable runtime returns `runtime_unavailable`; other
owner rejections return `rejected`. Failed results use `spawned = false` and a
non-nil failure. The result reports simulation-owner commit, not socket delivery or
client rendering.

`damage_entity` is the bounded common combat primitive for server-owned non-player
entities. It requires the `entity_damage` capability. `request_id` uses the normal
64-byte script-id grammar, `entity_id` must fit the server's signed 32-bit entity-id
space, and `amount` must be finite, positive, and at most 1,000,000. This operation is
not disguised player melee: it does not invent a player attacker, held item, cooldown,
knockback, or villager-player gossip attribution.

After host admission the request enters the existing simulation-owner generic entity
attack kernel. That kernel retains hurt-invulnerability, accepted-health publication,
death scheduling, and the normal server-entity kill reward path. Success publishes
required targeted `entity.damage_result` with the original request id/entity id/raw
amount, `damaged = true`, authoritative post-commit `health`, exact `killed`, and
`failure = nil`. Missing/non-living targets, hurt-invulnerability, or other definite
owner rejection return `damaged = false`, `health = nil`, `killed = false`, and
`failure = "rejected"`. Queue pressure returns `busy`; a closed/stopped/timed-out/
shutting-down/unavailable owner returns `runtime_unavailable`. The result reports the
simulation-owner combat commit, not eventual client packet/rendering state.

Custom payloads share one script boundary: `send_custom_payload` sends data,
and `player.custom_payload` receives it.
The plugin must declare `custom_payload:<namespace:path>` for the exact
`channel`; sending on another owner's channel fails synchronously before
the bounded command batch, like any undeclared capability. `payload` is a
binary-safe Luau string of at most 32768 bytes; larger bodies are rejected
by host admission and never reach the wire. After admission the router
writes one `ClientboundCustomPayload` with the same channel and bytes to
the connected player's ordered reliable session lane. Inbound payloads on
owned channels arrive as targeted `player.custom_payload` events described
above; the client's `minecraft:brand` payload additionally arrives as
`player.client_brand`.

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

**Durable batch storage and snapshot scans**

These calls require both `capabilities = ["storage_batches"]` and
`required_features = ["storage_batches"]` in the package manifest. API version
`0.6.0` alone does not advertise this extension. An unknown required feature,
or this capability without its required feature, fails package admission.

```luau
solaris.storage_batch_cas(request_id, operation_id, {
    { operation = "cas", key = "ledger", expected_version = 4, value = "7" },
    { operation = "cas", key = "intent", expected_version = nil, value = "paid" },
})
solaris.storage_scan(request_id, prefix, cursor, limit)
solaris.operation_status(request_id, operation_id)
```

`request_id` correlates delivery; `operation_id` identifies the durable mutation.
Both use the existing lowercase ASCII letters/digits/underscore/hyphen grammar
and 64-byte bound. A batch contains 1–16 distinct keys, uses the same CAS/delete
preconditions and value limits as standalone storage, and changes every key
at one revision or changes none. Mutation order is canonicalized by key.
Successful operation fingerprints and outcomes survive delivery acknowledgement,
compaction and restart. A new request id with identical operation content
replays that outcome without applying the batch again; substituted content
returns `operation_conflict`. The host supplies the owner namespace.

Results are targeted to `on_operation_result`; no broadcast subscription is
needed. These synchronous storage calls use `state = "committed"` or
`"rejected"`. Successful results carry an integer `revision`; rejection carries
an explicit `failure`. Revisions crossing Luau are bounded by `2^53-1`.
The closed payload forms are:

- `kind = "storage_batch"`: `changes`, a one-based array of `{ key, deleted }`.
- `kind = "storage_page"`: `entries`, a one-based array of
  `{ key, value, revision }`, and an optional continuation `cursor`.
- `kind = "none"` for a rejected request.

Batch precondition conflicts return `stale_revision`; exhausted capacity returns
`capacity`. An unavailable or fail-stopped storage actor returns
`runtime_unavailable`. An uncertain post-append synchronization result is not
reported as rejection: recovery resolves the journal and replays its pending
receipt. `operation_status` returns the saved owner-scoped outcome;
`not_found` is a failed lookup, not evidence of a stored rejected operation.

For a scan, pass `nil` as the first cursor, an optional empty-string prefix,
and a required limit of 1–64. Records are sorted by key. Continuations retain
the original snapshot revision and values despite concurrent writes; retrying
the same cursor returns the same page. Continue with the same prefix and limit.
A cursor belongs to one plugin, expires after 60 seconds, and is invalid after
server restart. Foreign cursors return `forbidden`; expired/unknown cursors
return `cursor_expired`; changing query parameters returns `invalid_request`.
Start a new scan after expiry. A terminal page has `cursor = nil`.

The host retains at most eight paginated snapshots per plugin, 64 globally,
and 64 MiB of charged snapshot data including cursor/key overhead. Snapshot
creation scans only the bounded owner namespace, not the world or other plugins.
Retained immutable records share their values with live storage; quota accounting
still charges the full retained value size.

**Durable residents**

These calls require both `capabilities = ["persistent_residents"]` and
`required_features = ["persistent_residents"]`. As with durable batch storage, an
unknown required feature, or this capability without its required feature, fails
package admission.

```luau
solaris.claim_resident(request_id, operation_id, actor_id, entity_uuid, expected_entity_revision)
solaris.spawn_resident(request_id, operation_id, spawn_site_token, profile)
solaris.query_residents(request_id, handles, cursor)
solaris.release_resident(request_id, operation_id, handle, expected_revision)
solaris.set_resident_pois(request_id, operation_id, handle, home_poi, work_poi, meeting_poi, expected_revision)
```

A resident `handle` is an opaque owner-scoped string, not an entity id or a
pointer. It addresses the same NPC after restart, unload/reload and region
migration; a handle stolen by another plugin yields `forbidden`, and a UUID or a
coordinate supplied by the caller never grants authority. Owner comes from the
admitted plugin, actor from the authenticated session.

`claim_resident` adopts a live adult villager: the entity must be a loaded
`minecraft:villager` with no other owner, inside `MAX_RESIDENT_CLAIM_DISTANCE`
of the actor's authenticated pose in the simulated dimension.
`expected_entity_revision` is core's durable record revision for that entity, or
`0` when the entity has no resident record yet. A plugin that already owns the
NPC gets its existing handle back and never re-claims. `spawn_resident` consumes
one core-issued `spawn_site_token`; a consumed, released, or foreign token is
refused, and repeating the same `operation_id` with the same fingerprint returns
the stored outcome instead of a second resident. `set_resident_pois` binds the
same resident to owner-scoped POI handles; `nil` clears one binding.

`query_residents` takes at most 64 handles, or a cursor over the plugin's own
resident pages (64 records per page, `cursor_expired` for an unknown cursor), and
returns per handle: entity UUID, lifecycle, revision, generation id, POIs, and
live pose/health/carried items while the chunk is loaded.

Lifecycle states are `alive_loaded`, `alive_unloaded`, `dead`, `released`.
`unloaded` is not `dead` and never justifies replacing a fighter with the nearest
villager. A dead or converted entity leaves a heavy tombstone that keeps the
handle bound (blocking a silent rebind) but frees living capacity; `release`
drops ownership explicitly, keeps the NPC in the world and frees living capacity
too. Session disconnect neither dismisses a garrison nor drops ownership, and
unloading a plugin never transfers NPCs to another owner.

Failures are typed: `invalid_request`, `forbidden`, `not_found`, `unloaded`,
`stale_revision`, `capacity`, `operation_conflict`, `cursor_expired`,
`runtime_unavailable`. A plugin may hold at most 64 living residents and 256
resident records including tombstones.

Settlement sites, owned `spawn_site_token` issuance and staged construction are
documented under [Settlement sites and staged construction](#settlement-sites-and-staged-construction).
Resident work orders, squad orders, equipment and demobilisation are
documented under [Resident work and squad orders](#resident-work-and-squad-orders).
`set_resident_pois` stores core-validated owner-scoped POI handles; physical POI
existence and capacity are checked against the site catalog.

**Resident work and squad orders**

Work orders require `capabilities = ["resident_work"]` and
`required_features = ["resident_work"]`; squad orders, combat policy and
demobilisation require `capabilities = ["resident_orders"]` and
`required_features = ["resident_orders"]`.

```luau
solaris.assign_resident_work(request_id, operation_id, handle, work_order, work_units, expected_revision)
solaris.cancel_resident_work(request_id, operation_id, handle, expected_revision)
solaris.issue_resident_order(request_id, operation_id, handles, expected_order_revisions, order)
solaris.cancel_resident_order(request_id, operation_id, handles, expected_order_revisions)
solaris.demobilize_resident(request_id, operation_id, handle, expected_revision)
```

`work_order` is a closed tagged union: `harvest`, `replant`, `cut_tree`, `mine`,
`haul`, `craft`, `fish`, `tend_livestock`, `construct`. Each names a concrete
bounded `area` (dimension, min/max block corners, at most 16 per axis), the
required tool/feed/recipe (a namespaced item id or recipe id), and the endpoint
that receives the produce. `haul` moves items between two of the plugin's own
resident endpoints. `construct` names a prepared `structure_id`, one authored
`stage` and the expected structure revision, and drives that stage through the
structure's committed reservation. A job is executed through the existing
gathering, recipe, inventory and movement mechanics — not a passive resource
generator — so a missing tool, missing input, protected plot, unloaded chunk or
blocked route pauses the job with a typed reason (`missing_tool`, `missing_input`,
`protected`, `unloaded`, `blocked_route`, `no_storage`, `unsupported`) and
reports zero committed work. The result carries only the work units and
inventory deltas that were actually committed.

`order` is a closed tagged union: `follow` (an authenticated player id),
`move` (dimension, anchor, heading, formation), `hold` (anchor, heading,
formation, engagement radius), `patrol` (2–16 waypoints, engagement radius),
`garrison` (approved post handles, engagement radius), `attack` (server-issued
target refs plus a bounded `policy` with a revision) and `retreat` (safe anchor,
formation). Formation is `line`, `column`, `wedge` or `square` with a spacing in
half-blocks; slot positions are computed by the engine, stable across updates,
account for entity footprint and obstacles, and an impossible formation reports
`blocked_route` instead of stacking members on one coordinate. A `garrison` post
handle is a completed guard point of interest of a C2 site snapshot; core
resolves the approved posts from that committed layout, assigns each member a
free slot with an engine-computed, stable position, persists the occupancy with
the order, and reconstructs the same post goal after a restart. A later garrison
order never double-books an occupied slot, and a member with no reachable free
post reports `blocked_route` and receives no goal. At most 64 handles are
accepted per order (gameplay squads up to 32); every member fence is checked
before any order is replaced, and a partially unavailable batch is refused as a
whole with a per-member reason. `attack` uses real range, line of sight, cooldown
and ammunition through the engine damage path, never repeated `damage_entity`
calls from Luau; the committed `combat` list on the order result carries
attacker/victim correlation and an event id so experience is awarded exactly
once. Engagement policy lists allied affiliations (≤64) and permitted hostile
categories; owned residents and allies are never targeted by proximity, and
player targets are off unless the policy explicitly permits them, with server PvP
rules applied on top.

`demobilize_resident` cancels the military order, returns equipment through the
canonical transfer path and moves `military → demobilizing → civilian`. It keeps
the same resident handle, home and plugin ownership; when a warehouse is
unreachable the resident stays `demobilizing` with the reason and keeps every
item — an NPC is never killed by a hidden debt timer. `release_resident` remains
the full ownership drop, not an ordinary dismissal.

Notifications for `resident.work_changed`, `resident.order_changed` and
`resident.combat_committed` are not implemented yet; state is read from the
operation result and `query_residents`. Loader-driven client views, world
preview, target selection and entity presentation for these orders are disabled
in v1 and must not be required or declared by a server-side package.

**Settlement sites and staged construction**

Site discovery and survey require both `capabilities = ["world_sites"]` and
`required_features = ["world_sites"]`; staged construction requires
`capabilities = ["structure_operations"]` and
`required_features = ["structure_operations"]`. An unknown required feature, or
either capability without its required feature, fails package admission.

```luau
solaris.list_settlement_sites(request_id, cursor, limit)
solaris.query_settlement_site(request_id, site_id, cursor, limit)
solaris.reserve_resident_site(request_id, operation_id, site_id, poi_id, expected_site_revision)
solaris.release_resident_site(request_id, operation_id, spawn_site_token)
solaris.survey_site(request_id, dimension, bounds, purpose)
solaris.prepare_structure(request_id, operation_id, blueprint_id, anchor, rotation, survey_token, expected_site_revision)
solaris.advance_structure(request_id, operation_id, structure_id, stage, reservation_ref, expected_revision, work_units)
solaris.pause_structure(request_id, operation_id, structure_id, expected_revision)
solaris.cancel_structure(request_id, operation_id, structure_id, expected_revision)
solaris.structure_status(request_id, structure_id)
solaris.bind_warehouse(request_id, operation_id, structure_id, container_id)
```

Candidates are a pure function of `(seed, profile revision, coordinates)`: the
same world always yields identical site ids, footprints, buildings, POIs, roads
and inhabitant generation ids regardless of chunk generation order or how often
discovery runs. `list_settlement_sites` pages over deterministic candidates in
scan order (1–64 records, an opaque owner-scoped cursor); `query_settlement_site`
returns one site, or a bounded POI page, by deterministic site id. `forbidden`
covers a foreign cursor or site id, `cursor_expired` an unknown cursor, and
`not_found` a site id that does not resolve. **Discovery never invents a plan in
Luau and never downgrades a large variant that does not fit** — a candidate that
cannot be laid out from the authored catalog is rejected, not silently shrunk.

A site snapshot carries the deterministic site id, the variant (`hamlet`,
`village`, `town`), the site revision, the footprint origin/size, the placed
buildings (`blueprint_id`, origin, rotation), the POIs (`poi_id`, kind
`home`/`work`/`meeting`/`guard`, position, capacity, state
`free`/`reserved`/`occupied`) and the inhabitant generation ids. Generation ids
derive from the world identity, the site id and the inhabitant slot, so
re-generating or reinstalling a chunk never yields a second inhabitant for the
same slot. A generation id is a fixed-width hash, so it stays bounded for any
world directory depth and does not depend on the server's directory path length.

`survey_site` takes a `bounds` record `{ min = {x, y, z}, max = {x, y, z} }` and
a `purpose` (`settlement`, `expansion`, `restoration`) and returns a bounded
snapshot: per-column heights, water depth and slope, usable plots, water and
biome/resource tags, intersecting claims and existing structures, chunk
availability (`loaded` or `unloaded`), a revision and a short-lived owner-scoped
`survey_token`. At most 128×128 columns are surveyed per call; larger sites are
surveyed as linked plots. An unsurveyed region reports `unloaded` rather than
loading an unbounded area. A token is bound to the owner, the bounds and the
revision it saw; if a player changes a block inside the footprint, the token no
longer authorises `prepare_structure` and the site must be re-surveyed
(`stale_revision`), never overwritten.

`prepare_structure` validates the blueprint, rotation, geometry, overlaps,
rights and the entrance/street connection against the survey, then reserves the
footprint and returns a persistent `structure_id`, the stage plan, the resource
plan hash and revisions. It builds nothing and spends nothing. Construction
mutations each carry their own `operation_id`; repeating an id with the same
fingerprint returns the stored outcome and a different one returns
`operation_conflict`. `advance_structure` links the next stage to a
`reservation_ref` from `reserve_inventory_items` and the immutable
`resource_plan_hash`; each world-commit portion is at most 512 blocks with
dependent groups committed together and produces a monotonic receipt carrying the
exact consumed materials, work units and revision — world edits, consumption and
the receipt commit through one recoverable protocol, so a crash cannot leave
blocks without cost or cost without work. A crash/replay between the reservation
and a stage commit neither double-consumes nor builds twice. `pause_structure`
and `cancel_structure` serialise against the active portion and return a final
receipt watermark plus per-resource `consumed`, `returned` and `remaining`, where
`reserved = consumed + returned + remaining`; only the returned remainder is
ever credited. Already-built blocks stay in the world on pause or cancel, and a
player block change inside the footprint pauses the operation as
`paused/site_changed` without consuming or overwriting the player's build.
`structure_status` returns the saved owner-scoped snapshot. A plugin may have at
most 64 active construction operations.

```luau
-- survey a plot, prepare a house, then build it stage by stage
local survey = solaris.survey_site("survey-1", "minecraft:overworld",
    { min = { x = 0, y = 0, z = 0 }, max = { x = 63, y = 255, z = 63 } }, "settlement")
-- on_operation_result: payload.survey.survey_token, payload.survey.revision
```

**Blueprint authoring reference**

An authored catalog lives next to the manifest at `structures/<id>.toml`, one
blueprint per file, with an optional `<id>.ruined.toml` ruined variant of the
same footprint. Keys are closed; unknown keys, a blueprint id outside the
package namespace, a stray non-`.toml` file, an out-of-range or undeclared
palette property and any limit violation reject the whole package before it is
admitted. `id` is namespaced and owned by the package; `revision` is an integer;
the content hash is derived, never authored (a deployed manifest hash that does
not match the derived content hash is refused).

```toml
id = "solaris:house_small"
revision = 3
variant_of = "solaris:house_small"

[footprint]
size = [13, 9, 11]
anchor = [6, 0, 5]

[[palette]]
index = 0
block = "minecraft:oak_planks"
properties = { }

[[blocks]]
x = 0
y = 0
z = 0
palette = 0

[[poi]]
id = "home"
kind = "home"
at = [6, 1, 5]
capacity = 2

[[street_connection]]
at = [0, 0, 5]
facing = "west"

[[stage]]
id = "frame"
blocks = [ { x = 0, y = 1, z = 0, palette = 1 } ]
```

`rotation` is a quarter turn (0/90/180/270); dependent properties — stairs,
doors, beds, fences and wall connections — are rotated through the registry
property model, never blanket-replaced, and a multi-part object is committed as
one unit. Block entities are limited to a whitelist (empty containers and the
data a bed or sign needs); arbitrary NBT, loot tables, commands and spawners are
refused. Every buildable structure must declare physical entrances and POIs.

Hard limits, per deployed plugin: 128 blueprints, 64 settlement variants, 128
building placements per settlement, 65 536 blocks per blueprint, 64 blocks per
blueprint footprint axis, 16 MiB decoded catalog (checked outside the Lua VM),
512 blocks per world commit portion and 64 concurrent construction operations. A
settlement **site** footprint is territory, not a building: 128×128 (hamlet),
192×192 (village) or 256×256 (town), reported verbatim in the site snapshot and
bounded separately at 256 per axis. Each building placed inside it still uses a
blueprint bounded at 64 per axis, so the two limits must not be conflated. A
whole town is never packed into one mega-template or one Lua callback.

Construction execution (this section) is landed and tested. `advance_structure`
consumes the materials the plugin reserved and records a receipt; linking actual
builders to a stage is `solaris.assign_resident_work` with a `construct` work
order, or `solaris.advance_structure` when the plugin drives the portion itself.

**Package discovery and startup validation**

Core discovers the settlement profile during server startup, before the plugin
storage actor starts. A package owns the profile when its manifest declares both
`required_features = ["world_sites", "structure_operations"]` and it ships a
`structures/` directory next to `plugin.toml`. Blueprint ids must be namespaced;
the namespace is read from the authored catalog itself (the shipped
`solaris-settlements` package authors `solaris:*` ids), and every file in one
catalog must agree on it. Discovery reads every `structures/*.toml` in
deterministic file order, validates it through the frozen loader, and derives the
profile revision from each blueprint's owned id, revision and derived content
hash. Selection, layout and inhabitant generation ids are therefore a pure
function of `(seed, profile revision, coordinates)` and do not depend on chunk
generation order.

A violation fails startup by name — never a degraded empty catalog:

- a foreign or mixed id namespace, a duplicate id, an out-of-range or undeclared
  palette property, and any frozen-limit breach (128 blueprints, 64 variants, 64
  footprint axis, 65 536 blocks, 16 MiB decoded catalog);
- a stray catalog entry: any non-`.toml` file or subdirectory under `structures/`;
- a deployment-recorded content hash that does not match the derived one;
- two deployed packages claiming the profile, or a claim with no catalog.

When no package claims the profile, the config-driven `[data] settlement_profile`
prototype path in `crates/mc-server` stays the default and every settlement call
answers the typed `runtime_unavailable`; nothing panics and no empty catalog is
installed. No part of discovery requires the Solaris Loader: the shipped
server-side package declares no client bundle, and a Loader-required package is
not needed for the profile.

**Committed structure bounds**

`advance_structure` applies each portion through the same conditional world
storage kernel a player edit uses: at most 512 blocks per portion, dependent
groups atomic, restricted to the structure's own reserved footprint, and refused
(never partially applied) when a target position is not loaded, a precondition
changed, or the batch crosses a region. A committed portion marks its chunks
dirty and reaches `.mca` through the server's dirty-flush owner, so a committed
stage is visible to players and survives a world reopen; re-opening the plugin
ledger replays the structure, its consumption and its reservation without
building a second portion. Footprint change detection is localized through the
world chunk journal watermark, so a player edit inside the reserved footprint
pauses the structure as `paused/site_changed` while unrelated edits elsewhere do
not. A protected zone owned by another plugin intersecting the footprint blocks
`prepare_structure` (`blocked`) before anything is reserved.

`solaris.assign_resident_work` with a `construct` work order drives the prepared
stage directly: core resolves the structure's committed reservation (by the
recorded reference, or the one whose immutable resource plan hash matches before
the first advance), consumes exactly the reserved portion for the committed
cells, commits those blocks through the same world kernel, and reports the
receipt's work units. A repeated portion replays its receipt instead of spending
the reservation or building the blocks a second time. Nothing in this path
depends on the Loader or a client bundle.

**Owned item transfers and reservations**

These calls require both `capabilities = ["inventory_transfers"]` and
`required_features = ["inventory_transfers"]`. As with durable batch storage, an
unknown required feature, or this capability without its required feature, fails
package admission.

```luau
solaris.query_owned_inventory(request_id, endpoint, expected_revision)
solaris.transfer_owned_items(request_id, operation_id, actor_id, transfers, expected_revisions)
solaris.reserve_inventory_items(request_id, operation_id, endpoint, resource_plan, expected_revision)
solaris.inventory_reservation_status(request_id, reservation_ref)
solaris.release_inventory_reservation(request_id, operation_id, reservation_ref, expected_revision)
```

An `endpoint` is a closed tagged union. `{ kind = "player_inventory",
player_id = 512 }` addresses the canonical main-inventory and hotbar window
(slots 9–44) of one connected player. `{ kind = "warehouse", handle = "..." }`
addresses a placed container whose handle is minted by core for a verified
loaded container; a plugin never chooses a container by coordinates.
`solaris.bind_warehouse` issues a handle for one authored `empty_container`
seed of a placed structure the caller owns: the plugin names its durable
`structure_id` and the container's authored ordinal (`container_id`, in
blueprint order). A structure is placed while it is prepared, running, paused or
committed — a completed structure keeps its ground and its containers, so a
handle minted during construction survives completion — and only `cancelled`
(removed) stops being a warehouse, with `blocked`. For a request core has not
already recorded, core verifies ownership (`forbidden` for a foreign owner or
container), that the structure exists (`not_found`), that the ordinal names an
authored container of that blueprint and the blueprint still matches the
prepared content hash (`not_found`/`stale_revision`), and that the placed
position is a loaded container (`unloaded`/`not_found`); a replayed or repeated
request returns the recorded binding first, without re-running those checks.
The binding is durable and idempotent, returns `{ handle, structure_id,
container_id, revision }`, and every accepted request records a receipt
recoverable through `operation_status`; a repeat keeps the original binding and
its revision. The handle is a predictable, non-secret string of the form
`warehouse:<plugin>:<structure>:<container>`, bounded to 128 bytes. A
`warehouse` endpoint then reads through `query_owned_inventory` as the
container's canonical snapshot, fenced by the binding revision; a foreign,
unknown, cancelled, unloaded or non-container handle fails closed with
`forbidden`, `not_found`, `blocked` or `unloaded` and never reads as an empty
container. The write/transfer path for a warehouse endpoint is not enabled yet
and still fails closed.

```luau
local inventory = solaris.query_owned_inventory("q", { kind = "player_inventory", player_id = 512 })
solaris.transfer_owned_items("t", "haul-1", 512, {
    { source = { kind = "player_inventory", player_id = 512 }, source_slot = 9,
      destination = { kind = "player_inventory", player_id = 512 }, destination_slot = 10,
      count = 8 },
}, {
    { endpoint = { kind = "player_inventory", player_id = 512 },
      fence = { revision = 41, snapshot_hash = "…64 hex…" } },
})
```

A snapshot result is `kind = "owned_inventory"` with `result.kind = "snapshot"`:
the endpoint, a `fence` (`revision` plus derived canonical `snapshot_hash`), and
`slots`, a bounded array of `{ slot, item }`. An item summary carries
`resource_id`, `count`, optional `damage`, `enchanted` components, optional
`custom_name` and optional `item_model`; there is no arbitrary NBT. `slot` is
bounded by the endpoint (54 owned slots), and a query for a disconnected player
returns `not_found`.

Bounds: a request moves at most 16 transfers and at most 4096 items in total,
one resource plan covers at most 16 resource types and 512 work portions, and a
snapshot is at most 54 slots. Exceeding a bound fails DTO validation before any
effect.

`transfer_owned_items` checks the actor session, endpoint ownership, slot
validity, counts and every participant's expected revision before one
recoverable commit. The actor must be one of the participant player endpoints;
a foreign endpoint returns `forbidden`. Any `fence` mismatch returns
`stale_revision`; a destination that cannot accept the stack returns
`capacity`; a source that does not hold the count returns `insufficient_items`.
Item components and durability move with the stack. A successful transfer
returns `result.kind = "transfer"` with the participant endpoints and their new
`fences`. All participant state changes are appended as one world-inventory
decision, so after a crash a stack is never in two places and a replay never
moves it twice. When a commit's durability is unknown the operation is
recoverable rather than rejected: the handler stops, and `operation_status`
returns the journaled outcome after recovery.

Reservations are durable and block the reserved quantities from being consumed
by another transfer. `reserve_inventory_items` validates the endpoint fence,
sums the plan per resource, and fails `insufficient_items` when the endpoint
does not hold the requested quantity beyond quantities already reserved by the
same or another owner. It returns `reservation_ref` (opaque), the canonical
`resource_plan_hash`, and per-resource `{ resource_id, reserved, consumed,
returned, remaining }` with `reserved = consumed + returned + remaining` and
`receipt_watermark`. `inventory_reservation_status` returns the owner-scoped
saved snapshot; an unknown reference returns `not_found`.
`release_inventory_reservation` is serialized with consumption, CASes the
reservation revision, and returns the reservation with `released = true`,
`remaining = 0` and every un-consumed unit folded into `returned`; releasing an
already released or stale reservation returns `stale_revision`. Consumption of
a reservation happens only through committed receipts with a monotonic receipt
sequence and an exact consumed-material vector; that receipt path is consumed
by the world/work operation, not by a separate Lua call.

Every mutation is idempotent by `operation_id` and a canonical fingerprint:
repeating the same operation returns its stored result, and reusing the id with
different content returns `operation_conflict` with no effect. Results are
targeted `on_operation_result` events with `state = "committed"`/`"rejected"`,
an integer `revision` and an explicit `failure` (`forbidden`,
`stale_revision`, `not_found`, `unloaded`, `insufficient_items`, `capacity`,
`operation_conflict`, `runtime_unavailable`).

The `resident_equipment` and `resident_carry` endpoint kinds address one
durable resident handle's canonical 6 equipment slots and 8 carry slots. They
are part of the closed endpoint union and share the player endpoint's rules: the
actor session must be live, the resident must be owned by the calling plugin
(an absent resident returns `not_found`, a foreign one `forbidden`), every
participant fence is checked before the single recoverable commit, and the
transfer preserves durability and every component. A resident endpoint's fence
revision is its durable gear record revision, which advances on each commit and
also fences the resident's order/work assignments, so a concurrent order or a
stale transfer can never produce a second copy of gear an active order holds.
Both resident and player endpoints participate in the same world-inventory
decision, so a crash never leaves an item at two endpoints.

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
actor prepares every key first and holds the canonical player-state lock and
server save coordinator while appending and synchronizing one inventory decision
in the existing world journal. It durably projects the storage batch and
playerdata before replacing live inventory and publishing one ordered reliable
authoritative snapshot. Concurrent inventory
operations therefore cannot observe or interleave half of a successful runtime
transaction. A per-session lifetime gate makes disconnect either reject a
captured-but-not-started request or wait for an already-started commit before
the disconnected player state becomes saveable. Every storage record changed
by the batch receives the same revision.

The world decision contains the ledger mutations, canonical named-item inventory
NBT after-image and player UUID. Plugin-storage contains only its ledger
projection, not a second inventory recovery authority. Startup replays world
chunks, then restores storage and playerdata before admitting gameplay or saves,
including when Lua is disabled. Playerdata uses `SolarisInventoryWorldJournalLsn`;
this is a world journal LSN, not a plugin-storage revision.

The after-image includes the cursor and crafting/enchanting/merchant inputs.
Normal saves preserve newer recovered inventory while still saving unrelated
player state; a later inventory save at the same LSN is not overwritten by
replay. World checkpoints retain a decision until every durable projection is
complete. Startup reconstructs that readiness by replay, so recovered decisions
can subsequently be checkpointed.

Process-crash coverage kills the process after world sync with no storage
projection, after a storage append with uncertain sync, and after complete
projection. It verifies recovery, preservation of a later independent inventory
save, and checkpoint removal after recovery. An uncertain decision or failed
projection fences the player's inventory and signals the existing world
fail-stop path. Native item/container, pickup, survival, and plugin owner commits
check the inventory fence before effects. The uncertain transaction is not
reported as rejected or assumed committed.

The existing boolean callback is unchanged: this closes its ledger/playerdata
recovery gap, not every gameplay persistence transaction, and does not add
operation-id receipts to the older `inventory_storage_transaction` signature.
Owned inventory transfers/reservations and resident integration remain separate
work. Canonical container stacks now retain supported custom names and item
models alongside damage/enchantments through container moves and Anvil saves.

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

Plugins declaring `world_time` may submit one typed authoritative clock mutation:

```luau
solaris.set_world_time("market-night", 13000)
```

`request_id` follows the normal 64-byte script-id rule. `world_time` is an integer in
`0..=9_007_199_254_740_991` (`2^53 - 1`), the largest range in which every integer is
represented exactly by the Luau number type; larger Rust/Luau requests are rejected
rather than rounded. The router never writes the session clock directly: it
submits the request through the server-owned simulation command lane used by other
clock mutations. Success publishes targeted `world.time_set_result` with the exact
`request_id`, committed `world_time`, and `failure = nil`. Capacity pressure reports
`busy`; closed/stopped/shutting-down/unavailable simulation reports
`runtime_unavailable`; other rejected owner outcomes report `rejected`. The result is
an owner outcome, not a socket-delivery or client-render confirmation. An undeclared
`world_time` call traps synchronously before a command enters the host batch.

This is a bounded common-world API primitive, not a generic mutable-world handle.
Plugins still receive no world storage, clock registry, simulation owner, lock, region,
or scheduler reference.

Plugins declaring `world_blocks` may replace one root block with the registry default
state of one namespaced block id:

```luau
solaris.set_block("market-stone", "minecraft:overworld", "minecraft:stone", 1, 64, 1)
```

The first API is intentionally closed: `dimension` must be `minecraft:overworld`, the
block id must exist in the active registry, and the request selects only that block's
default state. Arbitrary block-state properties, block entities, cross-dimension writes,
and unbounded edit batches are not exposed. Y must remain inside the server world
height, while X/Z must remain inside the existing ±30,000,000 script horizontal
coordinate limit. `request_id` follows the normal 64-byte script-id rule.

After host capability/provenance admission, the router resolves the default state and
submits one server-owned block edit through the existing simulation-owner command lane;
it never acquires the world lock directly. Success publishes targeted
`world.block_set_result`. `applied = true` means the simulation owner accepted and
applied the requested block-edit batch, including an idempotent same-state write; it is
not a claim that the prior state differed. Success is exactly `applied = true` with
`failure = nil`; every rejection is exactly `applied = false` with a non-nil failure, so
there is no ambiguous `false`/`nil` result. Validation failures use `unknown_block`,
`unsupported_dimension`, or `out_of_world`; owner capacity pressure uses `busy`;
closed/stopped/unavailable runtime uses `runtime_unavailable`; other rejected owner
outcomes use `rejected`. The result is an owner outcome, not confirmation that every
client rendered the block or received its packets. An undeclared `world_blocks` call
traps before entering the host batch.

## Ownership Routing

Luau commands never contain region keys, leases, epochs, locks, sockets, or
worker handles. The server resolves ownership after admitting the bounded DTO:

- entity spawn, world-time mutation, and default-state world-block mutation enter the simulation owner;
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

`../solaris-default-plugins/basic-economy` uses one configurable physical item, such as
emeralds or gold ingots, as currency. Entering its configured cuboid opens a
server-owned inventory shop; `/economy` opens the same shop manually. A primary
click removes currency, grants the product, and advances the durable refund
ledger in one `inventory_storage_transaction`. A secondary click atomically
refunds only purchases recorded by this shop. Insufficient currency, a full
inventory, stale storage, or a concurrent purchase rejects the whole mutation.
`config.toml` documents the currency item and labels, zone, and products beside
the values an operator edits. Player-to-player payment, auctions, and multiple
simultaneous currencies are intentionally outside this basic plugin.

`../solaris-default-plugins/land-claims` provides `/claim status`, `/claim create`, and
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
`release_villager_binding` drops a previously claimed binding token from both
the adapter ownership map and the regional entity owner, so the bound villager
becomes claimable again immediately instead of lingering until its
simulation-tick expiry. A missing, expired, or foreign token returns `failure =
"binding_unavailable"`; temporary owner pressure returns `failure = "busy"`.
The result carries no goal because no new goal is installed.

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

Event dispatch has one aggregate 50 ms wall deadline, divided fairly among
remaining plugins with a maximum 10 ms slice per plugin. Interrupt checks
enforce the slice alongside instruction fuel; this is not a hard real-time
OS scheduling guarantee. All VMs share one host thread, so expensive handlers
can still consume its bounded event budget and delay other plugin work.
The 16 MiB limit covers Luau-managed memory, not total process RSS or every
native DTO allocation. A large plugin count multiplies per-VM memory.

This is language/authority isolation **inside the server process**, not a
process sandbox. Separate VMs, restricted libraries, declared capabilities,
plugin-owned result routing, and one-shot command admission protect ordinary
script boundaries. They do not contain a defect in the embedded VM/native
runtime or guarantee zero CPU contention with the kernel.

For low overhead, subscribe only to needed events, cache validated
`solaris.config()` data at load, prefer simulation timers over `server.tick`
handlers, and issue bounded queries/mutations only when gameplay needs them.
Startup rules are materialized once and introduce no Luau callback per block
or entity tick. None of these limits establishes an unmeasured whole-server
latency or memory percentage; profile the actual selected packages and load.

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

See [the contract examples](../../solaris-default-plugins/) for the configurable
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
