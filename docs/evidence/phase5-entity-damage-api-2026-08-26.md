# Phase 5 typed entity-damage / combat API — 2026-08-26

## Scope

This checkpoint adds the missing bounded common combat mutation/result surface for Phase 5 item 3. It intentionally does not expose player-melee internals, a general damage-source constructor, entity references, owner handles, locks, regional epochs, or direct persistence access.

API `0.6.0` adds:

- manifest capability `entity_damage`;
- Luau command `solaris.damage_entity(request_id, entity_id, amount)`;
- host command `ScriptCommand::DamageEntity` carrying a private-field `ScriptEntityDamageRequest`;
- required targeted result `entity.damage_result` / `on_entity_damage_result`;
- bounded public failure categories `busy`, `runtime_unavailable`, and `rejected`.

## Why this is not fake player melee

The plugin command does not invent a player attacker. It does not select a held item, game mode, attack cooldown, player exhaustion/durability cost, knockback origin, or villager-player gossip attribution. Those concepts remain owned by the real player-attack path.

The plugin operation is a server-owned non-player combat primitive routed through the existing generic server-entity attack kernel. That kernel already owns authoritative health mutation, hurt-invulnerability, health publication, death scheduling, and the normal server-entity kill reward path.

## Request contract

`ScriptEntityDamageRequest` contains only:

- `request_id`: normal bounded script id;
- `entity_id`: opaque numeric id already used by script observation/result DTOs, constrained to the runtime signed-32-bit entity-id space;
- `amount`: finite positive damage, capped at `1_000_000`.

The stable script command/event contracts remain `Eq`. Damage and post-commit health therefore store validated IEEE-754 `f32` bit patterns internally rather than weakening equality for every `ScriptCommand` / `ScriptEventKind`; public getters/Luau projection decode those exact bits back to numbers.

An undeclared `entity_damage` call traps before the bounded command batch crosses the host boundary. The plugin never passes its own plugin id; host-attached provenance selects the targeted result owner.

## Authority path

1. Luau validates request id/entity id/damage bounds and creates `ScriptEntityDamageRequest`.
2. Host command admission checks `entity_damage` and attaches unforgeable plugin provenance.
3. `ScriptRouter` converts only the already-bounded numeric entity id into the simulation entity id and calls `SimulationHandle::damage_script_entity`.
4. The server-owned simulation handle rejects session-fenced callers and enqueues `DamageScriptEntity` through the existing simulation command queue.
5. `SessionRegistry::damage_script_entity` accepts only a living non-item server entity, derives its normal kill rewards, and calls the existing `attack_server_entity_locked` generic combat kernel with no player attribution and no knockback origin.
6. The simulation owner publishes the kernel visibility dispatches and returns only `ScriptEntityDamageCommit { health, killed }` across the script adapter boundary; `EntityAttackOutcome` itself never enters `mc-script`.
7. The router converts the owner result into exactly one required targeted `entity.damage_result`.

The new response logic was extracted from the large simulation batch gateway. `xtask code-health` therefore remains within the architecture budget instead of adding combat semantics inline to the orchestration switch.

## Result semantics

Success carries the original request identity/entity id/raw amount plus:

- `damaged = true`;
- authoritative post-commit `health`;
- exact `killed`;
- `failure = nil`.

The result DTO is closed: success must contain finite non-negative health; a killed success must have health zero. Failure contains no health, reports `killed = false`, and has a non-nil failure.

Public failure taxonomy:

- simulation queue full / admission timeout -> `busy`;
- closed/stopped/response-timeout/shutdown/world-unavailable owner path -> `runtime_unavailable`;
- missing/non-living target, hurt-invulnerability, response mismatch, invalid command, or other definite owner rejection -> `rejected`.

This result reports simulation-owner combat commit. It does not claim that every client received/rendered a health/death packet.

## Executable evidence

### `mc-script`

`cargo test -p mc-script --features lua-runtime entity_damage -- --nocapture`:

```text
running 3 tests
...
test result: ok. 3 passed; 0 failed
```

Those tests prove:

- capability-gated typed command construction;
- exact request id/entity id/damage projection;
- success and failure targeted result fields;
- invalid zero/oversized damage and out-of-range entity ids reject before mutation;
- missing capability traps before batch admission;
- a disk plugin declaring `capabilities = ["entity_damage"]` emits a real host-attested request.

The full Luau feature suite on this checkpoint is 204/204 PASS.

### `mc-net`

`cargo test -p mc-net --lib script::router::entity_damage_tests -- --nocapture`:

```text
running 2 tests
...
test result: ok. 2 passed; 0 failed
```

The round-trip test creates two ordinary cow server entities through a test-only wrapper around the production spawn authority, then loads a strict disk plugin which emits three host-attested damage requests in one batch:

1. non-lethal `hurt`: simulation owner reduces authoritative health by exactly `2.0`; targeted result asserts `damaged=true`, non-nil health, `killed=false`, no failure;
2. lethal `kill`: the same generic combat kernel commits health zero and leaves the entity non-`Alive`; targeted result asserts `damaged=true`, health zero, `killed=true`, no failure;
3. missing entity id: owner returns no combat commit; targeted result asserts `damaged=false`, nil health, `killed=false`, `rejected`.

A second focused test pins the `busy` / `runtime_unavailable` / `rejected` mapping from simulation request errors.

The test-only spawn helper is `#[cfg(test)]`; it exposes no new production session/entity API.

The full `mc-net --lib` suite on the pre-review checkpoint is 1971 passed / 5 ignored / 0 failed.

## Phase 5 item 3 six-domain matrix

| Domain | Public typed surface already proven | Authority/result boundary |
| --- | --- | --- |
| Player | player observations/query plus `teleport_player` and targeted teleport result | session/runtime teleport authority; no session handle in Lua |
| World | `set_world_time` + targeted `world.time_set_result` | simulation owner; [`phase5-world-time-api-2026-08-26.md`](phase5-world-time-api-2026-08-26.md) |
| Block | bounded default-state `set_block` + targeted `world.block_set_result` | registry validation + simulation-owner block-edit lane; [`phase5-world-block-api-2026-08-26.md`](phase5-world-block-api-2026-08-26.md) |
| Inventory | typed player inventory and inventory/storage transactions with semantic result events | session inventory/storage authority; dedicated `mc-script` and `mc-net` transaction tests |
| Combat | `damage_entity` + targeted `entity.damage_result` | generic server-entity combat kernel through simulation owner; this evidence |
| Entity | correlated `spawn_entity` + targeted `entity.spawn_result` | actor-session fence + simulation owner; [`phase5-entity-spawn-api-2026-08-26.md`](phase5-entity-spawn-api-2026-08-26.md) |

The matrix demonstrates a common public typed surface in all six domains named by Phase 5 item 3 without exposing mutable engine internals. More domain APIs can still be added later; item 3 does not require every possible action in every domain.

## Quality gates before review

- `cargo test -p mc-script --features lua-runtime --quiet` — 204/204 PASS;
- `cargo test -p mc-net --lib --quiet` — 1971 passed / 5 ignored / 0 failed;
- focused Luau entity-damage tests — 3/3 PASS;
- focused router combat tests — 2/2 PASS;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS after removing one redundant closure;
- scoped `git diff --check` — PASS.

Benchmark: not applicable. This is an explicit plugin mutation path using the existing simulation owner and generic entity combat kernel; it adds no steady-state work when no plugin issues damage requests.

## Independent review

Exactly one bounded independent read-only reviewer returned `CHANGES` with one Low documentation-only finding: this evidence still said the scoped diff-check was pending even though the supplied primary result was PASS. The reviewer found no combat/API/authority defects and explicitly confirmed that the six-domain matrix is sufficient for the Phase 5 item-3 wording. The stale line was corrected above. Per repository policy, no second reviewer was run after fixing this finite finding.

Post-fix self-validation keeps formatter, code-health, strict workspace Clippy, affected suites, and scoped diff-check green.

## Disposition

Typed common combat subdomain: **PASS**.

Phase 5 item 3: **CLOSED**. The public API now has typed authority-preserving result-bearing surfaces for player, world, block, inventory, combat, and entity domains without exposing mutable engine internals.
