# ADR 0009 - Regional simulation behind the plugin boundary

**Date:** 2026-07-22
**Status:** Accepted, staged implementation

## Problem

Solaris is moving mutable world and entity simulation to regional single-writer
owners. Exposing that ownership model to Luau would force every plugin author to
handle migration, concurrency, stale references, and cross-region commit. A
globally mutable lock-free world would avoid visible regions only by moving the
same consistency problem into atomics and retries.

## Decision

Use a hybrid ownership model:

- regions own spatial simulation: entities, blocks, fluids, local physics, and
  other state whose authority follows world position;
- global actor services own non-spatial state such as plugin storage, economy,
  permissions, claim definitions, and plugin lifecycle;
- each Luau plugin has isolated state and serial handler semantics; the current
  runtime multiplexes those states on one shared host thread;
- the stable plugin boundary contains owned immutable events, bounded command
  batches, targeted completion events, and typed transactions;
- region keys, epochs, owner handles, ECS references, locks, sockets, and Rust
  pointers are never part of the stable plugin API.

The server routes an admitted command to the current owner and validates the
observed session/entity generation at commit. Region migration is invisible to
the plugin. Results return as exact targeted events. A future coroutine/await
helper may wrap those events but cannot introduce polling or elapsed-time
success.

Source ownership (2026-09-06): first-party Luau packages live in the independent
sibling `solaris-default-plugins` repository. Production core discovers deployed
packages through the existing `plugins.directory` boundary; it no longer embeds
first-party source or exposes `plugins.bundled`. Strict/expected deployment
validation still covers the complete discovered set. Core builds without the
plugin checkout; integration tests explicitly read that checkout to exercise
actual first-party behavior. This relocation does not change Lua API 0.6.0 or
grant first-party packages access beyond the ordinary plugin boundary.

### Component migration (2026-09-16)

The owner-selected successor is WIT `solaris:plugin@0.7.0` with Wasmtime in
`mc-plugin-host`. The composition root selects one runtime per deployment, not
both. Each component has its own Store; callbacks run serially on the host
worker, outside simulation owners, and publish validated batches through the
same `ScriptBoundary`. The Luau implementation and reload descriptions below
remain applicable to the retained Luau path, not promises about component reload.

Component zone commands reuse the existing capability check, zone registry and
plugin-scoped owner identity. Their completion is the owner's zone-keyed
`applied | refused` bit, delivered only to that plugin; WIT does not invent a
request id, detailed refusal, new registry or persistence mechanism. Component
zone-membership observations remain unimplemented.

Component messaging uses that same admission path for direct messages,
broadcasts and session-scoped disconnects. The host publishes only after the
entire callback, including canonical post-return cleanup, succeeds. A malformed
batch retires only its guest; transient admission pressure discards the whole
batch without retiring it. Disconnects retain the supplied session identity,
so a delayed request cannot disconnect a newer connection of the same player.

Component timers are private per-instance host state driven by the existing
pushed simulation ticks; there is no new scheduler thread, mailbox, journal or
simulation-owner API. The pure bounded schedule is shared with the retained
Luau adapter. One advancing tick stages all of its due callbacks under one
fuel/epoch/command allowance, counting timer mutations as well as game commands.
The host commits that schedule only after the entire delivery and ordinary
batch admission succeed. A non-retiring refusal leaves timers pending for a
later tick; guest memory/logging are not rolled back. Retirement drops the
schedule. This stronger admission boundary does not alter the retained Luau
path's existing commit-before-command-routing behavior.

P1 separates the contract from both VMs: `mc-script` owns deployment metadata,
events, commands, routes and admission, without VM dependencies or features.
The existing `mc-plugin-host` owns Wasmtime and the retained
`legacy_luau` module; its `legacy-luau` feature is migration-only. The server
enables that feature at composition, while production `mc-net` remains
runtime-independent.

Reload control shares the existing bounded host-input FIFO with events. A
`ScriptHostInputSender` wrapper sends a trusted host-only opaque reload payload;
the selected host downcasts it to its own reload request. This is not a guest
API, WIT value, new mailbox or general control framework. The payload's reply
channel closes if unsupported or dropped. `commit_reload` retains the same
admission fencing, route replacement and host-swap order. Existing Luau reload
behavior is preserved; component reload is still P6 work.

Migration is incomplete. P2's graphical hello/join acceptance is recorded in
`.analysis/codex-logs/wasm-p2-live-20260916/checkpoint.json`. P3 adds component
timers, but remaining operations, Loader/startup metadata, precommit hooks,
reload and final Luau removal still need their own acceptance evidence.

### Bounded precommit ownership (P5)

`mc-script::precommit` owns immutable build/damage contexts, operator-ordered
registrations, bounded admission, the queue-inclusive 100 ms deadline and
single-use approvals. Capacity is 64 in-flight decisions through native
consumption/drop, with at most 512 edits per build context. Build decisions are
Keep/Cancel; damage may replace the raw amount cumulatively. Cancel terminates
the handler chain. The host executes serially outside simulation ownership;
hook exports cannot use ordinary mutation, I/O or request imports.

`mc-net` retains the frozen native action and resumes it through the existing
bounded simulation queue. Each owner checks its snapshots, session, permissions
and declarative zone fence, then consumes approval immediately before its
ordinary commit. Regional build commits perform the same checks. Damage
continuations retain producer semantics, including melee/dragon kernels,
projectile impact ownership, explosion knockback and golem animation. A pending
projectile collision has one native adjudication; rejection must not resubmit
that collision on every tick.

Original programmatic completion senders remain with synchronous operations
and transfer only when an actual continuation is created. A native refusal
cannot be reported before a later hidden mutation. A failed mandatory guest
does not unregister protection; operator removal is separate from Store
retirement. No-handler actions retain their direct native path.

This records implementation ownership, not completed graphical acceptance or
release readiness. P5 evidence and remaining gates are tracked in `MEMORY.md`;
component reload and final Luau removal remain later migration outcomes.

## Event and mutation classes

Ordinary observations such as chat, death, zone entry, and completed world
changes are asynchronous immutable events. Their handlers may enqueue later
commands but cannot retain live world references.

Actions that must conserve state, including purchases, inventory exchanges,
teleports, and entity mutations, use typed host transactions. The host owns
routing, prepare/commit, rejection, and compensation. A plugin receives one
committed or rejected result and does not implement regional two-phase commit.

Hot admission rules such as land-claim build permission must not call Luau while
holding a region tick. Their owner service publishes an immutable versioned
policy index for local reads. Updating a rule changes that publication; normal
block admission remains local to the region.

Plugins register those rules through generic typed policy commands. The current
actor-or-operator zone policy carries an opaque plugin-scoped zone id, bounds,
and one normalized allowed actor UUID. Core routing never matches a plugin id or parses an
id convention; the plugin owns claim meaning and persistence.

Startup world generation follows the same boundary in declarative form. A
settlement-profile owner may publish one bounded immutable plan of known
building templates and roles, inhabitants, jobs, and plugin-scoped extension
ids. Startup validates and materializes that plan before generation; Luau never
receives a generator, chunk, region, or mutable-world handle. Entity
materialization enters a dedicated system-owned simulation command with
persisted villager type, profession, and level state. Its durable
per-inhabitant claim does not reuse ambient-herd admission, whose chunk-level
claim and payload have different semantics.

Startup gameplay rules (2026-09-09) extend that immutable boundary with an
optional package `rules.lua`. A separate sandbox evaluates it once against
package configuration under the existing source, memory, instruction, and
host-event budgets. Its bounded result defines native spawn groups and
placement, tree selection, and clay deposits; it supplies no callbacks to
entity ticks or chunk generation. A single plugin owns the plan. Startup
materializes it before generation and persists its resolved fingerprint in
the world contract. Adding, removing, or changing that plan requires a fresh
Solaris world and is rejected by live reload. Script formatting alone does
not change the resolved contract.

If an uncommon custom decision later needs synchronous-looking admission, its
adapter may suspend only the initiating action while the host processes it.
The region must continue ticking and may resume the action only from an exact
response with its original generation fence. This general suspension adapter
does not exist yet.

## Ordering and consistency

The current host consumes its bounded input FIFO on one thread and invokes one
handler at a time. Ordinary events and a prepared Luau generation replacement use
that same private host queue, so the replacement is a serial barrier for ordinary
queued events: inputs processed before it use the old generation and later inputs use
the new generation. Already emitted host commands remain committed output and keep
their admission tickets across the barrier. Coalesced `server.tick` delivery retains
its existing latest-value semantics rather than being redefined as a strict FIFO
record. Each plugin therefore observes serial handler execution without pretending the
server is single-threaded. If the host is later split into per-plugin workers, each
plugin must retain its admitted ordering and reload barrier semantics.

The asynchronous and dedicated-thread receivers share one nonblocking dequeue
policy in `ScriptHostEndpoint::try_recv_input`. FIFO priority, coalesced-tick
fairness, monotonic tick filtering and close-time draining remain identical;
only the empty-queue wait differs (`recv().await` versus `blocking_recv()`).

Queries return immutable snapshots with an explicit observed revision. A
transaction rechecks that revision or the narrower generation named by its DTO.
Cross-region or cross-service atomicity is provided only by a typed transaction
whose host adapter defines the participants and rollback rules. The API does
not offer an unbounded general world transaction.

Parallel plugin handlers or region-local pure handlers may be added later only
as an opt-in API with isolated state and measured need. They are not the
default and cannot weaken per-plugin FIFO ordering.

## Consequences

- Plugin authors write serial handlers and typed commands without locks or
  region awareness.
- Slow Luau cannot stall a region tick; queue and instruction limits isolate the
  plugin.
- Economy and claims do not become spatial simulation state merely to fit the
  regional scheduler.
- The host must provide explicit transaction adapters for compound gameplay
  operations instead of exposing generic mutable world access.
- Regional ownership can change internally without breaking plugin code.

## Current implementation status

`mc-script` already provides isolated Luau states multiplexed by one serial host
thread, bounded immutable DTOs, capability-gated command batches, targeted
result events, instruction and memory limits, and generic typed protected
zones. Runtime failure isolation survives into a typed terminal `LuaHostExitReport`:
shutdown can distinguish a normal drained event-queue close from non-normal host
exits and enumerate bounded per-plugin disable diagnostics without exposing the VM,
queue, lock, or owner internals. Prepared reload uses that same private host-input
serialization point: component candidates are compiled, configured, and initialized
outside live stores under a static combined old/candidate guest-memory capacity;
their init effects remain staged. A committed replacement changes route registrations
to fresh monotonic generations before it swaps instances, so old timers and targeted
late results cannot enter a same-id component replacement. Unix `mc-server` exposes
strict SIGHUP preparation without replacing the `ScriptBoundary`.
player inventory transactions, teleports, durable resident handles, work and
orders. Colony identity, roles and durable domain intent remain plugin-owned;
runtime adapters receive bounded requests and return owner-scoped results,
without exposing region keys, ECS references or pathing internals.
Entity spawn and resident work/orders enter simulation/regional owners; menu,
teleport, and standalone player-inventory commands enter the exact ordered
session lane. Standalone inventory transactions now plan against live
session-owner state and update its durable mirror before publishing a result,
instead of mutating persistence from the script router. The compound
inventory/storage transaction remains an explicit typed coordinator with an
internal session gate shared with standalone inventory owner commands; this
keeps their plan, durable mutation, and ordered owner application serialized.

Package discovery type-checks all callable `solaris` functions against the
check-only `mc-plugin-host/src/legacy_luau/solaris.d.luau` declarations. Known-name/argument errors fail
before VM startup, including in handlers not executed by `--check`. Dynamic
`any` values, advanced record shapes, resource bounds and authority checks remain
runtime responsibilities. The declarations do not add a runtime SDK or change
the asynchronous command/result boundary.

The existing world journal owns the compound commit decision: its inventory
frame contains canonical named-item player after-images and the prepared ledger
batch. Storage and playerdata are durable projections, completed before live
inventory publication. Player after-images include cursor and open crafting,
enchanting and merchant inputs. `SolarisInventoryWorldJournalLsn` fences replay
and stale saves; plugin-storage revision is a separate ordering domain.
The live projection uses the existing server save coordinator. Startup replays
chunks and inventory decisions before save/gameplay admission, even without Lua.
Checkpoint readiness is reconstructed by that replay, not plugin event delivery.
Unprojected decisions remain retained. Uncertain commit/projection marks the
canonical inventory recovery-required while its mutex is held and signals the
existing world fail-stop path. Native/session/regional commit preconditions
check the inventory fence before effects, including callbacks reacquiring the
mutex after planning. This is a concrete compound recovery mechanism, not a
claim of global gameplay atomicity.

The `storage_batches` feature adds owner-scoped operation receipts and bounded
snapshot pagination behind the same storage actor. Successful batch identity
and fingerprint remain durable after acknowledgement and compaction; uncertain
synchronization leaves recovery responsible for the outcome. Scan cursors hold
bounded immutable snapshots, not live mutable world references. Owned endpoint
transfers/reservations and resident adapters are not implemented by this change.

The actor protection path still reads the bounded
registry mutex; explosion planning, bounded random-fire planning, and baseline
normal-piston planning use an immutable snapshot. Piston edits are one atomic
base/head/destination group in both direct and scheduled-button paths. There is
no general coroutine wait API, custom-action suspension adapter, or published
versioned actor-policy index. This ADR fixes the architectural direction; it
does not claim that every gameplay transaction or publication stage is
complete.
