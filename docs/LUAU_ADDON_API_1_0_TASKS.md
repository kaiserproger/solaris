# Solaris Luau Addon API 1.0 — implementation backlog

Status: parked; runtime implementation is blocked until vanilla parity  
Date: 2026-07-29  
Specification: [`LUAU_ADDON_API_1_0_SPEC.md`](LUAU_ADDON_API_1_0_SPEC.md)

This file decomposes the future addon platform into finite, testable tasks. It
is a planning artifact, not the active development queue.

## 1. Binding scheduling rule

No task marked `BLOCKED-VP` may be implemented before all four conditions hold:

1. the scoped vanilla 26.1.2 overworld-survival parity gate is closed;
2. the canonical playable queue has no higher-priority common-gameplay blocker;
3. the owner explicitly opens the addon-platform milestone;
4. the selected task has one bounded vertical result and an evidence plan.

Until that gate is opened, development returns to
[`playable/ACTIVE.md`](playable/ACTIVE.md). The current vanilla-parity cursor is:

> Continue village defence and the remaining species-specific mob loop; close
> real attack behavior for each supported profile or keep it explicitly blocked.

Documenting or refining this backlog does not activate it.

## 2. Status legend

- `DONE-DOC`: documentation-only task completed; no runtime implementation.
- `BLOCKED-VP`: fully specified but blocked by the vanilla-parity gate.
- `READY`: gate opened and dependencies closed; may be selected.
- `ACTIVE`: exactly one bounded implementation slice is in progress.
- `DONE`: implementation, tests, evidence and canonical documentation closed.
- `DEFERRED`: intentionally outside API 1.0.

All implementation tasks below start as `BLOCKED-VP`.

## 3. Task contract

Every implementation task must produce all of the following unless the task
explicitly narrows the list:

- a written behavior contract before code;
- one production path, not helper-only infrastructure;
- typed happy and sad paths;
- deterministic unit or simulation tests;
- wire/client evidence where behavior is client-visible;
- restart evidence where state is durable;
- quota and failure-isolation evidence where untrusted addon code runs;
- a focused independent review for ownership, security or persistence changes;
- update of only the canonical owner documents;
- no unrelated cleanup.

A milestone closes only through its named acceptance slice. Building isolated
host functions is insufficient.

## 4. High-level dependency order

```text
A00 contract freeze
 -> A01 package and lifecycle
 -> A02 schemas, values and futures
 -> A03 events, policies and scheduling
 -> A04 typed storage and transactions
 -> A05 services, permissions and commands
 -> A06 Loader package negotiation and client VM
 -> A07 dynamic items and blocks
 -> A08 entities, components and AI
 -> A09 typed networking, UI and rendering
 -> A10 structures, dimensions and worldgen
 -> A11 regions, work, logistics and research
 -> A12 graph networks and moving assemblies
 -> A13 operations, migrations and compatibility
 -> A14 reference addons and API 1.0 release gate
```

Client and server subtracks may run in parallel only after their shared schemas,
package identity and capability model are frozen.

---

# A00 — Contract freeze and threat model

Goal: convert the architecture document into enforceable boundaries before any
new runtime API is introduced.

## A00-01 — Record the target specification

Status: `DONE-DOC`

Output:

- `docs/LUAU_ADDON_API_1_0_SPEC.md`;
- explicit Rust/Luau ownership boundary;
- dynamic and boot client content modes;
- reference-addon acceptance classes;
- activation gate.

Definition of done:

- the specification exists under `docs/`;
- it does not claim current implementation;
- it points back to the active vanilla-parity queue.

## A00-02 — Decompose the implementation backlog

Status: `DONE-DOC`

Output:

- this task file;
- stable `Axx-yy` task identifiers;
- dependencies, deliverables and acceptance conditions;
- every implementation item marked `BLOCKED-VP`.

## A00-03 — API 1.0 threat model

Status: `BLOCKED-VP`

Depends on: owner activation of the addon-platform gate.

Scope:

- server Luau sandbox threats;
- client Luau sandbox threats;
- hostile package/archive inputs;
- dependency confusion and namespace squatting;
- client-to-server payload forgery;
- resource exhaustion;
- cross-addon state and service abuse;
- world persistence poisoning;
- boot-registry mismatch;
- renderer/GPU denial-of-service.

Deliverables:

- trust-boundary diagram;
- capability matrix;
- abuse cases and fail-closed responses;
- quota dimensions;
- audit-event requirements.

Definition of done:

- every public capability maps to validation, quota, audit and revocation rules;
- no API relies on client-declared authority;
- no API exposes filesystem, raw sockets, raw packet writers, Rust/JVM objects or
  GPU calls.

## A00-04 — Compatibility and versioning policy

Status: `BLOCKED-VP`

Depends on: `A00-03`.

Scope:

- API SemVer rules;
- addon package version identity;
- schema versioning;
- network protocol versioning;
- world fingerprint compatibility;
- feature negotiation;
- deprecation windows;
- current Plugin API `0.6.0` compatibility position.

Definition of done:

- source, storage, wire and world compatibility are separately defined;
- incompatible world-content changes cannot be silently loaded;
- compatibility adapters cannot weaken API 1.0 capability checks.

## A00-05 — Performance budgets and scale targets

Status: `BLOCKED-VP`

Depends on: `A00-03`.

Define frozen target budgets for:

- handler fuel and wall time;
- queued futures and events;
- client/server bandwidth;
- addon memory;
- storage and blob volume;
- custom entities;
- block-entity wakeups;
- UI node count;
- render submissions and asset memory;
- worldgen plan size;
- graph node counts;
- moving assembly size.

Definition of done:

- Create-class 1,000-node and MineColonies-class 100-citizen targets have named
  profiling gates;
- overload behavior is bounded and observable rather than an implicit slowdown.

## A00-06 — API naming and domain-leak review

Status: `BLOCKED-VP`

Depends on: `A00-04`.

Review every proposed public Rust and Luau namespace against the generic-boundary
rule.

Definition of done:

- no engine API embeds `colony`, `kingdom`, `research`, `turret`, `home_order`,
  `kinetic_press` or another reference-addon domain noun;
- domain concepts appear only in reference addons and examples.

Milestone acceptance:

- approved contract, threat model, compatibility policy and budgets;
- no production runtime changes yet.

---

# A01 — Package model, dependency graph and lifecycle

Goal: load a validated shared/server/client addon package with deterministic
startup order and no gameplay capabilities yet.

## A01-01 — `addon.toml` schema

Status: `BLOCKED-VP`

Depends on: `A00` acceptance.

Implement bounded parsing and normalization for:

- identity and display metadata;
- entrypoints;
- activation/client policy;
- compatibility versions;
- dependencies and conflicts;
- provided/consumed services;
- server/client capabilities;
- world-persistent marker;
- uninstall policy.

Tests:

- malformed, duplicate, oversized and escaping values;
- unknown fields under a closed schema;
- canonical namespace normalization;
- exact diagnostic paths.

## A01-02 — Canonical package identity

Status: `BLOCKED-VP`

Depends on: `A01-01`.

Define package identity from:

- addon ID;
- version;
- package schema;
- complete content hash;
- relevant platform version;
- client registration mode.

Definition of done:

- any byte change alters identity;
- identity is stable across filesystem ordering;
- identity is used by cache, world metadata and client handshake.

## A01-03 — Dependency and feature resolver

Status: `BLOCKED-VP`

Depends on: `A01-01`, `A00-04`.

Support:

- required/optional/load-before/conflict relations;
- SemVer ranges;
- required features;
- cycles and ambiguous providers;
- deterministic ordering independent of directory order.

Tests:

- cycles;
- missing optional and required addons;
- conflicting versions;
- deterministic normalized graph;
- provider selection override.

## A01-04 — Shared/server/client module resolver

Status: `BLOCKED-VP`

Depends on: `A01-02`.

Implement side-aware module loading:

- shared code may not import server/client-only modules;
- server code cannot import client implementation modules;
- client code cannot import server implementation modules;
- cross-addon imports require declared dependencies and exported modules;
- import cycles fail with bounded diagnostics.

## A01-05 — Startup phase state machine

Status: `BLOCKED-VP`

Depends on: `A01-03`, `A01-04`.

Implement the ordered phases from discovery through accepting players.

Definition of done:

- registry calls fail outside registration phases;
- world/storage access fails before world open;
- service calls fail before provider enable;
- startup rollback disables only addons that can safely be omitted;
- a world-critical addon failure stops startup.

## A01-06 — Config schema and immutable snapshots

Status: `BLOCKED-VP`

Depends on: `A01-01`.

Support typed config fields, defaults, bounds, enums, descriptions,
reloadability, secrets and client visibility.

Tests:

- generated default config;
- invalid startup config;
- invalid reload preserves old snapshot;
- secret fields never enter client payloads or logs.

## A01-07 — Package-only validation command

Status: `BLOCKED-VP`

Depends on: `A01-05`, `A01-06`.

Add an operator command that validates packages, dependencies, capabilities,
assets and hashes without opening a world.

Acceptance slice:

- one shared/server-only empty addon and one shared/server/client empty addon
  load in deterministic order;
- invalid side import and dependency cycle fail before the server accepts a
  connection;
- no gameplay API is exposed yet.

---

# A02 — Typed schemas, values, handles and futures

Goal: establish one safe type system for config, storage, services, networking,
components and client state.

## A02-01 — Runtime schema algebra

Status: `BLOCKED-VP`

Depends on: `A01` acceptance.

Implement bounded schemas for:

- booleans, signed/unsigned integers and finite floats;
- strings and bytes;
- resource IDs, UUIDs and logical IDs;
- vectors, positions, colors and transforms;
- arrays, maps, records, enums, options and tagged unions;
- recursive references with explicit depth bounds.

## A02-02 — Deterministic schema fingerprinting

Status: `BLOCKED-VP`

Depends on: `A02-01`.

Definition of done:

- semantically identical normalized schemas share a fingerprint;
- field-order accidents cannot change the fingerprint;
- incompatible changes are detectable;
- fingerprints are usable by storage, services and network channels.

## A02-03 — Luau type-prelude generation

Status: `BLOCKED-VP`

Depends on: `A02-01`.

Generate strict Luau types from normalized schemas and host APIs.

Tests:

- valid examples type-check;
- invalid field access, wrong union branch and wrong service signatures fail;
- generated names are deterministic and namespace-safe.

## A02-04 — Immutable value projection

Status: `BLOCKED-VP`

Depends on: `A02-01`.

Project Rust values to immutable Luau snapshots with bounded allocation and no
alias to mutable engine state.

## A02-05 — Opaque handle registry

Status: `BLOCKED-VP`

Depends on: `A02-04`, `A00-03`.

Support:

- session handles;
- lease-based entity and operation handles;
- persistent logical IDs;
- owner addon binding;
- generation fencing;
- expiry and revocation.

Tests:

- stale generation;
- foreign addon token;
- expired lease;
- reconnect/session replacement;
- owner migration.

## A02-06 — Typed result and failure vocabulary

Status: `BLOCKED-VP`

Depends on: `A02-01`.

Define common failures such as:

- `invalid_argument`;
- `permission_denied`;
- `not_found`;
- `unavailable`;
- `stale_revision`;
- `conflict`;
- `quota_exceeded`;
- `cancelled`;
- `deadline_exceeded`;
- `unsupported`;
- `internal_failure`.

Domain-specific details remain namespaced structured data.

## A02-07 — Future and cancellation runtime

Status: `BLOCKED-VP`

Depends on: `A02-05`, `A02-06`.

Implement:

- typed future completion;
- deadlines;
- cancellation;
- addon-disable cancellation;
- disconnected-player cancellation;
- bounded waiter count;
- no blocking of owner threads;
- result delivery into the owning VM only.

## A02-08 — Schema fuzz and allocation tests

Status: `BLOCKED-VP`

Depends on: `A02-01` through `A02-07`.

Fuzz malformed schemas and values, oversized containers, deep nesting,
invalid UTF-8 boundaries and cancellation races.

Acceptance slice:

- a server addon defines one schema, requests one asynchronous echo through an
  engine-owned test adapter and receives a typed result;
- malformed values, stale handles and cancelled futures fail without cross-addon
  delivery or leaked waiters.

---

# A03 — Events, policies, transforms and scheduling

Goal: give Luau complete policy control without handing it locks or relying on
unbounded tick callbacks.

## A03-01 — Event registry and ownership

Status: `BLOCKED-VP`

Depends on: `A02` acceptance.

Support:

- broadcast facts;
- owner-targeted results;
- subscription validation;
- immutable event DTOs;
- deterministic handler ordering inside one addon;
- unspecified ordering across unrelated addons unless an explicit pipeline is
  declared.

## A03-02 — Post-commit fact publication

Status: `BLOCKED-VP`

Depends on: `A03-01`.

Migrate or bridge current gameplay events into a uniform post-commit model.

Definition of done:

- rejected/no-op actions publish no false fact;
- facts carry exact committed identity, dimension and revision where relevant;
- one action does not produce duplicate semantic facts.

## A03-03 — Synchronous policy hooks

Status: `BLOCKED-VP`

Depends on: `A03-01`, `A00-05`.

Implement bounded `allow`, `deny` and typed modification decisions.

Restrictions:

- no await;
- no storage/network call;
- no mutable global side effect that affects the current commit;
- deterministic fuel budget;
- fail-closed or owner-configured failure policy.

## A03-04 — Transform pipelines

Status: `BLOCKED-VP`

Depends on: `A03-03`.

Support typed transformation of loot, recipes, damage plans, spawn plans and
other explicitly registered plan types.

Definition of done:

- ordering and conflict resolution are explicit;
- transformed plans are fully revalidated;
- one addon cannot edit another addon's private fields.

## A03-05 — Timers and retained jobs

Status: `BLOCKED-VP`

Depends on: `A02-07`.

Support:

- after/every/game-time schedules;
- persistent and transient timers;
- unique/idempotent job keys;
- bounded payload schemas;
- cancellation;
- restart reconstruction;
- per-addon quotas.

## A03-06 — Change and spatial subscriptions

Status: `BLOCKED-VP`

Depends on: `A03-01`.

Support bounded subscriptions for:

- component changes;
- region enter/exit;
- nearby entity query changes;
- network graph dirty state;
- work-request transitions;
- chunk load/unload.

## A03-07 — Policy latency and failure gate

Status: `BLOCKED-VP`

Depends on: `A03-03` through `A03-06`.

Measure policy hooks under many installed addons. Prove one runaway handler is
interrupted and cannot block the authoritative lane beyond the frozen budget.

Acceptance slice:

- a Luau claim policy denies a real survival block break before mutation;
- an allowed break commits and emits exactly one post-commit fact;
- policy timeout fails according to the declared policy;
- no storage/zone two-commit compensation is needed for the decision itself.

---

# A04 — Typed storage and cross-owner transactions

Goal: support durable economies, towns and settlements without addon-specific
Rust state or crash windows between storage and gameplay commits.

## A04-01 — Typed KV collections

Status: `BLOCKED-VP`

Depends on: `A02`, `A03` acceptance.

Add schema-owned KV collections with revisioned get/put/CAS/delete and cursor
listing.

## A04-02 — Typed indexed tables

Status: `BLOCKED-VP`

Depends on: `A04-01`.

Support primary keys, bounded secondary indexes, uniqueness, cursor queries and
index migration.

## A04-03 — Documents and content-addressed blobs

Status: `BLOCKED-VP`

Depends on: `A04-01`.

Provide bounded document and blob storage for research trees, structure plans,
large graphs and user-authored assets. Hash and size are authoritative.

## A04-04 — Storage migrations

Status: `BLOCKED-VP`

Depends on: `A04-01` through `A04-03`.

Implement ordered migrations with:

- old/new schema fingerprints;
- dry-run validation;
- checkpoints;
- restart after interruption;
- rollback or explicit irreversible declaration;
- operator progress.

## A04-05 — General transaction plan model

Status: `BLOCKED-VP`

Depends on: `A04-01`, current owner mutation APIs.

Define plan operations and preconditions for:

- addon storage;
- player and container inventory;
- world edits;
- entity components;
- economy accounts;
- region/group state;
- jobs.

## A04-06 — Owner discovery and deterministic lock order

Status: `BLOCKED-VP`

Depends on: `A04-05`.

The engine resolves all owners before commit, orders acquisition, rechecks
revisions and never lets Luau hold an owner lock.

## A04-07 — Journal and crash recovery

Status: `BLOCKED-VP`

Depends on: `A04-06`.

Prove:

- committed plan survives restart completely;
- aborted plan leaves every participant unchanged;
- replay by idempotency key does not duplicate effects;
- recovery does not publish facts twice.

## A04-08 — Transaction result detail

Status: `BLOCKED-VP`

Depends on: `A04-05`.

Return participant-scoped failures such as insufficient item, full inventory,
stale storage, unavailable player, policy denial or owner timeout.

## A04-09 — Bulk edit sessions and undo tokens

Status: `BLOCKED-VP`

Depends on: `A04-05` through `A04-07`.

Implement bounded selections, masks, patterns, staged chunk edits, progress,
cancellation and undo journal ownership.

Acceptance slice:

- rewrite the basic economy purchase/refund path on the general transaction plan;
- preserve physical currency, product and durable ledger atomically;
- report exact insufficient-currency, full-inventory, stale-ledger and player-gone
  failures;
- prove save/restart and idempotent replay.

---

# A05 — Services, permissions, groups and commands

Goal: support the Vault/LuckPerms/Essentials/Towny interoperability class.

## A05-01 — Service schema registry

Status: `BLOCKED-VP`

Depends on: `A02`, `A04` acceptance.

Define versioned sync/async method schemas and provider metadata.

## A05-02 — Provider resolution

Status: `BLOCKED-VP`

Depends on: `A05-01`, `A01-03`.

Resolve by compatibility, operator preference, priority and features. Fail
cycles and ambiguous mandatory providers before accepting players.

## A05-03 — Service call isolation

Status: `BLOCKED-VP`

Depends on: `A05-02`, `A02-07`.

Enforce caller/provider capabilities, deadlines, cancellation, quotas, typed
failures and no direct VM object sharing.

## A05-04 — Standard economy service v1

Status: `BLOCKED-VP`

Depends on: `A05-01` through `A05-03`, `A04`.

Define accounts, currencies, balances, debit/credit/transfer, idempotency,
offline subjects and transaction-plan participation.

## A05-05 — Permission registry and tri-state evaluation

Status: `BLOCKED-VP`

Depends on: `A05-01`.

Support permission declaration, online/offline subjects, defaults, groups,
inheritance and immutable decision snapshots.

## A05-06 — Context providers and invalidation

Status: `BLOCKED-VP`

Depends on: `A05-05`.

Implement bounded dimensions/regions/worlds/relations and addon contexts with
explicit invalidation rather than per-tick global recomputation.

## A05-07 — Generic group and relation service

Status: `BLOCKED-VP`

Depends on: `A05-01`, `A04`.

Provide generic groups, memberships, roles and typed directed relations. Do not
add town/kingdom/nation semantics to Rust.

## A05-08 — Typed command trees

Status: `BLOCKED-VP`

Depends on: `A02`, `A05-05`.

Support literals, typed arguments, suggestions, help, localization, player and
console sources, permissions, cooldowns and async results.

## A05-09 — Audit and decision explanation

Status: `BLOCKED-VP`

Depends on: `A05-04` through `A05-08`.

Expose bounded operator traces for provider choice, permission decisions,
command denial and economy failure without leaking secrets.

Acceptance slice:

- two economy providers install simultaneously;
- operator selection deterministically chooses one;
- an Essentials-class sample addon uses economy and permission services without
  importing either provider;
- offline balance and contextual permission survive restart.

---

# A06 — Loader negotiation, package delivery and client Luau VM

Goal: activate a verified client addon on Fabric, NeoForge and Forge through one
shared contract.

## A06-01 — API 1.0 client manifest and package index

Status: `BLOCKED-VP`

Depends on: `A01`, `A02`, `A00-03`.

Replace/extend the closed Loader bundle index with the API 1.0 package identity,
entrypoints, assets, schemas, permissions and registration mode.

## A06-02 — Capability consent model

Status: `BLOCKED-VP`

Depends on: `A06-01`.

Define per-server decisions for assets, custom UI, input, rendering, local
storage, boot registration and any trusted material/shader capability.

Definition of done:

- changed permission set prompts again;
- denial downloads or activates nothing unauthorized;
- decisions are not shared between server addresses.

## A06-03 — Artifact streaming and cache

Status: `BLOCKED-VP`

Depends on: `A06-01`.

Reuse and generalize exact size/hash, contiguous chunks, staging, atomic move,
cache identity and stale-connection fences.

## A06-04 — Dynamic activation state machine

Status: `BLOCKED-VP`

Depends on: `A06-02`, `A06-03`.

Activate dynamic assets and virtual registries without restart, then acknowledge
the exact registry/resource fingerprint before Play.

## A06-05 — Boot activation state machine

Status: `BLOCKED-VP`

Depends on: `A06-02`, `A06-03`.

Implement download -> authorize -> restart-required -> pre-freeze registration
-> reconnect -> fingerprint confirmation.

Tests:

- missing restart;
- stale cached package;
- registry mismatch;
- server changes package while client restarts;
- downgrade and rollback.

## A06-06 — Client Luau runtime embedding

Status: `BLOCKED-VP`

Depends on: `A06-04`.

Create isolated client VMs with strict type-checking, fuel, memory, error budgets
and no filesystem/JVM/raw-render access.

## A06-07 — Shared module parity

Status: `BLOCKED-VP`

Depends on: `A06-06`, `A01-04`.

Prove the same shared pure Luau module and schema fingerprints execute on server
and client with deterministic serialized values.

## A06-08 — Client lifecycle and cleanup

Status: `BLOCKED-VP`

Depends on: `A06-06`.

Clear VM state, registries, packs, UI, input and network handlers on disconnect.
A stale close from an old connection cannot remove a newer connection's state.

## A06-09 — Three-loader real-client gate

Status: `BLOCKED-VP`

Depends on: `A06-01` through `A06-08`.

Run exact dynamic activation, interaction and disconnect cleanup on Fabric,
NeoForge and Forge.

Acceptance slice:

- one package contains shared/server/client Luau and one verified texture;
- all three loaders activate it in dynamic mode;
- client Luau renders a bounded diagnostic label and returns one typed action;
- denial, malformed package and stale connection fail closed.

---

# A07 — Dynamic item, block and block-entity content

Goal: register and use multiple addon-owned items and blocks without the current
single/few-carrier special-case ceiling.

## A07-01 — Canonical dynamic registry allocator

Status: `BLOCKED-VP`

Depends on: `A06` acceptance.

Allocate per-connection projections for many owned IDs while persisting only
canonical namespaced IDs.

## A07-02 — Typed item definitions

Status: `BLOCKED-VP`

Depends on: `A07-01`, `A02`.

Support stack size, durability, rarity, model/name, components, use action,
cooldown, equipment/tool/food/projectile descriptors and creative grouping.

## A07-03 — Custom item components

Status: `BLOCKED-VP`

Depends on: `A07-02`, `A04`.

Persist, compare, stack and migrate schema-owned item data. Unknown or invalid
component payloads fail closed.

## A07-04 — Server-authoritative item use

Status: `BLOCKED-VP`

Depends on: `A07-02`, `A03`, `A04`.

Route use-on-air/use-on-block/use-on-entity through authoritative snapshots,
policies and transaction plans.

## A07-05 — Typed block definitions and state properties

Status: `BLOCKED-VP`

Depends on: `A07-01`.

Support properties, shapes, hardness, resistance, light, loot, sounds, render
model and placement rules.

## A07-06 — Block state persistence and palette projection

Status: `BLOCKED-VP`

Depends on: `A07-05`.

Persist canonical addon state, project exact session runtime IDs in block
updates and chunk palettes, and reject unacknowledged clients.

## A07-07 — Block entities and wakeup model

Status: `BLOCKED-VP`

Depends on: `A07-05`, `A02`, `A04`.

Support schema components, inventories, replication scopes, scheduled wakeups,
chunk load/unload and restart.

## A07-08 — Recipes, loot and tags

Status: `BLOCKED-VP`

Depends on: `A07-02`, `A07-05`.

Register addon content in recipes, loot and typed tags without hard-coded Rust
knowledge of addon IDs.

## A07-09 — Dynamic client item/block rendering

Status: `BLOCKED-VP`

Depends on: `A07-02`, `A07-05`, `A06`.

Resolve verified models, blockstates, item definitions, names and tooltips for
multiple simultaneous addons.

## A07-10 — Survival conservation wire gate

Status: `BLOCKED-VP`

Depends on: `A07-01` through `A07-09`.

Acceptance slice:

- two addons register multiple distinct items and blocks;
- a real client receives, places, uses, breaks, drops and picks them up;
- custom component data survives inventory movement, item entity, save/restart
  and reconnect;
- wrong owner, stale hand, missing ACK and full inventory conserve state.

---

# A08 — Entity archetypes, components, AI and abilities

Goal: implement Recruits/Citizens/MythicMobs-class entities without
entity-specific Rust adapters.

## A08-01 — Entity archetype registry

Status: `BLOCKED-VP`

Depends on: `A07`, `A02`.

Register dimensions, category, tracking, base components, navigation class and
client presentation.

## A08-02 — Schema-owned ECS components

Status: `BLOCKED-VP`

Depends on: `A08-01`.

Support persistence, replication and mutation policy for addon components while
keeping ECS storage private.

## A08-03 — Component attach/detach transactions

Status: `BLOCKED-VP`

Depends on: `A08-02`, `A04`.

Attach, replace and remove traits through revision-fenced owner commands.

## A08-04 — Persistent logical entity identity

Status: `BLOCKED-VP`

Depends on: `A08-02`, `A04`.

Separate durable NPC identity from active ECS and wire identities. Cover unload,
restart, owner migration and rematerialization.

## A08-05 — Generic entity spawn/materialize/despawn

Status: `BLOCKED-VP`

Depends on: `A08-01`, `A08-04`.

Use canonical archetype ID, typed initial components and policy validation.

## A08-06 — Navigation goal API

Status: `BLOCKED-VP`

Depends on: current pathfinding owner, `A02-07`.

Expose move/follow/flee/patrol/look/idle goals with typed acceptance,
completion, interruption and failure.

## A08-07 — Behavior-tree runtime

Status: `BLOCKED-VP`

Depends on: `A08-02`, `A08-06`.

Compile bounded trees from Luau definitions; wake from sensors/events; preserve
state without 20 Hz Lua polling for every entity.

## A08-08 — Utility AI, memory and sensors

Status: `BLOCKED-VP`

Depends on: `A08-07`.

Implement typed blackboards, TTL memory, bounded spatial sensors, utility scores
and explicit cadence.

## A08-09 — Squads and formations

Status: `BLOCKED-VP`

Depends on: `A08-06`, `A08-08`.

Provide generic squad membership, leader relation, formation slots and owner
migration. Luau owns orders and relation policy.

## A08-10 — Ability graph primitives

Status: `BLOCKED-VP`

Depends on: `A03`, `A08-02`.

Register triggers, conditions, targeters, costs, cooldowns, cast phases,
mechanics and effects.

## A08-11 — Projectile and damage integration

Status: `BLOCKED-VP`

Depends on: `A08-10`, authoritative combat path.

Custom abilities must reuse server-owned reach, collision, damage source,
invulnerability, attribution, death and loot commits.

## A08-12 — Client model and animation state

Status: `BLOCKED-VP`

Depends on: `A08-01`, `A09` client rendering foundations.

Replicate only declared render state and drive verified models/animations.

Acceptance slice:

- a Recruits-class sample registers one recruit archetype;
- recruit persists owner/role/squad across restart;
- Luau implements follow, hold, patrol and attack selection;
- Rust only executes generic goals and combat primitives;
- two squads use a formation without cross-owner or tracking leaks.

---

# A09 — Typed networking, custom UI, assets and rendering

Goal: make client behavior programmable while keeping networking and GPU access
bounded.

## A09-01 — Typed addon channels

Status: `BLOCKED-VP`

Depends on: `A02`, `A06`.

Register versioned clientbound/serverbound schemas by phase and direction.
Generate bounded codecs and reject undeclared payloads.

## A09-02 — Delivery scopes

Status: `BLOCKED-VP`

Depends on: `A09-01`.

Support exact player, tracking entity, tracking chunk and dimension scopes. No
raw broadcast or packet writer.

## A09-03 — Request correlation and anti-replay

Status: `BLOCKED-VP`

Depends on: `A09-01`, `A02-07`.

Add request IDs, session binding, deadlines, duplicate suppression and bounded
pending maps.

## A09-04 — Asset registry

Status: `BLOCKED-VP`

Depends on: `A06`.

Register verified textures, models, animations, sounds, particles, language and
materials with exact ownership and dependency rules.

## A09-05 — Declarative UI tree

Status: `BLOCKED-VP`

Depends on: `A06-06`, `A09-01`.

Implement text, image, item/block/entity preview, button, input, scroll,
virtualized list, tabs, flex/grid and tooltip nodes.

## A09-06 — Reactive UI state and server patches

Status: `BLOCKED-VP`

Depends on: `A09-05`.

Support local state, immutable props, keyed patches, optimistic presentation and
authoritative rejection/reconciliation.

## A09-07 — Accessibility and localization

Status: `BLOCKED-VP`

Depends on: `A09-05`.

Require accessibility labels, keyboard/controller navigation, scale handling,
subtitles and localized message IDs.

## A09-08 — HUD and world overlays

Status: `BLOCKED-VP`

Depends on: `A09-05`, `A09-04`.

Register bounded HUD anchors and tracked world overlays for regions, paths,
networks and build previews.

## A09-09 — Declarative render registry

Status: `BLOCKED-VP`

Depends on: `A09-04`.

Support item, block, block-entity, entity, armor, held-item, particle, sky and
fog presentation through verified assets.

## A09-10 — Render command buffer

Status: `BLOCKED-VP`

Depends on: `A09-09`, `A00-05`.

Expose mesh instance, line, text and bounded world-feature submissions. Enforce
per-addon frame budgets and interruption.

## A09-11 — Animation and audio state machines

Status: `BLOCKED-VP`

Depends on: `A09-04`, `A09-09`.

Support animation tracks, blend/state machines, timeline events, bone
attachments, positional sound, music zones and subtitles.

## A09-12 — Safe material graph

Status: `BLOCKED-VP`

Depends on: `A09-09`, `A00-03`.

Define a bounded material/shader graph. Arbitrary shader source remains
`DEFERRED` unless separately trusted and approved.

## A09-13 — Ponder-like guide scenes

Status: `BLOCKED-VP`

Depends on: `A09-05`, `A09-09`, structure preview primitives.

Implement staged scenes, ghost structures, highlights, arrows, camera paths,
actors, simulated transfers and recipe links.

Acceptance slice:

- one custom management screen renders 100 virtualized rows;
- button actions use typed, session-bound requests;
- one HUD and one world overlay appear only in declared scopes;
- one custom entity animation and one Ponder-like scene run on all three loaders;
- budget overflow disables only the offending render feature.

---

# A10 — Structures, dimensions, portals and world generation

Goal: satisfy Twilight Forest and MineColonies structure requirements without
per-block unrestricted callbacks.

## A10-01 — Structure template registry

Status: `BLOCKED-VP`

Depends on: `A07`, `A04-09`.

Register NBT/SNBT or normalized templates, anchors, palettes, processors and
content hashes.

## A10-02 — Structure validation and preview

Status: `BLOCKED-VP`

Depends on: `A10-01`, `A09-08`.

Support scan, rotate, mirror, placement validation, ghost preview, material
requirements and exact rejection reasons.

## A10-03 — Staged construction work plan

Status: `BLOCKED-VP`

Depends on: `A10-01`, `A11` work-order foundation.

Convert a structure into bounded dependency-aware edit batches with progress,
cancellation and restart continuity.

## A10-04 — Declarative worldgen schema

Status: `BLOCKED-VP`

Depends on: `A02`, existing worldgen ownership.

Register noise, density, surface, carver, feature, biome, structure-set,
template-pool, processor, loot and spawn descriptors.

## A10-05 — Deterministic Luau worldgen kernels

Status: `BLOCKED-VP`

Depends on: `A10-04`, `A00-05`.

Run pure chunk/region-batch functions with seeded RNG, no I/O/global mutation and
bounded placement plans.

Tests:

- replay equality;
- worker-count independence;
- fuel/memory interruption;
- oversized plan rejection;
- no mixed old/new authority after config change.

## A10-06 — Dimension registry

Status: `BLOCKED-VP`

Depends on: boot client mode, `A10-04`.

Register dimension type, generator, biome source and client effects before
freeze. Persist the exact descriptor fingerprint.

## A10-07 — Portals and travel transactions

Status: `BLOCKED-VP`

Depends on: `A10-06`, authoritative teleport and world ownership.

Support frame/interior matching, activation policy, destination calculation,
portal creation and restart-safe transfer.

## A10-08 — Progression policy integration

Status: `BLOCKED-VP`

Depends on: `A03`, `A10-01`, `A10-06`.

Allow typed locks over structure entry, portal use, loot, abilities and biome
access.

## A10-09 — World fingerprint and migration gate

Status: `BLOCKED-VP`

Depends on: `A01-02`, `A10-04`, `A10-06`.

Persist exact package identities, registry fingerprints and generation graphs.
Reject incompatible startup unless a declared migration or explicit operator
override exists.

Acceptance slice:

- a Twilight-class sample creates one boot-registered dimension and portal;
- deterministic biomes, structures and custom feature generate identically on
  replay;
- client sky/fog/music activate only in that dimension;
- incompatible worldgen package change is rejected before chunk generation.

---

# A11 — Regions, work orders, logistics and research

Goal: satisfy Towny/Kingdoms and MineColonies-class server logic using generic
primitives.

## A11-01 — Region geometry registry

Status: `BLOCKED-VP`

Depends on: `A03`, `A04`, `A05-07`.

Support chunk sets, cuboids, bounded polygons, dimensions, nested regions,
priorities and ownership.

## A11-02 — Published policy index

Status: `BLOCKED-VP`

Depends on: `A11-01`.

Produce immutable, revisioned policy snapshots consumed by ordinary break,
place, container, fluid, fire, explosion, piston, entity-interaction, PvP and
other authoritative paths without global plugin locks.

## A11-03 — Policy inheritance and relation contexts

Status: `BLOCKED-VP`

Depends on: `A11-02`, `A05-06`, `A05-07`.

Resolve nested rules, role bindings, group relations, conflict overrides and
explicit denial reasons.

## A11-04 — Map projection service

Status: `BLOCKED-VP`

Depends on: `A11-01`, `A09-08`.

Publish bounded region geometry and metadata to approved map/UI consumers.

## A11-05 — Durable work-order engine

Status: `BLOCKED-VP`

Depends on: `A04`, `A03-05`.

Support typed orders, priorities, dependencies, reservation, assignment,
blocked reasons, progress, cancellation and restart.

## A11-06 — Resource request and reservation engine

Status: `BLOCKED-VP`

Depends on: `A11-05`, item/container APIs.

Support matchers, quantities, substitutions, destinations, reservation,
expiration and transactional transfer.

## A11-07 — Logistics route primitive

Status: `BLOCKED-VP`

Depends on: `A11-06`, navigation and graph foundations.

Plan bounded source/destination routes without exposing inventories or pathfinder
internals to Luau.

## A11-08 — Research graph and typed modifiers

Status: `BLOCKED-VP`

Depends on: `A04`, `A05`.

Register prerequisites, costs, unlocks and typed modifiers. Prevent arbitrary
mutation of another addon.

## A11-09 — Tax, upkeep and scheduled settlement jobs

Status: `BLOCKED-VP`

Depends on: `A05-04`, `A11-01`, `A03-05`.

Provide generic hooks for addon-owned periodic policy using economy transactions
and idempotent retained jobs.

## A11-10 — MineColonies-class recovery gate

Status: `BLOCKED-VP`

Depends on: `A08`, `A10`, `A11-05` through `A11-08`.

Acceptance slice:

- 100 persistent citizens;
- buildings validated from templates;
- professions/roles remain Luau data;
- requests travel warehouse -> courier -> worker;
- one staged building order survives save/restart;
- no `colony` Rust registry exists.

## A11-11 — Towny/Kingdoms-class policy gate

Status: `BLOCKED-VP`

Depends on: `A11-01` through `A11-04`, `A05`.

Acceptance slice:

- claims, nested groups, ranks, banks, taxes, diplomacy and temporary war policy
  are implemented in Luau;
- common world actions consume one published policy index;
- economy and permissions are replaceable services;
- restart preserves every durable relation and policy revision.

---

# A12 — Graph networks and moving assemblies

Goal: satisfy the Create-class simulation boundary.

## A12-01 — Generic port model

Status: `BLOCKED-VP`

Depends on: blocks, block entities and components.

Register typed directional ports, compatibility rules and connection changes.

## A12-02 — Topology ownership and connected components

Status: `BLOCKED-VP`

Depends on: `A12-01`.

Maintain components across block edits, chunk boundaries, load/unload and owner
migration without Luau scanning the world.

## A12-03 — Scalar constraint solver

Status: `BLOCKED-VP`

Depends on: `A12-02`.

Provide generic bounded solving suitable for speed/direction/stress/capacity.
Luau supplies node semantics and overload policy.

## A12-04 — Directed item routing solver

Status: `BLOCKED-VP`

Depends on: `A12-02`, inventories and transaction plans.

Support routes, filters, reservations, backpressure and exact transfer commits.

## A12-05 — Fluid/energy/signal graph families

Status: `BLOCKED-VP`

Depends on: `A12-02`, `A00-05`.

Add generic bounded volume/pressure, capacity and signal propagation contracts.
Do not encode a specific mod's units or machine types in Rust.

## A12-06 — Graph snapshots and client replication

Status: `BLOCKED-VP`

Depends on: `A12-02`, `A09`.

Replicate only tracking-scoped, declared presentation state. Support debug and
Ponder visualizations without exposing private server state.

## A12-07 — Assembly selection and atomic extraction

Status: `BLOCKED-VP`

Depends on: block state persistence and transaction plans.

Select bounded connected blocks, validate movement policy and atomically replace
world blocks with an assembly representation.

## A12-08 — Assembly physics and collision

Status: `BLOCKED-VP`

Depends on: `A12-07`, engine physics.

Support translation/rotation, broadphase, entity riding, owner migration and
bounded collision callbacks.

## A12-09 — Mounted capabilities

Status: `BLOCKED-VP`

Depends on: `A12-07`, inventories and graph ports.

Preserve mounted inventories, block-entity components and declared network
capabilities without retaining live world locks.

## A12-10 — Reassembly and crash recovery

Status: `BLOCKED-VP`

Depends on: `A12-07` through `A12-09`.

Prove atomic reassembly, obstruction rejection, restart reconstruction and no
block/item duplication.

## A12-11 — Batched/instanced assembly rendering

Status: `BLOCKED-VP`

Depends on: `A09-09`, `A12-06`.

Render large assemblies under explicit instance/draw budgets on all three
loaders.

Acceptance slice:

- a Create-class sample runs a 1,000-node kinetic graph;
- one source drives shafts and a machine under stress limits;
- one moving assembly carries mounted inventory, collides, stops and reassembles;
- server restart conserves blocks, components and inventory;
- client receives tracking-scoped animation and a Ponder scene.

---

# A13 — Operations, tooling, migrations and compatibility

Goal: make the platform operable and safe for real addon development.

## A13-01 — Addon SDK type package

Status: `BLOCKED-VP`

Depends on: stable host APIs through `A12`.

Publish generated strict Luau types, schemas, examples and side-aware module
stubs.

## A13-02 — Pure Luau test runner

Status: `BLOCKED-VP`

Depends on: `A02`, package resolver.

Support deterministic tests, assertions, fixtures, coverage and machine-readable
results without opening a world.

## A13-03 — Server simulation harness

Status: `BLOCKED-VP`

Depends on: runtime APIs.

Provide deterministic clock, fake players, worlds, storage, owners,
transactions, permissions and network observations.

## A13-04 — Client UI/render harness

Status: `BLOCKED-VP`

Depends on: `A09`.

Support screen snapshots, render-command snapshots, asset checks, input and
resource reload/reconnect tests.

## A13-05 — Addon replay capture

Status: `BLOCKED-VP`

Depends on: events, scheduler, RNG and transactions.

Capture authoritative inputs, package/schema IDs and RNG seeds; replay without
external time or nondeterministic ordering.

## A13-06 — Per-addon metrics and profiler

Status: `BLOCKED-VP`

Depends on: all runtime execution paths.

Expose fuel, wall time, memory, event/future queues, storage, transactions,
network, entities, worldgen and render cost.

## A13-07 — Error budget and automatic isolation

Status: `BLOCKED-VP`

Depends on: `A13-06`, threat model.

Interrupt offenders, abort staged work and disable only the relevant addon or
feature. Define world-critical startup behavior.

## A13-08 — Config/logic/asset reload

Status: `BLOCKED-VP`

Depends on: lifecycle, client VM and storage.

Implement atomic config reload, bounded VM handoff and client asset reload.
Registry/worldgen changes remain restart-only.

## A13-09 — Upgrade migrations

Status: `BLOCKED-VP`

Depends on: storage, registries and world fingerprint.

Support item/block/entity/component/storage renames and transforms with dry run,
progress and interruption recovery.

## A13-10 — Uninstall policy executor

Status: `BLOCKED-VP`

Depends on: `A13-09`.

Apply declared conversion/archive/despawn policies. Never leave an unresolved
registry ID silently in a world.

## A13-11 — Plugin API `0.6.0` compatibility adapter

Status: `BLOCKED-VP`

Depends on: stable API 1.0 primitives.

Host existing bounded plugins through adapters where safe. Preserve their
current semantics without making old special-case calls the new core model.

## A13-12 — Documentation and generated reference

Status: `BLOCKED-VP`

Depends on: `A13-01`.

Generate API docs, capability tables, manifest/schema reference, failure codes,
examples, migration guides and operator diagnostics.

Acceptance slice:

- intentionally slow server and client addons are isolated under load;
- replay reproduces one transaction/event sequence;
- config and asset reload preserve the last valid state on failure;
- one API `0.6.0` example runs through the compatibility adapter;
- uninstall dry run reports every affected durable object.

---

# A14 — Reference addons and API 1.0 release gate

Goal: prove the platform through complete vertical addons rather than API surface
count.

## A14-01 — Essentials-class reference addon

Status: `BLOCKED-VP`

Depends on: `A05`, `A04`, `A09` as needed.

Implement:

- homes;
- warps;
- teleport requests;
- kits;
- mail;
- chat formatting;
- economy and permission service consumption;
- offline player records.

Definition of done:

- restart and reconnect gates;
- provider replacement requires no addon code change;
- every denial is typed and auditable.

## A14-02 — Towny/Kingdoms-class reference addon

Status: `BLOCKED-VP`

Depends on: `A05`, `A11`.

Implement:

- claims;
- towns/kingdoms as Luau records;
- groups/ranks;
- banks/taxes/upkeep;
- diplomacy and conflict state;
- policy-driven world actions;
- management UI and map projection.

## A14-03 — Recruits-class reference addon

Status: `BLOCKED-VP`

Depends on: `A08`, `A09`, `A11` regions/relations.

Implement persistent recruit ownership, roles, squads, formations, orders,
combat relation policy, custom screen, model and animation.

## A14-04 — MythicMobs-class reference addon

Status: `BLOCKED-VP`

Depends on: `A08`, `A09`.

Implement multiple archetypes, reusable ability graphs, projectile, spawn
conditions and one multi-phase boss.

## A14-05 — MineColonies-class reference addon

Status: `BLOCKED-VP`

Depends on: `A08`, `A10`, `A11`.

Implement 100 citizens, professions, buildings, work orders, requests,
warehouse/courier flow, research and management UI.

## A14-06 — Twilight Forest-class reference addon

Status: `BLOCKED-VP`

Depends on: boot mode, `A08`, `A09`, `A10`.

Implement one complete custom dimension loop: portal -> biome/structure -> mobs
-> boss -> progression unlock -> return.

## A14-07 — Create-class reference addon

Status: `BLOCKED-VP`

Depends on: `A07`, `A09`, `A12`.

Implement kinetic generation, shafts, stress, one processing machine, mounted
inventory, moving assembly and guide scene.

## A14-08 — WorldEdit-class reference addon

Status: `BLOCKED-VP`

Depends on: `A04-09`, structures, UI/commands and permissions.

Implement selection, masks, patterns, clipboard, progress, cancellation and
undo.

## A14-09 — Multi-addon interoperability world

Status: `BLOCKED-VP`

Depends on: `A14-01` through `A14-08`.

Run a world with all reference addons enabled. Prove:

- deterministic dependency/provider resolution;
- namespace isolation;
- shared economy/permissions;
- region policies affect machines, NPCs and bulk edits correctly;
- no cross-addon token, storage, UI or network access;
- restart and reconnect preserve state.

## A14-10 — Scale and soak matrix

Status: `BLOCKED-VP`

Depends on: `A14-09`.

Required gates include:

- 100 active persistent citizens;
- 1,000-node kinetic graph;
- large region policy set;
- moving assembly under nearby players;
- custom dimension generation;
- multiple client screens and overlays;
- disconnect/reconnect and server restart;
- constrained CPU/memory runs;
- long soak with no unbounded queue or storage growth.

## A14-11 — Security and package corpus

Status: `BLOCKED-VP`

Depends on: full platform.

Run malformed archives, schemas, assets, network payloads, world state,
migrations, render submissions, service graphs and dependency graphs.

## A14-12 — API 1.0 release decision

Status: `BLOCKED-VP`

Depends on: every prior acceptance task.

API 1.0 may be declared stable only when:

- all eight reference addons close their vertical acceptance loops;
- the multi-addon world, scale, soak and security gates pass;
- dynamic mode passes Fabric/NeoForge/Forge;
- boot mode passes required restart/fingerprint flows;
- no public engine API contains a reference-addon domain concept;
- all durable content has migration and uninstall behavior;
- operator and developer documentation is complete.

---

# 5. Tasks explicitly deferred beyond API 1.0

The following are not hidden requirements for the first stable API:

- arbitrary Java bytecode supplied by addons;
- native Rust/C/C++ addon libraries;
- unrestricted filesystem, sockets or HTTP;
- raw OpenGL/Vulkan access;
- unrestricted custom shader source;
- Forge/Fabric/Bukkit binary API emulation;
- client-authoritative gameplay;
- unbounded per-block or per-entity Luau tick callbacks;
- transparent loading of worlds with missing persistent addon definitions.

A later API may add a separately trusted outbound HTTP capability or custom
shader source only after a new threat model and explicit operator/client consent.

# 6. Suggested first implementation slice after the gate opens

This is a future cursor, not permission to start now.

Select `A00-03` through `A02-07`, then close one narrow vertical package:

```text
validated addon.toml
-> shared schema
-> strict server Luau
-> typed future
-> one typed post-commit result
-> package validation command
```

Do not start with custom mobs, dimensions or rendering. Without stable package
identity, schemas, capabilities and futures, those features would reproduce the
current special-case growth under a larger name.

# 7. Current development cursor

The next active engineering task remains outside this backlog and is owned by
[`playable/ACTIVE.md`](playable/ACTIVE.md):

1. continue village defence;
2. replace the next supported `UnsupportedSpecial` mob profile with its real
   species-specific attack path, or keep it visibly unsupported;
3. add the exact authoritative and client-visible evidence required by the
   existing vanilla-parity workflow;
4. repeat the vanilla-parity queue before activating any task above.
