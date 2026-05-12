# Project Spec: Custom Minecraft Engine

**Codename:** TBD (decide when creating the repository — it affects namespace in protocol channels, item IDs, etc.)

**Document version:** 0.1 (drafting)
**Last updated:** at creation
**Author/owner:** you

---

## 0. Purpose of this document

This is a **reference for yourself**, not a marketing PRD or a client contract. It answers the question "why did I make this decision 8 months ago" when you return to the code after a break or want to change something fundamental.

This document is mandatorily reviewed at milestones M3, M6, M9, M12. Reality will diverge from the plan — that's normal. The important thing is to record divergences and reasons, not to pretend the plan never changed.

---

## 1. Vision and non-goals

### 1.1 What we're building

A custom Minecraft-compatible game engine in Rust, consisting of:

- **Server** — authoritative server implementing vanilla Minecraft Java protocol 26.1 + a custom protocol extension
- **Client mod** — a Fabric/NeoForge mod in Java, extending the vanilla 26.1 client to support dynamic registration of custom blocks/items/entities/effects via the protocol extension
- **Resource pack** — a vanilla resource pack delivered by the server, containing models/textures for base content

End artifact: a **playable server** that vanilla clients with the client mod installed can connect to, and where they see a replica of the modpack (Medieval Siege Machines, Farmer's Delight, Macaw's, Villager Recruits/Workers, Epic Knights, MusketMod) or a close approximation.

### 1.2 Why

Reasons in order of importance:

1. NeoForge server performance under heavy modpacks with many mobs and players is a real bottleneck for some scenarios (large RPG/PvP servers). A native Rust server addresses this.
2. Hard dependency of modpacks on specific mod authors and their port schedules — a fragmentation point in the NeoForge ecosystem. A custom engine removes this dependency.
3. Using this modpack as the first major testbed surfaces real extensibility requirements rather than theoretical ones.
4. The project is interesting in its own right.

### 1.3 Non-goals (what we are NOT doing)

Listed explicitly so scope doesn't creep:

- **NOT bit-perfect vanilla parity.** Not in worldgen, not in redstone tick order, not in RNG. Goal is "mechanics behave as expected", not byte-for-byte vanilla matching.
- **NOT compatibility with Forge/Fabric/NeoForge server-side mods.** No JNI bridge for running NeoForge mods on this server.
- **NOT cross-platform below the minimum.** Server: Linux x86_64 as primary, Windows x86_64 best-effort, others on demand. Client mod: whatever 26.1 vanilla supports.
- **NOT Bedrock Edition.** Java Edition only.
- **NOT a full 1:1 modpack at M12.** Realistic goal for M12-M24 is a playable demo with a subset of each mod's mechanics.
- **NOT a custom launcher/installer in early stages.** Players use Prism/MultiMC + our mod jar.

### 1.4 Definition of done for the whole project

Version 1.0 is ready for public release when:

1. Server sustains 20 concurrent players at view distance 8 with > 18 TPS on a typical VPS (4 vCPU, 8GB RAM)
2. All 6 modpack mods are replicated at least at the level of core mechanics (see §6 for breakdown)
3. Client mod is published on Modrinth/CurseForge with an automatic installer
4. Server survives crash recovery without data loss
5. Full documentation: server admin setup guide, player setup guide, plugin API reference

---

## 2. Target platform and dependencies

### 2.1 Vanilla version

**26.1.x Java Edition** (Tiny Takeover, released 2026-03-24, the first version released fully unobfuscated).

The exact protocol version is locked at the start of M1 — for example, `26.1.2` (latest at that time). Mojang patch updates will be supported as minor engine releases with updated protocol constants.

### 2.2 Rust toolchain

- Stable Rust, MSRV is the latest stable at project start
- Edition 2024
- Async runtime: **tokio** (industry standard, best ecosystem and documentation)
- ECS: **bevy_ecs** standalone (without the rest of bevy) — mature, performant, no legacy

### 2.3 Java toolchain (for the client mod)

- Java 25 (26.1 requirement)
- Mod loader: **NeoForge for 26.1** when ready; until then, Fabric loader on 26.1 vanilla as fallback
- Mixins, Mixin AP — standard
- Build: Gradle with NeoGradle plugin

### 2.4 Critical external dependencies

Crates we fundamentally depend on. Replacing any of these is a migration:

- `tokio` — async runtime
- `bevy_ecs` — ECS
- `fastnbt` — NBT serialization
- `valence_nbt` or our own implementation — fallback
- `flate2` — compression
- `aes` + `cfb-mode` — protocol encryption
- `serde`, `serde_json` — for resource pack/data pack JSON
- `tracing` — logging
- `clap` — CLI

This list **must not** grow uncontrollably. Any new heavy dependency requires justification in a PR comment.

---

## 3. High-level architecture

### 3.1 Crates layout

A workspace with multiple crates for dependency management and testability:

```
mcengine/
├── crates/
│   ├── mc-protocol/          # wire protocol: packets, codec, encryption
│   ├── mc-nbt/               # NBT helpers (on top of fastnbt where needed)
│   ├── mc-world/             # block states, chunk format, world storage
│   ├── mc-worldgen/          # generation pipeline, biomes, structures
│   ├── mc-physics/           # block physics, collisions, fluids
│   ├── mc-entity/            # entity system, AI, pathfinding
│   ├── mc-net/               # connection management, session lifecycle
│   ├── mc-data/              # data pack loader, registries, recipes
│   ├── mc-extension/         # custom protocol extension (for the client mod)
│   ├── mc-script/            # plugin/script API (Lua/WASM)
│   ├── mc-server/            # main binary: tying everything together
│   └── mc-test-harness/      # diff testing infrastructure (see §7)
└── client-mod/               # separate sub-project, Java/Gradle
```

Principles:
- `mc-server` is the only crate that knows about everything
- Other crates are as independent as possible
- No circular dependencies
- Public API minimized — `pub` only what's actually needed outside the crate

### 3.2 Architectural decisions made

**ECS vs OOP for entities.** Decision: ECS via `bevy_ecs`. Rationale: vanilla's entity hierarchy in Java is heavily inheritance-based; porting 1:1 to Rust would mean trait-object pain. ECS fits Rust ownership better, parallelizes better, makes adding components easier without refactoring base types. Cost: must think in data-oriented style.

**Async vs sync for the tick loop.** Decision: tick loop is **synchronous** in a single thread (a `bevy_ecs` Schedule), network tasks are async via tokio. World state is mutated only from the tick thread. Network → tick communication via crossbeam channels. Worker pool for heavy parallel work (chunk generation, pathfinding, light propagation) is a separate rayon pool, communicating via the snapshot capture pattern.

**Chunk storage.** Anvil-compatible region files as the primary format. Rationale: compatibility with external tools, ability to dump vanilla worlds for tests, two-way migration. In-memory representation is palette-based like vanilla, but with our own data structures (potentially SoA layout for cache locality).

**Block state representation.** Global numeric ID like vanilla (`u32`, the protocol requires it), resolved through a registry at startup. Block state properties as `Vec<(String, String)>` for flexibility, with typed accessors via generated code on the hot path.

**Threading model.** See §4.

**Protocol extension layout.** See §5.

### 3.3 What does NOT go into ECS

Not all game state lives in ECS. Outside ECS:

- **Chunk data** — own structure (HashMap<ChunkPos, ChunkData>), because access is by coordinate, not by entity id
- **Block entity data** — inside chunks
- **Player connections** — a separate registry, because lifecycle is tied to network state, not game state
- **Static registries** (block types, item types, biomes) — read-only after init

ECS is used for: mobs, dropped items, projectiles, particles (server-side), experience orbs, other "moving things".

---

## 4. Threading model

This is critical to design early because the wrong choice here is the most expensive mistake to fix later.

### 4.1 Threads

- **Main game thread** — tick loop, the sole writer to world state
- **Network IO** — tokio runtime, M threads. Receives packets, parses, sends to a `network → game` queue. Receives `game → network` events and writes to TCP.
- **Compute pool** (rayon) — N threads for:
  - Chunk generation (read-only world snapshot)
  - Pathfinding (read-only world snapshot)
  - Light propagation (read-only world snapshot, returns a diff)
  - Heavy worldgen feature placement
- **IO pool** — separate tokio task for disk operations (region file save/load)
- **Plugin thread(s)** — Lua/WASM execution, isolated, communicates with main via message passing

### 4.2 Snapshot pattern

For compute pool tasks that need to read world state:

1. Main thread captures a snapshot of relevant chunks at a specific tick (read-only `Arc<ChunkData>`)
2. Passes it to a compute pool task with the request
3. Task returns the result via a channel
4. Main thread on a subsequent tick applies the result, validating that world state hasn't drifted too far

More detail in §5.

### 4.3 What you must not do

- No mutexes on shared world state. If you need one, that's a red flag in the architecture.
- No async tasks blocking on the game thread.
- No `tokio::spawn` inside the tick loop.

---

## 5. Custom protocol extension (for the client mod)

### 5.1 Concept

We use the vanilla `custom payload packet` mechanism (sometimes "plugin channels"). Server and client exchange arbitrary packets on namespaces. The vanilla client ignores unknown channels; our client mod listens to its own namespace.

### 5.2 Namespaces

- `mcengine:handshake` — version negotiation at login
- `mcengine:registry_sync` — dynamic registration of custom types
- `mcengine:block_update` — extensions to vanilla block update packet for custom block states
- `mcengine:entity_update` — custom entity transforms, animations
- `mcengine:gui` — custom GUI screens
- `mcengine:effect` — custom particles, sounds, screen effects

### 5.3 Versioning

Each message in our custom protocol has:

```
{
  schema_version: u16,    // bump on breaking changes
  message_type: u16,      // discriminator
  payload: bytes
}
```

Server and client mod exchange supported schema versions during handshake. If the client mod is outdated — graceful degradation: server sends only base vanilla packets, custom features are unavailable to that player, others can still use them.

### 5.4 Fallback for vanilla client without the mod

If the client mod is not installed — the player sees a **vanilla approximation**:
- Custom blocks → nearest vanilla block with a matching texture via resource pack note block hack (where applicable)
- Custom items → vanilla items with `custom_model_data`
- Custom entities → display entities on a vanilla mob base

This is significant work, but provides inclusivity. Decision: **do not implement fallback in M0-M12, require client mod.** For M12-M24 — graceful fallback as a separate large milestone.

### 5.5 Anti-cheat

Server remains authoritative. Client mod only renders and predicts. Server-side validation:

- All player actions (place block, break block, use item) are validated on the server independently of the client
- Position updates: vanilla anti-cheat plus server-side movement check
- Custom mechanics with client prediction (e.g. firing a musket): server immediately corrects on mismatch

---

## 6. Modpack replica scope

What from each mod we implement as acceptance criteria for project DoD.

### 6.1 Macaw's Mods

- ≥ 50 decorative blocks from core Macaw's (Furniture, Roofs, Doors, Bridges, Windows, Trapdoors, Fences)
- Sit-on-chair works (player riding invisible entity pattern)
- All blocks with correct hitboxes and collision shapes
- Recipes available via vanilla crafting

### 6.2 Farmer's Delight

- ≥ 8 culinary tools (cutting board, cooking pot, stove, basket, sink)
- ≥ 30 food items with `custom_model_data` + custom food properties (saturation, hunger, effects)
- ≥ 4 crops (tomato, onion, rice, cabbage) with growth phases
- Cooking pot with a functional GUI and cooking recipes (chest GUI baseline through M17, custom GUI via client mod from M18)
- Knife crafting — ingredient → multiple outputs

### 6.3 Villager Recruits / Villager Workers

- Villager hire mechanic via GUI (currency → recruit follows you)
- Combat AI for recruits (follow owner, attack hostiles)
- Worker AI for workers: ≥ 3 professions (lumberjack, miner, farmer)
- Equipment slots on villagers (showing weapons and armor — visual approximation)
- Salary/feeding mechanic

### 6.4 Epic Knights

- ≥ 4 armor sets (full plate, chainmail, gambeson, helm) with custom 3D models via the `equippable` component
- ≥ 8 weapons (sword variants, halberd, spear, mace, shield variants)
- Shield blocking mechanic (vanilla-extended)
- Two-handed weapon mechanic for halberds (server-side: damage area + animation lock)
- Crafting recipes

### 6.5 Medieval Siege Machines

- ≥ 3 siege engines (catapult, trebuchet, ram)
- Multi-block construction via crafting + placement
- Player riding and control via input
- Projectile physics with block damage (TNT-like)
- Aim/elevation controls
- Reload mechanic with timing

### 6.6 MusketMod

- ≥ 3 firearms (musket, pistol, blunderbuss)
- Reload mechanic with time delay (`use_cooldown` component + animation hooks)
- Ballistics (start with hitscan, then ballistic projectile)
- Muzzle flash + smoke (vanilla particles)
- Bayonet attachment mechanic
- Ammo (powder + ball) consumption

### 6.7 Out of scope (even after M24)

- Mod compatibility patches (e.g. JEI integration)
- Complex client-side effects requiring shaders (if any mod has them)
- Exact damage/durability balance — approximation, not a copy

---

## 7. Testing

### 7.1 Approaches

**Unit tests** — for all pure functions: NBT codec, packet codec, noise functions, RNG. Coverage > 80% for these modules.

**Integration tests** — for each crate, testing the public API.

**Differential tests against vanilla** — spin up a vanilla 26.1 server in Docker alongside ours, run scenarios, compare outputs. More below.

**Property tests** via `proptest` — for invariants (RNG round-trip, NBT round-trip, palette compaction is idempotent, etc.).

**Smoke tests** — daily in CI: spin up the server, headless vanilla client (`azalea`), run a scenario, assert no crash.

### 7.2 Differential testing infrastructure

A separate milestone (M2.5 in the roadmap). Components:

- Vanilla 26.1 server in Docker, configurable via RCON
- Headless bot client (`azalea`) connecting to both servers
- Scenario DSL (RON or YAML) describing: starting world, sequence of actions, observations
- Snapshot serializer: after N ticks, dump world state in canonical form
- Diff engine with categorization: critical / suspicious / probably ok

### 7.3 Performance benchmarks

`criterion` benchmarks for hot paths:
- Chunk encoding/decoding
- Pathfinding
- Light propagation
- Block update cascades
- Tick loop throughput at N entities

Run daily in CI; regressions > 10% — alert.

---

## 8. Known risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Mojang breaks protocol in 26.x patch | Medium | High | Lock to specific 26.1.x, update on our schedule |
| NeoForge for 26.1 doesn't ship for a long time | High | Medium | Start on Fabric loader, switch when NeoForge is ready |
| ECS performance is worse than expected | Low | High | Early benchmarking at M3, willingness to move critical systems out of ECS |
| 26.1 worldgen is too complex for a reasonable replica | Medium | High | Start with simplified worldgen, iterate |
| Solo developer burns out around month 6 | High | High | 2-4 week milestones, tangible artifacts, public demos |
| Legal pushback from Mojang | Low | Catastrophic | No decompiled Mojang code in our repo, no parity claims, no asset reproduction |
| Rust server performance not better than NeoForge + Moonrise | Low | Medium | Early profiling, benchmark comparisons against baseline |

### 8.1 Legal posture

- We use wiki.vg protocol documentation and Minecraft Wiki for mechanics — public reverse engineering
- We do not use decompiled Mojang source in our code
- We do not use Mojmap mappings to inform our code — we write our own, working from documentation
- Bundled binary assets (textures, models, sounds, fonts) are original or community-made under permissive licenses
- **Vanilla *data* files** (the contents of `data/minecraft/**` inside
  the official server jar — registry JSON, tags, loot tables, recipes,
  worldgen JSON, structure NBT) **are permitted as build/runtime input**.
  See [ADR 0001](decisions/0001-vanilla-data-as-runtime-input.md) for
  the reasoning. They live under `data/vanilla/` and are reproduced by
  `tools/extract-vanilla-data.sh` from `.analysis/server.jar`.
- Players require a legitimate Minecraft license (connect via Mojang auth, like any server)

---

## 9. Milestone roadmap

High-level plan; details go in separate milestone docs as we approach each one. **All durations are in person-hours**, converted to calendar time at your 10-20h/wk pace.

### Phase 1: Foundation (M0-M3, ~250-400 hours = 3-6 months)

- **M0: Project bootstrap** (10-20h) — workspace, CI, base crates, "hello world" smoke
- **M1: Network + handshake** (60-100h) — TCP, packet codec, status, login through play state
- **M2: World representation** (80-120h) — block registry, chunk data, palette, NBT round-trip
- **M2.5: Differential testing infra** (80-120h) — vanilla in Docker, bot driver, scenario DSL, snapshot diff
- **M3: Empty world ready** (40-60h) — client connects, sees a flat/empty world, can walk, no crash after 1 hour

### Phase 2: Single-player viable (M4-M7, ~400-600 hours = 5-8 months)

- **M4: Worldgen baseline** (120-180h) — terrain noise, biomes, simple structures, no vanilla parity
- **M5: Block physics + fluids** (80-150h) — gravity, water/lava flow, basic block updates
- **M6: Player actions** (60-100h) — block break/place, inventory, basic survival mechanics
- **M7: First playable demo** (80-150h) — can run, explore, survive; internal alpha

### Phase 3: Multiplayer + extensibility (M8-M11, ~300-500 hours = 4-6 months)

- **M8: Multiplayer** (80-120h) — multiple players, view distance, chunk streaming
- **M9: Data pack loader** (60-100h) — vanilla data pack format: blocks/items/recipes/loot tables from JSON
- **M10: Custom protocol extension v1** (80-150h) — handshake, registry sync, basic custom blocks/items
- **M11: Plugin API (Lua)** (80-130h) — event bus, scriptable behaviors

### Phase 4: Modpack replica part 1 (M12-M16, ~400-700 hours = 5-9 months)

- **M12: Client mod scaffolding** (100-150h) — Fabric/NeoForge mod, mixins infrastructure, version handshake
- **M13: Macaw's replica** (80-120h) — simplest mod, exercises the whole extension pipeline
- **M14: Farmer's Delight replica** (100-150h) — custom blocks, items, cooking
- **M15: Epic Knights replica** (120-180h) — custom armor models via equippable component, weapons
- **M16: Public alpha release** (50-100h) — Modrinth publication, documentation, installer

### Phase 5: Modpack replica part 2 + polish (M17-M24, ~400-700 hours = 5-9 months)

- **M17: Villager Recruits/Workers replica**
- **M18: Custom GUI framework via client mod**
- **M19: Medieval Siege Machines replica**
- **M20: MusketMod replica**
- **M21: Performance pass** (profiling, optimization, native chunk I/O)
- **M22: Anti-cheat + server-authoritative validation**
- **M23: Developer documentation + plugin API stable**
- **M24: 1.0 release**

### 9.1 Critical gates

**After M3:** if the client doesn't connect reliably or performance is dismal — STOP, reconsider the architecture. Do not move forward on a rotten foundation.

**After M7:** if the game is unplayable even solo — STOP, reconsider scope. The goal may need to be reduced.

**After M12:** if the client mod doesn't work on a fresh 26.1.x patch — reconsider the strategy (graceful fallback? versioning policy?).

---

## 10. Workflow and tooling

### 10.1 Working cycle

Recommended cadence for solo part-time:

- **Monday evening:** review last week, plan current week
- **Tue-Thu, 3-4h each:** implementation tasks from the current milestone
- **Sat-Sun:** larger blocks, refactoring, testing, documentation
- **Every 2 weeks:** demo session — run, play 30 minutes, write down what doesn't work

### 10.2 Using Claude Code

Claude Code is used for milestone-level tasks (10-40 hours when fully scoped). Not for architecture. Not for parity tasks without an oracle. The principle:

1. A milestone is extracted from the project (see individual milestone documents)
2. A prompt is built from the template (see CLAUDE_CODE_PROMPTS.md)
3. Claude Code works on a feature branch
4. You review, test, merge manually into main
5. Between milestones — you do the work. Architecture, integration debugging, vanilla-comparison testing.

### 10.3 Git workflow

- `main` — working branch, CI always green
- `dev/MX-name` — feature branches for milestones
- Tags at each milestone: `m0`, `m1`, …, `v1.0`
- Conventional commits (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`)
- PR review — yourself. Sleep on it between writing and merging.

### 10.4 Documentation

- `README.md` — what this is, how to run it, for users
- `CONTRIBUTING.md` — for future contributors
- `docs/architecture.md` — this file (updated when architecture changes)
- `docs/milestones/MX.md` — one per milestone
- `docs/decisions/` — ADRs (Architecture Decision Records) for nontrivial decisions
- Doc comments in code (`///`) — on the public API

---

## 11. Open questions

Decisions deferred until more context is available:

- [ ] Format for cape/cloak/wing attachments on players? Decided in M15.
- [ ] Plugin API schema — Lua DSL or embedded full Lua? Decided in M11.
- [ ] Custom dimensions support via client mod — needed for 1.0? Decided in M19.
- [ ] Telemetry (opt-in) for performance regressions — should we have it? Decided in M16.
- [ ] Hosting docs (gitbook? mdbook? plain HTML?) — decided in M16.

---

## 12. Changelog

| Date | Version | Change | Who |
|---|---|---|---|
| at creation | 0.1 | Drafted from initial discussion | you |

---

**End of document.**
