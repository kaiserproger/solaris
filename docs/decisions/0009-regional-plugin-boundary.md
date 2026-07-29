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

If an uncommon custom decision later needs synchronous-looking admission, its
adapter may suspend only the initiating action while the host processes it.
The region must continue ticking and may resume the action only from an exact
response with its original generation fence. This general suspension adapter
does not exist yet.

## Ordering and consistency

The current host consumes its bounded event FIFO on one thread and invokes one
handler at a time. Each plugin therefore observes serial handler execution
without pretending the server is single-threaded. If the host is later split
into per-plugin workers, each plugin must retain its admitted FIFO order.

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
zones. `mc-net` already has production adapters for storage, zones, menus,
player inventory transactions, teleports, colonies, and villager bindings.
Entity spawn and villager work enter simulation/regional owners; menu,
teleport, and standalone player-inventory commands enter the exact ordered
session lane. Standalone inventory transactions now plan against live
session-owner state and update its durable mirror before publishing a result,
instead of mutating persistence from the script router. The compound
inventory/storage transaction remains an explicit typed coordinator with an
internal session gate shared with standalone inventory owner commands; this
keeps their plan, durable mutation, and ordered owner application serialized.
The actor protection path still reads the bounded
registry mutex; explosion planning, bounded random-fire planning, and baseline
normal-piston planning use an immutable snapshot. Piston edits are one atomic
base/head/destination group in both direct and scheduled-button paths. There is
no general coroutine wait API, custom-action suspension adapter, or published
versioned actor-policy index. This ADR fixes the architectural direction; it
does not claim that every gameplay transaction or publication stage is
complete.
