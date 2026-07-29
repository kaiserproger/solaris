# Solaris Luau Addon API 1.0

Status: future architecture specification, implementation blocked by the vanilla-parity gate  
Date: 2026-07-29  
Target game/protocol line: Minecraft Java 26.1.x  
Current production plugin API: `0.6.0`

This document defines the target addon platform that may follow the current
bounded server-side plugin API. It is not an implementation promise for the
current milestone and does not authorize work ahead of the scoped vanilla
26.1.2 overworld-survival parity target.

The implementation backlog is owned by
[`LUAU_ADDON_API_1_0_TASKS.md`](LUAU_ADDON_API_1_0_TASKS.md). Every runtime task
there remains blocked until the owner explicitly opens the addon-platform gate
after vanilla parity.

## 1. Purpose

Solaris addons must be able to implement both sides of a Minecraft extension:

- complete server-owned gameplay and domain logic;
- client content registration and presentation;
- custom items, blocks, block entities, entities and mobs;
- textures, models, animations, sounds and particles;
- custom screens, menus, HUD elements and world overlays;
- optional bounded custom rendering;
- dimensions, biomes, structures, portals and world generation;
- economies, permissions, towns, kingdoms, claims and diplomacy;
- NPC professions, work orders, logistics, research and settlements;
- armies, squads, formations, abilities, bosses and progression;
- mechanical, item, fluid, energy and signal networks;
- moving assemblies and mounted storage;
- typed interoperability with other addons.

The platform is intended to cover the architectural classes represented by
Create, MineColonies, Twilight Forest, Recruits, EssentialsX, Towny,
KingdomsX, Vault, iConomy-class economies, LuckPerms, Citizens, MythicMobs and
WorldEdit without moving those domain nouns into Rust.

## 2. Governing boundary

### 2.1. Luau owns game meaning

Luau owns:

- rules and policies;
- domain vocabulary;
- addon state machines;
- economy and progression formulas;
- settlement, town, nation and kingdom semantics;
- NPC roles, orders, schedules and work selection;
- machine recipes and production policy;
- ability composition and boss phases;
- UI behavior and client presentation state;
- content selection and composition;
- service-provider behavior;
- addon-to-addon contracts.

### 2.2. Rust and Solaris Loader own authoritative mechanics

Rust and the Loader own:

- world and entity storage;
- ECS implementation and regional ownership;
- collision, physics and pathfinding;
- transaction coordination and journals;
- inventory and container commits;
- chunk installation and deterministic generation execution;
- registry implementation and runtime ID projection;
- packet encoding, connection tracking and publication scopes;
- client package verification and bootstrapping;
- renderer integration and GPU command submission;
- thread scheduling, quotas and fault isolation;
- persistence, migrations and crash recovery primitives;
- graph execution and bounded simulation solvers.

### 2.3. Architectural correctness test

If a public Rust API contains a specific addon-domain noun such as `colony`,
`kingdom`, `home_order`, `turret`, `research` or `kinetic_press`, the boundary
is probably wrong.

The expected Rust vocabulary is generic:

- `persistent_entity`;
- `component`;
- `region_policy`;
- `goal`;
- `work_request`;
- `graph_network`;
- `transaction`;
- `structure_plan`;
- `render_feature`;
- `typed_service`.

Complete Luau logic does not mean direct access to Rust objects. Luau expresses
validated intent; the engine performs authoritative, bounded, replayable work.

## 3. Reference ecosystem lessons

The platform design uses the following projects as architectural pressure tests:

- Create: interacting world machinery, kinetic networks, stress propagation,
  mounted storage, moving contraptions, rich animation and Ponder-like visual
  documentation.
- MineColonies: persistent citizens, many professions, buildings, schematics,
  requests, warehouses, couriers, research, colony management and custom UI.
- Twilight Forest: a complete dimension, portals, biome and structure graphs,
  bosses, loot, progression locks, client sky/fog/music and custom entities.
- Recruits: persistent owned NPCs, squads, formations, orders, combat roles and
  command interfaces.
- EssentialsX: commands, homes, warps, kits, teleport requests, chat, economy
  integration, offline player state and permission checks.
- Towny and KingdomsX: regions, nested social groups, ranks, taxes, banks,
  diplomacy, war, structures, NPC defenders and map projections.
- Vault and iConomy-class plugins: replaceable economy/permission/chat providers
  behind stable service contracts.
- LuckPerms: contextual permissions, offline subjects, immutable query snapshots
  and asynchronously persisted state.
- Citizens: stable logical NPC identities with composable traits, navigation,
  events and persistence.
- MythicMobs: reusable triggers, conditions, targeters, mechanics, effects and
  multi-phase encounters.
- WorldEdit: regions, masks, patterns, clipboards, operation plans, progress,
  cancellation and undo.

Reference repositories and documentation:

- <https://github.com/Creators-of-Create/Create>
- <https://github.com/ldtteam/minecolonies>
- <https://github.com/TeamTwilight/twilightforest>
- <https://github.com/talhanation/recruits>
- <https://github.com/EssentialsX/Essentials>
- <https://github.com/TownyAdvanced/Towny>
- <https://github.com/CryptoMorin/KingdomsX>
- <https://github.com/MilkBowl/VaultAPI>
- <https://github.com/iConomy/Core>
- <https://github.com/LuckPerms/LuckPerms>
- <https://github.com/CitizensDev/Citizens2>
- <https://github.com/MythicCraft/MythicMobs>
- <https://github.com/EngineHub/WorldEdit>

The references are compatibility targets and sources of design pressure, not
code dependencies.

## 4. Addon package

### 4.1. Directory layout

```text
example-addon/
|-- addon.toml
|-- config.schema.toml
|-- config.toml
|-- shared/
|   |-- init.luau
|   |-- types.luau
|   `-- protocol.luau
|-- server/
|   |-- register.luau
|   |-- main.luau
|   |-- migrations/
|   `-- tests/
|-- client/
|   |-- register.luau
|   |-- main.luau
|   |-- screens/
|   `-- tests/
|-- data/
|   |-- items/
|   |-- blocks/
|   |-- entities/
|   |-- recipes/
|   |-- loot/
|   |-- abilities/
|   |-- structures/
|   |-- worldgen/
|   `-- guides/
`-- assets/
    `-- example/
        |-- textures/
        |-- models/
        |-- animations/
        |-- sounds/
        |-- particles/
        |-- lang/
        `-- materials/
```

`shared/` is side-neutral and may import only shared schemas and pure modules.
`server/register.luau` and `client/register.luau` run in registry phases.
`server/main.luau` and `client/main.luau` run after the relevant registries and
services are ready.

### 4.2. Manifest example

```toml
schema = 1
id = "example.industrial"
name = "Industrial Example"
version = "1.4.0"
api = "1.0"
description = "Mechanical production addon"

[entrypoints]
shared = "shared/init.luau"
server_register = "server/register.luau"
server = "server/main.luau"
client_register = "client/register.luau"
client = "client/main.luau"

[activation]
server = "required"
client = "required"
client_registration = "boot"
world_persistent = true

[compatibility]
minecraft = "26.1.x"
world_schema = 3
network_schema = 2

[[dependencies]]
id = "solaris.permissions"
version = "^1.0"
relation = "optional"

[[services.provides]]
id = "example.industrial:kinetic_network"
version = "1.0"

[capabilities.server]
storage = ["addon", "world"]
world_read = ["*"]
world_edit = ["example.industrial:*"]
entity_spawn = ["example.industrial:*"]
entity_control = ["example.industrial:*"]
network = ["example.industrial:*"]
services = ["provide", "consume"]

[capabilities.client]
registries = ["items", "blocks", "entities"]
assets = ["textures", "models", "animations", "sounds"]
ui = ["screens", "hud", "world_overlay"]
render = ["entities", "block_entities", "particles", "instancing"]
input = ["keybinds"]
network = ["example.industrial:*"]
```

### 4.3. Namespaces and durable identifiers

Every durable resource uses a canonical namespaced identifier:

```text
addon-id:path
```

Numeric registry, packet and ECS IDs are runtime projections. They must never
be written to addon storage, world files or durable network payloads.

Addon IDs, local paths, schema names, channel names and service names are
validated and bounded before any registry or runtime allocation.

## 5. Client content modes

Minecraft registries freeze before an ordinary server connection can deliver
arbitrary native content. Solaris therefore needs two explicit content modes.

### 5.1. Dynamic mode

```toml
client_registration = "dynamic"
```

The addon is downloaded and activated while joining the server without a client
restart. It uses Solaris-owned generic carriers and virtual runtime types:

- virtual items;
- virtual blocks and block entities;
- virtual entities and projectiles;
- virtual particles;
- custom screens and HUD;
- renderable world objects;
- models, textures, sounds and animations.

Each connection receives an exact runtime projection. World state stores only
canonical addon IDs. Dynamic mode should cover the majority of server-specific
content and lightweight content mods.

### 5.2. Boot mode

```toml
client_registration = "boot"
```

Boot mode is required when the addon needs real pre-freeze client registration,
including advanced entity types, fluids, dimension effects, render layers,
materials or model pipelines.

Connection flow:

1. the server sends the exact package identity and capability set;
2. Loader downloads and verifies the artifact;
3. the client authorizes the requested capabilities;
4. Loader requests a restart;
5. the cached addon registers before registry freeze on the next launch;
6. the client reconnects with an exact registry fingerprint;
7. the server rejects any mismatched fingerprint.

Boot addons still contain only Luau, declarative data and assets. Arbitrary Java
bytecode or native libraries are outside the API.

### 5.3. Client requirement policy

An addon declares one policy:

- `required`: exact client addon is required to join;
- `optional`: server mechanics work without the client package;
- `cosmetic`: client absence changes presentation only;
- `vanilla_fallback`: the addon provides explicit vanilla projections.

Fallback behavior must be declared and testable. The server may not silently
pretend unsupported custom content is vanilla-equivalent.

## 6. Lifecycle

### 6.1. Startup phases

1. `discover`
2. `resolve_dependencies`
3. `validate_package`
4. `register_shared_schemas`
5. `register_server_content`
6. `register_client_content`
7. `freeze_registries`
8. `open_world`
9. `run_migrations`
10. `enable_services`
11. `start_runtime`
12. `accept_players`

Static registration after `freeze_registries` is rejected.

### 6.2. Runtime callbacks

```luau
function addon.on_enable(context) end
function addon.on_world_open(event) end
function addon.on_player_ready(event) end
function addon.on_config_reload(event) end
function addon.on_disable(reason) end
```

Callbacks receive immutable snapshots and capability-scoped handles.

### 6.3. Reload classes

- `config`: validate and atomically replace the immutable config snapshot;
- `logic`: restart the VM with a bounded handoff state and no registry changes;
- `assets`: perform a client resource reload;
- `registry/worldgen`: restart-only.

A failed reload leaves the previous working version active.

## 7. Runtime programming model

### 7.1. Immutable snapshots

```luau
type EntitySnapshot = {
    id: EntityHandle,
    revision: number,
    type_id: ResourceId,
    dimension: ResourceId,
    position: Vec3,
    velocity: Vec3,
    components: {[ResourceId]: any},
}
```

Snapshots contain no session object, region owner, ECS pointer, lock, socket or
worker reference.

### 7.2. Opaque handles

```luau
type EntityHandle = {
    token: string,
    generation: number,
}
```

Handles may be session-scoped, lease-based or persistent logical identifiers.
Every mutable operation revalidates ownership, generation and capability.

### 7.3. Futures

Cross-owner work returns a typed future:

```luau
local result = await(entity.move_to(entity_id, target, options))
```

A future has cancellation, deadline and typed failure. Synchronous policy hooks
may not await.

### 7.4. Scheduling instead of universal tick callbacks

```luau
scheduler.after_ticks(20, "heal_check", payload)
scheduler.every_ticks(100, "tax_collection", options)
scheduler.at_game_time("06:00", "morning_shift")
```

Supported wakeup sources include timers, component changes, spatial
subscriptions, network dirtiness, work-request transitions and retained jobs.
Persistent timers and jobs survive restart where declared.

`server.tick` remains a diagnostic or tightly bounded escape hatch, not the
normal way to implement every object.

## 8. Events, policies and transforms

### 8.1. Post-commit facts

```luau
events.observe("player.item_crafted", handler)
events.observe("entity.died", handler)
events.observe("world.block_changed", handler)
```

Facts are immutable and published only after the authoritative commit.

### 8.2. Pre-commit policy hooks

```luau
events.policy("world.block_break", function(event)
    if claims:is_protected(event.actor, event.position) then
        return Policy.deny("claim.protected")
    end

    return Policy.allow()
end)
```

A policy hook:

- runs with a small deterministic fuel budget;
- may not await or perform I/O;
- receives a complete authoritative snapshot for its decision;
- returns `allow`, `deny` or a bounded typed modification;
- cannot commit world state directly.

### 8.3. Transform hooks

```luau
events.transform("loot.plan", function(plan)
    return plan:add("example:token", 1)
end)
```

A transform changes a proposed plan. The engine validates the complete result
before commit.

### 8.4. Typed results

Every command returns a structured result:

```luau
{
    committed = false,
    failure = "permission_denied",
    conflicting_revision = 481,
}
```

Public API 1.0 does not use an unexplained boolean failure.

## 9. Transactions

### 9.1. General plan

```luau
local plan = transaction.plan({
    actor = player,
    idempotency_key = "shop:" .. order_id,
})

plan:require_inventory(player, {
    item = "minecraft:emerald",
    count = 10,
})
plan:debit_item(player, "minecraft:emerald", 10)
plan:credit_item(player, "example:machine", 1)
plan:storage_cas(
    storage.table("orders"),
    order_id,
    expected_revision,
    next_order
)

local result = await(transaction.commit(plan))
```

A plan may combine:

- player inventories and cursors;
- containers and addon-owned inventories;
- addon storage;
- economy accounts;
- world block edits;
- entity components;
- permission/group state;
- region policy state;
- scheduled jobs.

The engine determines owners, orders locks, validates revisions, writes the
journal, commits and publishes. Luau never holds a lock.

### 9.2. Bulk world edits

```luau
local edit = world.edit_session({
    dimension = "minecraft:overworld",
    actor = player,
    max_blocks = 100000,
    history = "player",
})

edit:set_region(region, pattern, mask)
edit:paste(schematic, transform)
edit:replace(region, from_mask, to_pattern)

local result = await(edit:commit())
```

Bulk edits require limits, masks, patterns, progress, cancellation, revision
checks and an optional undo token. They are plans, not arbitrary block loops.

## 10. Storage

The storage layer supports four bounded forms:

- key/value records;
- typed indexed tables;
- typed documents;
- content-addressed blobs.

Example:

```luau
storage.define_table("towns", {
    primary_key = "id",
    schema = TownSchema,
    indexes = {
        {"name", unique = true},
        {"nation_id"},
        {"owner_uuid"},
    },
})
```

Required properties:

- schema fingerprints;
- migrations;
- optimistic revisions;
- transactions;
- indexes and cursor pagination;
- offline player records;
- quotas;
- deterministic serialization;
- backup and export;
- bounded change streams.

Raw SQL is not public addon API.

## 11. Service registry

Service discovery is part of Solaris core rather than a mandatory Vault-like
bridge addon.

```luau
services.define("solaris:economy", {
    version = "1.0",
    methods = {
        balance = schema.fn(PlayerId, Money),
        transfer = schema.async_fn(TransferRequest, TransferResult),
    },
})

services.provide("solaris:economy@1", EconomyProvider, {
    priority = 100,
})

local economy = services.require("solaris:economy@^1")
```

Standard service families:

- `solaris:permissions`;
- `solaris:economy`;
- `solaris:chat`;
- `solaris:identity`;
- `solaris:groups`;
- `solaris:regions`;
- `solaris:placeholders`;
- `solaris:maps`;
- `solaris:quests`;
- `solaris:party`;
- `solaris:mail`.

Multiple providers are resolved by version, operator preference, priority and
feature compatibility. Cycles fail before runtime.

## 12. Permissions

```luau
permissions.register("example.machine.use", {
    description = "Allows machine interaction",
    default = "member",
})

local decision = permissions.check(player, "example.machine.use", {
    dimension = event.dimension,
    region = event.region,
    kingdom_relation = "ally",
})
```

Permission results are tri-state:

- `allow`;
- `deny`;
- `undefined`.

Addons may register bounded context providers. The engine caches contexts and
invalidates them from explicit state changes instead of recomputing every
context on every tick.

Permissions must support online and offline subjects, group inheritance,
contextual resolution, audit traces and immutable query snapshots.

## 13. Commands

```luau
commands.register("town", {
    permission = "town.command",
    arguments = {
        commands.literal("create")
            :then(commands.string("name", {
                min = 3,
                max = 24,
                suggestions = suggest_town_names,
            }))
            :executes(create_town),

        commands.literal("claim")
            :then(commands.enum("mode", {
                "current",
                "square",
                "fill",
            }))
            :executes(claim_land),
    },
})
```

The command API supports typed trees, player/console sources, localized errors,
permissions, cooldowns, suggestions, help generation, audit logs and async
execution.

## 14. Content registries

### 14.1. Items

```luau
registry.items.define("example:command_staff", {
    stack_size = 1,
    durability = 512,
    rarity = "rare",

    components = {
        ["solaris:tool"] = {
            mining_speed = 4.0,
        },
        ["example:staff_mode"] = {
            schema = StaffModeSchema,
            default = {mode = "follow"},
        },
    },

    use_action = "example:command_recruits",
    client = {
        model = "example:item/command_staff",
        tooltip = "example.item.command_staff.tooltip",
    },
})
```

Required item features:

- custom typed data components;
- stack and durability rules;
- equipment, food, projectiles and tools;
- use actions and cooldown groups;
- enchantment compatibility;
- model predicates;
- recipes, loot and creative groups;
- capability ports;
- server-authoritative use validation.

### 14.2. Blocks

```luau
registry.blocks.define("example:kinetic_press", {
    properties = {
        facing = block.property.direction4(),
        powered = block.property.boolean(false),
        progress = block.property.int(0, 7, 0),
    },

    hardness = 4.0,
    resistance = 12.0,
    collision = "example:kinetic_press",
    block_entity = "example:kinetic_press_state",

    ports = {
        {type = "example:rotation", face = "back"},
        {type = "solaris:item_input", face = "top"},
        {type = "solaris:item_output", face = "bottom"},
    },

    client = {
        model = "example:block/kinetic_press",
        renderer = "example:kinetic_press_renderer",
    },
})
```

Required block features:

- typed state properties;
- collision, selection and occlusion shapes;
- light, hardness and resistance;
- block entities and inventories;
- redstone, fluid and generic ports;
- random and scheduled wakeups;
- multiblocks;
- placement, use and break actions;
- loot;
- dynamic rendering;
- worldgen inclusion;
- movement/assembly rules.

### 14.3. Block entities

```luau
registry.block_entities.define("example:kinetic_press_state", {
    schema = {
        inventory = component.inventory(4),
        progress = component.u16(),
        network_node = component.reference("example:rotation_node"),
    },

    persistence = "world",
    replication = {
        progress = "tracking",
        inventory = "owner_screen",
    },
})
```

Block entities wake from explicit causes: input change, graph update, scheduled
work, interaction, chunk load or recipe completion. A universal 20 Hz Luau
callback per block entity is not the primary model.

## 15. Entities, components and persistent identities

### 15.1. Entity archetype

```luau
registry.entities.define("example:recruit", {
    category = "creature",
    dimensions = {width = 0.6, height = 1.95},
    tracking_range = 96,

    components = {
        ["solaris:health"] = {max = 20},
        ["solaris:movement"] = {speed = 0.3},
        ["solaris:navigation"] = {type = "ground"},
        ["example:owner"] = OwnerSchema,
        ["example:squad"] = SquadSchema,
        ["example:role"] = RoleSchema,
    },

    ai = "example:recruit_brain",

    client = {
        model = "example:entity/recruit",
        texture = "example:textures/entity/recruit.png",
        animation = "example:animations/recruit.animation.json",
        renderer = "example:humanoid",
    },
})
```

### 15.2. Components and traits

```luau
entity.attach(entity_id, "example:guard", {
    defend_region = region_id,
})

entity.attach(entity_id, "example:trader", {
    catalog = "example:weapons",
})
```

Traits are schema-owned components with explicit persistence, replication and
mutation policy. One entity may compose multiple independent traits.

### 15.3. Persistent logical entities

A persistent NPC uses a stable logical ID independent of the current ECS and
wire entity IDs:

```text
example:citizen/8dc7...
```

The logical record survives unload, representation despawn, restart, owner
migration and runtime-ID changes. Materialization into an active ECS entity is
an engine operation.

## 16. AI

### 16.1. Navigation

```luau
local result = await(ai.navigate(entity, {
    target = position,
    speed = 0.35,
    tolerance = 1.5,
    doors = "open",
    avoid = {"example:danger_zone"},
}))
```

Rust owns pathfinding and movement admission. Luau owns goal selection and
response to typed navigation results.

### 16.2. Behavior trees

```luau
ai.behavior_tree.define("example:guard_brain",
    ai.selector({
        ai.sequence({
            ai.condition("example:has_attack_target"),
            ai.task("example:attack_target"),
        }),
        ai.sequence({
            ai.condition("example:order_is_hold"),
            ai.task("solaris:idle"),
        }),
        ai.task("example:patrol"),
    })
)
```

### 16.3. Utility AI

```luau
ai.utility.define("example:citizen_daily_routine", {
    considerations = {
        "example:hunger",
        "example:work_due",
        "example:danger",
        "example:sleep_need",
    },
})
```

### 16.4. Memory and blackboards

Memory has a schema, scope and optional TTL. Supported scopes include entity,
squad, settlement, encounter and navigation. Addons do not receive raw access
to vanilla brain internals.

### 16.5. Sensors

```luau
ai.sensor.define("example:nearby_enemy", {
    cadence_ticks = 10,
    query = entity.query({
        radius = 24,
        components = {"solaris:living"},
    }),
})
```

Sensors execute through bounded spatial indexes and publish changes instead of
forcing every brain to rescan the world.

### 16.6. Squads and formations

```luau
squads.define_formation("example:shield_wall", {
    slots = formation.grid(5, 2, 1.2),
    facing = "leader_look",
})
```

The engine assigns slots, handles avoidance and path requests, migrates owners
and replicates motion. Luau owns order semantics and formation transitions.

## 17. Abilities and encounters

```luau
abilities.define("example:fire_nova", {
    triggers = {
        ability.trigger.health_below(0.5),
        ability.trigger.cooldown_ready(),
    },

    conditions = {
        ability.condition.has_target(),
    },

    targeter = ability.targeter.entities_in_radius(8),

    actions = {
        ability.effect.particle("example:fire_ring"),
        ability.effect.damage({
            amount = 6,
            type = "example:arcane_fire",
        }),
        ability.effect.knockback(1.4),
    },

    cooldown = 200,
})
```

The ability system supports triggers, conditions, targeters, mechanics,
effects, cast phases, interruption, costs, channeling, projectiles, cooldowns,
boss bars and phase transitions. Damage and hit validation remain server
authoritative.

## 18. Work, requests, logistics and research

### 18.1. Work orders

```luau
work.define_type("example:build_structure", BuildOrderSchema)
```

Durable states:

- `queued`;
- `reserved`;
- `active`;
- `blocked`;
- `completed`;
- `cancelled`.

### 18.2. Resource requests

```luau
requests.create({
    requester = citizen_id,
    resource = item.matcher("#minecraft:logs"),
    count = 64,
    destination = warehouse_port,
    priority = 50,
})
```

The engine provides durable queues, reservation, item matching, route planning,
transactional transfer, dependencies and cancellation. Luau decides who
requests what, priorities, substitutions and shortage policy.

### 18.3. Research

```luau
research.define("example:improved_tools", {
    branch = "example:industry",
    prerequisites = {"example:basic_smelting"},
    requirements = {
        building_level("example:university", 2),
        item_cost("minecraft:iron_ingot", 16),
    },
    effects = {
        modifier("example:worker_speed", 0.1),
        unlock("example:steel_tools"),
    },
})
```

Research effects are typed modifiers and unlocks, not arbitrary writes into
another addon.

## 19. Regions, towns and kingdoms

```luau
local region = regions.create({
    id = "example:town/" .. town_id,
    geometry = regions.chunk_set(chunks),
    dimension = dimension,
    owner = group_id,
})

regions.set_policy(region, {
    ["world.block_break"] = policy.roles({
        owner = true,
        member = true,
        ally = false,
        stranger = false,
    }),

    ["container.open"] = policy.permission("example.town.container"),
    ["pvp.damage"] = policy.relation_matrix("example:diplomacy"),
})
```

The region core supports:

- chunk sets, cuboids and bounded polygons;
- nested regions, priorities and inheritance;
- role and permission bindings;
- relation contexts;
- policy snapshots for ordinary gameplay commits;
- taxes and upkeep hooks;
- temporary conflict overrides;
- enter/exit subscriptions;
- map projections.

`town`, `nation`, `kingdom`, `outpost`, `siege` and `war` remain addon concepts.

## 20. Mechanical and logistics networks

### 20.1. Network type

```luau
simulation.network.define("example:rotation", {
    value_schema = {
        speed = schema.f64(),
        direction = schema.i8(),
        stress = schema.f64(),
        capacity = schema.f64(),
    },

    topology = "block_ports",
    solver = "solaris:scalar_constraint",
})
```

### 20.2. Node semantics

```luau
simulation.network.node_type("example:shaft", {
    ports = {
        {face = "axis_positive"},
        {face = "axis_negative"},
    },

    transfer = function(input, state)
        return input
    end,
})
```

Rust owns topology discovery, connected components, partitioning, dirty
propagation, bounded solving, cross-chunk ownership, snapshots and replication.
Luau owns node types, transfer semantics, overload policy, recipes and failure
behavior.

Built-in graph families should include:

- scalar constraint flow;
- directed item routing;
- fluid volume and pressure;
- energy capacity;
- torque and stress;
- signal propagation;
- reservation networks;
- dependency graphs.

Small bounded graphs may use pure Luau solvers.

## 21. Moving assemblies

```luau
local result = await(assemblies.assemble({
    origin = bearing_position,
    selector = assembly.connected_blocks({
        max_blocks = 2048,
        include_tags = {"example:movable"},
    }),
    transform = "rotating",
    storage_policy = "mounted",
}))
```

The engine owns atomic extraction, state preservation, collision broadphase,
movement, riding, owner migration, reassembly, publication and batched
rendering. Luau owns assembly admission, mounted capability behavior, controls,
collision policy and disassembly rules.

## 22. Dimensions and world generation

### 22.1. Dimension descriptor

```luau
registry.dimensions.define("example:twilight_realm", {
    type = {
        fixed_time = 13000,
        skylight = true,
        ceiling = false,
        coordinate_scale = 1.0,
        ambient_light = 0.25,
    },

    generator = "example:twilight_generator",
    biome_source = "example:twilight_biomes",

    client_effects = {
        sky = "example:twilight_sky",
        fog = "example:blue_fog",
        music = "example:twilight_ambient",
    },
})
```

### 22.2. Declarative graph

The normal worldgen path is data-driven and supports noise, density functions,
surface rules, carvers, features, biome placement, structure sets, template
pools, processors, loot and spawn rules.

### 22.3. Pure Luau kernels

```luau
worldgen.feature.define("example:giant_tree", {
    max_blocks = 12000,

    generate = function(context)
        local rng = context:rng("giant_tree")
        return GiantTree.generate(context.origin, rng)
    end,
})
```

Worldgen kernels are deterministic pure functions with seeded RNG, bounded
input/output, no storage/network/time access, no mutable global state and a
strict fuel/memory budget. The callback operates on a chunk or region batch and
returns a placement plan; it is not invoked once per block.

### 22.4. World fingerprint

World metadata stores exact addon package identities, registry fingerprints,
dimension descriptors, worldgen graph hashes and structure schema versions.
An incompatible change requires a migration, a fresh world or an explicit
operator override.

## 23. Portals and progression

```luau
portals.define("example:twilight_portal", {
    frame = block.matcher({
        tags = {"minecraft:dirt", "minecraft:grass_blocks"},
    }),
    interior = block.matcher("minecraft:water"),
    activation = function(context)
        return context:has_surrounding_tag("minecraft:flowers")
    end,
    destination = "example:twilight_realm",
})
```

Progression restrictions are normal policy hooks over structures, regions,
portals, loot or abilities. They must return typed denial reasons suitable for
server messages and client UI.

## 24. Structures and schematics

```luau
structures.register_template("example:guard_tower", {
    file = "data/structures/guard_tower.snbt",
    anchors = {
        entrance = "entrance",
        guard_spawn = "guard_spawn",
        storage = "storage",
    },
})
```

Required structure operations:

- scan and validation;
- preview;
- rotate and mirror;
- palette substitution;
- processors;
- material requirements;
- staged construction;
- progress and cancellation;
- undo;
- ownership;
- content-addressed caching;
- policy-controlled uploads.

A MineColonies-like builder receives work orders and bounded edit plans, not an
unrestricted world pointer.

## 25. Typed networking

### 25.1. Channel definition

```luau
network.define("example:open_army_screen", {
    direction = "server_to_client",
    phase = "play",
    schema = ArmyScreenSchema,
    version = 2,
})
```

The codec is generated from the schema and bounded before allocation.

### 25.2. Delivery scopes

```luau
network.send(player, channel, payload)
network.send_tracking_entity(entity, channel, payload)
network.send_tracking_chunk(chunk, channel, payload)
network.broadcast_dimension(dimension, channel, payload)
```

Tracking scopes are mandatory for scalable content. An addon may not obtain the
raw packet writer.

### 25.3. Client input is untrusted

```luau
network.on_server("example:army_action", function(player, request)
    request_schema:validate(request)

    if not army:can_control(player, request.squad_id) then
        return
    end

    army:apply_order(request)
end)
```

A client never supplies authoritative damage, final payment state, entity
position or transaction success.

## 26. Client Luau runtime

Each addon runs in an isolated sandbox or equivalent isolated environment.
Client code has no filesystem, raw sockets, JVM reflection, native library
loading, raw OpenGL, other-server storage or undeclared cross-addon memory.

The client API may expose only capability-scoped access to:

- registries and activated content;
- verified assets;
- declarative rendering and command buffers;
- screens, HUD and world overlays;
- key bindings and bounded input;
- audio and particles;
- camera modifiers;
- typed networking;
- localization;
- addon-local storage;
- accessibility settings.

## 27. UI

### 27.1. Vanilla-compatible menus

Server-owned inventory menus remain available for optional-client and vanilla
fallback paths.

### 27.2. Custom screens

```luau
ui.register_screen("example:army", function(props, state)
    return ui.column({
        ui.text(props.title),
        ui.list(props.units, function(unit)
            return ui.row({
                ui.entity_preview(unit.entity),
                ui.text(unit.name),
                ui.button("Follow", function()
                    network.send("example:army_action", {
                        unit = unit.id,
                        order = "follow",
                    })
                end),
            })
        end),
    })
end)
```

The UI toolkit supports flex/grid layout, scrolling, virtualized lists, tabs,
item/block/entity previews, tooltips, text input, keyboard/controller
navigation, accessibility labels, localization, reactive state, animation,
server patches and optimistic presentation with authoritative result handling.

### 27.3. HUD and world overlays

```luau
ui.hud.register("example:stress_meter", {
    anchor = "top_right",
    render = render_stress_meter,
})
```

World overlays cover territory boundaries, machine graphs, path previews,
build previews, quest markers, damage indicators and selection shapes.

## 28. Rendering

### 28.1. Declarative path

The normal content path uses verified models, animation controllers, particles,
materials, render layers, emissive textures, transforms, attachments and
instancing.

### 28.2. Replicated render state

```luau
render.entities.register("example:recruit", {
    model = "example:recruit",
    state = {
        order = schema.enum({"idle", "follow", "attack"}),
        team_color = schema.color(),
        weapon = schema.resource_id(),
    },

    controller = function(state)
        if state.order == "attack" then
            return animation.play("example:recruit_attack")
        end
        return animation.play("example:recruit_idle")
    end,
})
```

The server replicates only the declared bounded render state.

### 28.3. Custom command buffer

```luau
render.world.on_submit(function(frame)
    frame:mesh(instance_mesh, transform, material)
    frame:line(start_pos, end_pos, style)
    frame:text(label, world_position, options)
end)
```

Luau records commands; it does not invoke the GPU directly. The Loader enforces
budgets for draw calls, vertices, instances, bytes, texture memory, material
complexity and submission time.

### 28.4. Feature registration points

- item renderer;
- block renderer;
- block entity renderer;
- entity renderer;
- armor and held-item layers;
- particles;
- world features;
- HUD layers;
- post effects;
- sky and fog controllers.

### 28.5. Shaders

API 1.0 should prefer a safe material graph. Arbitrary shader source, if ever
supported, requires a separate trusted capability and explicit client consent.

## 29. Animation and audio

```luau
animations.define("example:press_cycle", {
    length = 1.2,
    tracks = {
        piston = animation.position({...}),
        wheel = animation.rotation({...}),
    },
})

sounds.define("example:machine_loop", {
    files = {"example:sounds/machine_loop.ogg"},
    attenuation = 16,
    streaming = false,
})
```

Required features include state machines, blend trees, timeline events, bone
attachments, procedural transforms, positional loops, dynamic pitch/volume,
music zones, boss music and subtitles.

## 30. Ponder-like guides

```luau
guides.scene("example:kinetic_press", function(scene)
    scene:place_structure("example:press_demo")
    scene:show_text("example.guide.connect_shaft")
    scene:highlight_block("shaft")
    scene:rotate_network("rotation", 16)
    scene:show_item_transfer("input", "output")
end)
```

Guides support staged scenes, ghost structures, highlighting, arrows, localized
text, simulated transfers, camera paths, entity actors, interactive steps,
recipe links and automatic indexing.

## 31. Configuration

`config.schema.toml` declares types, defaults, bounds, enums, documentation,
reloadability, secret fields and client visibility.

Solaris generates validation, default config, operator documentation and an
optional operator UI. The addon receives an immutable snapshot. An invalid
reload cannot replace the last valid configuration.

## 32. Dependencies and feature negotiation

Dependencies support:

- `required`;
- `optional`;
- `load_before`;
- `conflicts`;
- semantic version ranges;
- feature requirements.

```toml
[[dependencies]]
id = "example.core"
version = ">=2.1 <3.0"
relation = "required"
features = ["logistics"]
```

Addons should query features rather than hard-code implementation names:

```luau
if platform.features:has("render.instancing.v2") then
    -- use the optimized path
end
```

## 33. Security, quotas and fault isolation

### 33.1. Server quotas

- Luau memory and instruction fuel;
- handler deadlines;
- event and future queues;
- storage and blob bytes;
- network bandwidth;
- world-edit volume;
- spatial-query cost;
- active and persistent entities;
- timers and jobs;
- logs and metrics labels.

### 33.2. Client quotas

- artifact and asset bytes;
- texture and mesh memory;
- UI nodes;
- draw calls and instances;
- particles;
- animation states;
- network messages;
- input bindings.

### 33.3. Failure behavior

A runtime failure aborts the staged command batch, ends the handler, charges the
addon error budget and may disable only that addon. It must not partially commit
world state or crash unrelated addons.

A world-critical addon may fail server startup if its absence would make stored
world content ambiguous.

## 34. Observability

```luau
metrics.counter("orders_completed")
metrics.histogram("path_request_ms")
metrics.gauge("active_citizens")
```

Solaris exposes per-addon CPU/fuel, memory, handler latency, queue depth,
transaction failures, storage size, network bytes, entity/component counts,
render cost and worldgen cost.

Operators need a bounded profiler such as:

```text
/addons profile example.industrial
```

## 35. Testing

### 35.1. Pure Luau tests

Pure modules, formulas, policies and state machines run without a world.

### 35.2. Server simulation tests

The harness provides deterministic clocks, fake players, worlds, storage,
transactions, permissions, regional ownership and entity snapshots.

### 35.3. Client tests

- UI snapshots;
- render-command snapshots;
- asset validation;
- input handling;
- typed-channel compatibility;
- resource reload and reconnect.

### 35.4. Wire and real-client tests

Required vertical paths include custom item, block, entity, screen, interaction,
resource reload, reconnect and capability denial. Dynamic and boot modes have
separate gates.

### 35.5. Replay

Authoritative input events, schema versions and RNG seeds may be captured for a
deterministic replay of addon behavior.

## 36. Upgrade and uninstall

An addon with durable content declares an uninstall policy:

```toml
[uninstall]
blocks = "replace_with:minecraft:air"
items = "convert_to:minecraft:paper"
entities = "despawn"
storage = "archive"
```

Production addons should provide migrations:

```luau
function migrate(context)
    context.items:rename("example:old_staff", "example:command_staff")
    context.components:transform("example:citizen", migrate_citizen)
end
```

The runtime may not silently forget a registry ID that still exists in world or
inventory state.

## 37. Required acceptance addons

API 1.0 is not complete merely because individual host functions exist. It must
support the following vertical reference addons.

### 37.1. Create-class addon

- at least 1,000 connected kinetic nodes;
- speed, direction, stress and capacity;
- recipe-driven machines;
- moving assembly;
- mounted inventory;
- tracking-scoped replication;
- client animation and instanced rendering;
- one Ponder-like scene.

### 37.2. MineColonies-class addon

- persistent colony records;
- at least 100 active citizens;
- professions and composable traits;
- validated buildings and schematics;
- durable work orders;
- resource requests, warehouses and couriers;
- research;
- management UI;
- restart and unload recovery.

### 37.3. Twilight Forest-class addon

- custom dimension and portal;
- biomes and structures;
- custom mobs;
- multi-phase boss;
- progression locks;
- custom loot;
- client sky, fog and music;
- world fingerprint enforcement.

### 37.4. Recruits-class addon

- recruitable persistent NPCs;
- ownership and permissions;
- squads and formations;
- follow, hold, patrol and attack orders;
- relations and PvP policy;
- command screen;
- synchronized models and animations.

### 37.5. Essentials-class addon

- homes, warps and teleport requests;
- kits and mail;
- chat formatting;
- economy provider;
- permission checks;
- offline player records.

### 37.6. Towny/Kingdoms-class addon

- claims and nested social groups;
- ranks and contextual permissions;
- banks, taxes and upkeep;
- nations, diplomacy and war states;
- addon-owned structures or defenders;
- map projections;
- cross-addon economy and permissions.

### 37.7. MythicMobs-class addon

- custom archetypes;
- triggers, conditions and targeters;
- reusable ability graphs;
- projectiles;
- boss phases;
- client effects;
- spawn conditions.

### 37.8. WorldEdit-class addon

- selections, masks and patterns;
- clipboard/schematic operations;
- bounded bulk commits;
- progress and cancellation;
- undo;
- permission and region policy integration.

## 38. Explicit non-goals

API 1.0 does not expose:

- direct ECS storage;
- Rust references;
- region locks;
- raw packet writers;
- persistent numeric registry IDs;
- direct world-file writes;
- SQL connections;
- unrestricted filesystem access;
- unrestricted HTTP or sockets;
- arbitrary Java or native code;
- raw OpenGL/Vulkan calls;
- memory belonging to another addon;
- bypasses around the transaction pipeline;
- client-declared authoritative state.

Bit-for-bit compatibility with Forge, Fabric or Bukkit internals is not a goal.
Behavioral capability and addon portability are the target.

## 39. Compatibility with Plugin API 0.6.0

The current API remains a deliberately bounded server-plugin contract. API 1.0
must not be grown through an unbounded sequence of special-case calls.

The migration direction is:

- current storage operations -> typed storage and transactions;
- current menu calls -> vanilla-compatible menu layer;
- current zone calls -> generic region policy;
- current player/villager commands -> entity goals and components;
- current Loader bundles -> dynamic/boot content packages;
- current targeted result events -> futures plus typed result events.

A compatibility adapter may host existing `0.6.0` plugins, but API 1.0 does not
need to preserve every old call as a first-class primitive.

## 40. Activation gate

No implementation task in this specification starts merely because this file
exists.

The addon-platform backlog may be activated only when all of the following are
true:

1. the scoped vanilla 26.1.2 overworld-survival parity gate is closed according
   to the project's canonical playable and validation documents;
2. no open common-gameplay blocker has higher priority;
3. the owner explicitly authorizes the first API 1.0 implementation milestone;
4. the selected milestone is a finite vertical slice with real wire/client
   evidence, not a broad framework rewrite.

Until then the active cursor remains the vanilla-parity queue in
[`playable/ACTIVE.md`](playable/ACTIVE.md), currently village defence and the
remaining species-specific mob loop.
