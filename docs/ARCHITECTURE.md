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

Keep strict Luau unless a measured requirement justifies replacing it. Reuse the
existing common Loader implementation with thin Fabric/NeoForge/Forge adapters;
do not build a second independent client stack.

The common API uses owned values, opaque generation-checked handles, immutable
observations, bounded requests, and typed completion/rejection. A plugin sees
serial callbacks, not engine concurrency. World reads, mutations, queries,
transactions, timers, and services follow the same naming and failure model.
Capabilities authorize access; resource budgets constrain its cost. Neither
client input nor an addon-provided identifier establishes authority.

The API must cover world/content operations, entities and inventory, persistence
and typed services, and Loader-backed input, screens/HUD, assets, audio,
particles, and bounded rendering. Domain-specific economies, colonies, machines,
and bosses belong in addons, not in special Rust entry points.

Implemented UI follows one `present_client_ui` command through `ScriptBoundary`,
one owned `ui` resource schema, and one `present_ui` permission. Screen, HUD and
hidden modes share bounded title/body values and exact-session authorization.
One Minecraft presenter owns modal views and the per-connection HUD set; thin
adapters only register transport and HUD-layer hooks. HUD is currently bounded
literal text, not general client scripting or an arbitrary layout engine.

Implemented keyboard input extends the same owned `interactions` resource and
`on_loader_interaction` event with trigger/press/release phases, under Loader
wire protocol 2. Shared bounded held state and one native keyboard HEAD hook
preserve vanilla input and release on focus loss; activation/logout fences
bindings to the exact connection. This is fixed declared keyboard input, not
rebinding, arbitrary client scripts, or the complete target API.

Implemented audio uses the same verified asset pack: owned mono OGG Vorbis
`sounds`, permission `play_sounds`, and one shared Minecraft playback owner.
`play_client_sound`/`stop_client_sound` pass through `ScriptBoundary` and the
exact-session ordered lane. Personal and fixed world-position one-shots use
native volume, pitch and attenuation; disconnect clears playback. Adapters
only transport commands. Loops, moving sources and particles are not implemented.

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
