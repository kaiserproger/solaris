# Solaris core architecture

Status: new target architecture, owner-requested 2026-09-05. The current tree is
not fully migrated. This document owns the replacement design; implementation
state and evidence belong in the short memory cursor, not in competing plans.

## Acceptance contract

- Preserve Minecraft Java 26.1.2 protocol, ordinary gameplay invariants, and
  vanilla world-format compatibility. Use the local `.analysis/server.jar` and
  captured vanilla behavior as the oracle; do not infer packet layouts.
- Preserve item conservation, authoritative movement, lifecycle ordering,
  validated mutations, and the documented save/commit guarantees.
- Reduce mechanisms and duplicated authority, not merely line width or file
  size. Generated boilerplate and forwarding layers are still complexity.
- Do not turn a failed performance gate green by reducing its population,
  clients, tick count, coverage, or assertions. Setup failure is not a completed
  measurement. Debug and optimized results are different evidence.
- Solaris internals have no released compatibility obligation. Migrate every
  caller, then remove the replaced path; no permanent shims or dual authorities.

## Five responsibilities

| Responsibility | Owns | Must not own |
| --- | --- | --- |
| Immutable data | Validated registries, identifiers, recipes, loot, block/item facts, content definitions | Sessions, mutable world state, plugin execution |
| Spatial simulation | Players and other entities, blocks, inventories, local scheduled work, authoritative commits | Socket I/O, plugin callbacks, blocking disk I/O |
| Non-spatial services | Accounts, permissions, plugin storage and lifecycle, durable service transactions | A second spatial state or a second gameplay implementation |
| Adapters | Wire decoding/encoding, authentication, connection state, persistence I/O, CLI | Independent gameplay policy or authoritative inventory/world mirrors |
| Extensions | Sandboxed addon logic and declarative client presentation | Engine pointers, locks, ECS internals, raw filesystem/network/GPU access |

`mc-domain` remains the shared value layer. `mc-data`, `mc-world`, `mc-entity`,
`mc-physics`, and `mc-worldgen` retain their useful domain implementations.
The simulation/application boundary must be independently usable without TCP;
its final extraction may introduce one core crate, not one crate per verb.
`mc-net` becomes the connection adapter and `mc-server` the composition root.
A crate is added only when real migrated behavior gives it a responsibility.

Startup gameplay data now has one library-owned `mc_server::startup_data::StartupData`
assembly boundary. The CLI receives validated tables before terrain or world
preparation; malformed recipes and sidecars cannot partially initialize a world.
Per-table `Effective<T>` provenance wrappers are removed. `--check` retains its
configuration-validation and warning contract without opening a world.

The remaining obstacles are concrete: gameplay and simulation still live in
`mc-net`, and player authority is mixed with session bookkeeping. Addon payloads
now share the live `mc-script::ScriptBoundary`; the separate `mc-extension`
boundary is removed. Moving files without removing the remaining ownership
problems does not complete the redesign.

## One mutation model

```text
input -> bounded admission -> current owner -> validated commit -> effects
```

Network actions, addon commands, and scheduled game work use the same gameplay
kernels. An adapter supplies authenticated provenance, not a second mutation
implementation. The owner validates current generation/revision and permissions.
Rejected work changes nothing; committed effects are captured before fallible
external delivery. A response distinguishes owner commit from socket delivery
or client rendering.

Hotbar selection has no connection-owned mirror. Held-item gameplay reads the
owner-committed selection from shared player state; the connection retains only
pending action and delivery state for this path. Inventory/container projections
and the broader player/session ownership migration remain separate unfinished work.

Window-0 clicks, recipe-book crafting, offhand swaps and command grants now
prepare local inventory/cursor candidates and publish owner outcomes. The
connection's committed client baseline is retained for stale-transaction checks;
it is not overwritten with speculative candidates. Other menu handlers remain
outside this cutover.

Placement planning rejects collision shapes intersecting the placing player.
Rejected non-positional player-pose commits restore the last accepted pose rather
than tearing down the connection. Respawn chunk replay first switches its center
to the accepted respawn pose. Mining permits a changed state of the same block
within the same chunk instance and uninterrupted block identity, but captures the
current state/token for owner commit. Sparse per-block revisions also retain the
last different-type replacement revision, so leaves→air/stone→leaves cannot
inherit an older mining request; equal-state ABA remains rejected.
Breathing and suffocation require water/solid contact at both feet and eyes.
Sprinting into shallow water does not start the swimming stance unless the eyes
are already submerged.

Natural admission has global category caps plus per-chunk ground distribution and
a water-creature subcap. Spawn candidates use loaded views within 128 blocks,
independently of the smaller AI simulation radius. After successful persistence,
the storage owner evicts excess clean, unretained chunks, keeping a warm tail of
64. Retained views and dirty chunks remain outside that eviction policy.

Current dragon flight commits its pose through the regional entity owner.
Because flight and the dying rise bypass generic physics, the dragon authority
also refreshes chunk visibility and publishes the accepted pose through the
shared movement tracker. Newly visible clients receive a spawn at that pose,
not a relative delta against an unknown position.

One spatial owner writes its state at a time. Read-only work consumes immutable
views; expensive computation returns a result that is validated before apply.
Cross-owner coordination exists only for operations requiring atomicity across
those owners. There is no universal distributed transaction around every action.
Plugin authors never handle region keys, leases, epochs, or ownership migration.

## Overload policy approved by the owner

Within declared capacity, retain vanilla semantics. Above it, protecting the
kernel and healthy players takes precedence over the speed of an overloaded
farm or addon. Explicit refusal before mutation and local delay of expensive
work are allowed. Overload must not delete items, discard accepted mutations,
or produce partial transactions.

Account for induced engine work, not just script instructions: scans, edits,
entities, path searches, queued work, retained memory, and outbound data.
Bounds must apply before expensive allocation or execution. CPU capacity is
detected once; measured pressure adjusts bounded admission and scheduling.
Do not add operator worker-thread percentages or unbounded retry queues.

Keep slow connection handling and addon execution off the simulation owner.
Consumers wake from actual input, completion, capacity, or simulation events;
polling and wall-clock sleeps are not scheduling mechanisms. This is bounded
work and isolation, not a hard real-time guarantee against OS or hardware stalls.

## One addon contract, server and client

The server runs Wasmtime components implementing the versioned WIT contract
`solaris:plugin@0.7.0` (`crates/mc-script/wit`). `mc-plugin-host` owns component
loading and bounded callbacks; `mc-script` remains the runtime-independent
deployment metadata and admission boundary. Rust guest source and its SDK live
in the out-of-tree `sdk/rust/` workspace. Guests are independently built and
encoded into deployable component artifacts; production core does not compile
guest source or require a sibling checkout.

Each package carries `plugin.toml`, `plugin.wasm`, optional `config.toml`, and
only the declared resources it needs. `[plugins]` selects the deployment root,
strictness, expected package set, grants, and optional hook registrations; it
does not select a guest runtime. Strict production deployment requires explicit
operator grants for every capability a package requests. A package that does
not validate fails closed before a world or listener exists.

Component startup has two phases in two stores: `configure` runs once in a
short-lived store with no runtime capability and may answer only a normalized
startup contribution; `init` runs once in the component's runtime store and
builds instance state from the same package configuration. State retained in
`configure` cannot reach `init`. The component host validates
`required_features`, `[client]` bundles, `[worldgen]`, and startup
contributions before bind. The resulting effective plan determines the world
identity; a component binary, compiler, or contract revision does not.
Nothing between preparing a deployment and binding the listener may leave a
world or socket behind: a manifest, artifact, startup, or world-contract
failure stops the prepared deployment, and `--check` neither opens a world nor
binds.

Isolation, budgets, and the single admission boundary remain Solaris'
responsibility: the component sandbox does not replace the contract, world
owners, or durability rules. Reuse the existing common Loader implementation
with thin Fabric/NeoForge/Forge adapters; do not build a second independent
client stack.

The common API uses owned values, opaque generation-checked handles, immutable
observations, bounded requests, and typed completion/rejection. A plugin sees
serial callbacks, not engine concurrency. World reads, mutations, queries,
transactions, timers, and services follow the same naming and failure model.
Capabilities authorize access; resource budgets constrain its cost. Neither
client input nor an addon-provided identifier establishes authority: the native
owners fence every request on the exact live session, the owning plugin, the
live instance and revision, and the actions the presented model enables.

Component precommit decisions use the same bounded host-input queue, not a
synchronous Store call from a simulation owner. Build and damage freeze an
immutable context plus native state fences; the owner resumes through its
bounded command queue. Approval is consumed once, under the commit locks and
after state/session/permission revalidation. Producer-specific melee, dragon,
projectile, explosion and golem continuations retain their existing native
effects rather than replacing them with a second damage kernel. Programmatic
callers retain their original reply until a deferred continuation actually owns
it. Pending projectile impacts have native ownership so one collision cannot
submit repeated decisions. Mandatory protection registration outlives a failed
guest Store; the no-subscriber path does not wait for the guest.

The API must cover world/content operations, entities and inventory, persistence
and typed services, and Loader-backed input, screens/HUD, assets, audio,
particles, and bounded rendering. Domain-specific economies, colonies, machines,
and bosses belong in addons, not in special Rust entry points.

Implemented client presentation is a bounded set of typed requests - open,
present and close a view, play and stop an owned sound, grant an owned block
item - the component form of the Loader's existing view, sound and
custom-block surface. The server supplies every view instance id and revision,
a plugin only echoes a server-issued selection token inside a model, and a
request becomes an effect only where the Loader content-and-permission pair,
the live session and the native owners admit it. One Minecraft presenter owns
modal views and the per-connection set of non-modal HUD instances: several
owners' HUDs coexist, an update or a close names one exact instance and
revision, and a HUD never opens, replaces or closes a modal. A modal's caption
is the activated screen declaration's own title, and a view's initial model is
the authoritative one its Open carried. Screens and widgets are declared
schema-2 index data, not client scripting: a widgetless HUD renders nothing and
hosts only its declared input bindings. Thin adapters only register transport,
keyboard/focus hooks and HUD-layer registrations, and a disconnect or content
change drops every modal, HUD instance, held binding and action sink, so a
reconnect starts from no state and cannot replay an old edge.

Declared keyboard input belongs to the same activation: a schema-2 HUD screen
may declare at most eight bindings, each a canonical native key name bound to
owner-qualified press and release actions and admitted only with the views,
view_actions, present_views and send_view_actions content and permissions. The
client produces key edges at the HEAD and RETURN of the vanilla key-press path
plus GUI, overlay, screen-change and window-focus producers. Presses that open
or close a screen do not reach gameplay bindings. Declared F2/F11 edges may
also reach plugins while preserving vanilla screenshot/fullscreen handling.
Autorepeat is not an edge, and losing focus emits
exactly one release per held binding. Every admitted edge carries the live
instance, revision and increasing per-instance action sequence, and only
actions the presented model enables are sent. Runtime input stays vanilla: this
is fixed declared keyboard input, not rebinding, arbitrary client scripts, or
the complete target API.

Implemented audio uses the same verified asset pack: owned mono OGG Vorbis
`sounds`, permission `play_sounds`, and one shared Minecraft playback owner. The
typed play and stop requests pass through `ScriptBoundary` and the
exact-session ordered lane. Personal and fixed world-position one-shots use
native volume, pitch and attenuation; disconnect clears playback. Adapters
transport commands only. Loops, moving sources and particles are not
implemented.

Server-only addons retain vanilla-client support. Client features require the
actual negotiated capabilities; no invisible fake substitute counts as support.
Use one package identity, permission model, schema, and SDK. Connection-time
activation and pre-registry-freeze registration are different Minecraft
lifecycle phases, not separate addon platforms. Native registration requiring
restart must be explicit. Downloaded arbitrary Java/native code is not an API.

Each addon has isolated state and bounded memory, instructions, execution time,
queued requests, and engine work. Client limits also cover decoded assets,
geometry, UI nodes, and per-frame work. Failure is isolated to the offender;
uncommitted batches abort without changing unrelated state. General mutable
world handles and unrestricted synchronous hooks are excluded.

## Cutover and proof

1. Move startup data policy out of the CLI and remove repeated data/provenance
   wrappers; preserve real startup and `--check` behavior.
2. Establish shared game values and one authoritative gameplay mutation path;
   remove session-owned gameplay mirrors as their callers migrate.
3. Separate simulation ownership from connections and I/O. Collapse duplicate
   extension routing into the same admitted action/effect boundary.
4. Complete the uniform server/client addon contract and induced-work isolation.
5. Run the unchanged behavior, vanilla, graphical-client, and workload gates.
   Retain exact failures and hardware/build scope; do not declare global parity
   or capacity from a smaller successful probe.

Documentation has one architecture contract, one current API reference, a short
operator guide, and a small current-state cursor. Evidence records observations;
plans are not evidence. Replaced plans, function-by-function tours, duplicated
memory, and obsolete instruction packs are removed rather than copied forward.
