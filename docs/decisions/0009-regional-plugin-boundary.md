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

`mc-script` provides bounded immutable DTOs, capability-gated command batches,
targeted result events, and generic typed protected zones. `mc-plugin-host`
executes Wasmtime components with one Store per plugin on a serial worker, under
fuel, epoch, and memory limits. A retired guest loses its routes without gaining
access to owner queues or locks. Prepared reload uses the private host-input
serialization point: component candidates are compiled, configured, and initialized
outside live stores under a static combined old/candidate guest-memory capacity;
their init effects remain staged. A committed replacement gives routes fresh
monotonic generations before it swaps instances. Candidate init commands
reserve queue and admission before the switch; their tickets bind to the
committed registration under the admission lock before publication, not when
the server eventually consumes the command. Events queued before the swap
retain their admission generation; results constructed from an admitted plugin
storage, compound inventory/storage, or typed operation request also retain
the issuing command's generation, including when the owner replies after the
swap. The retired Store cannot receive a replacement's result, and the new
Store cannot receive that old request's result under the same plugin id. A
committed operation remains queryable through its durable id if its callback
is lost; the host does not reinterpret a missing callback as rollback. This
generation fence is not a claim about events without an admitted request.
Unix `mc-server` exposes strict SIGHUP preparation without replacing the
`ScriptBoundary`. Native owner routes include player inventory transactions,
teleports, durable resident handles, work and
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

The server-owned warehouse-to-resident equipment/carry transfer has no player
session participant (`actor_id = 0`). The inventory owner resolves the plugin's
bound container and live resident handle, checks both endpoint fences and
reserved stock, then journals the physical container after-image, changed
resident gear record and operation receipt under one decision. A gear-only
record update advances its inventory fence without replacing its work, squad
order or their nested revisions. Settlement role, employer and kit policy stay
in the Wasm guest: it CAS-persists the exact transfer intent before issuing the
native move and CAS-finalizes the role only after the committed receipt. On
reload it reissues the same operation id and fenced content; native receipt
replay precedes fence checking, so a lost callback cannot pay or equip twice.

Dismissal reverses that same inventory composite, not its authority. The guest
persists source-stack components and destination slots for the issued kit
separately from the resident's personal wage. It reads the resident equipment
fence (not the independent resident-identity revision), then CAS-persists a
`Stop` intent before the native owner cancels any active goals and work and
records `Civilian` or `Military → Demobilizing` without moving items. It snapshots
the bound warehouse and resident slots, matches only the issued stacks by
components and slot, CAS-persists a fenced `Return` intent, then requests a
zero-actor equipment/carry-to-warehouse transfer. The owner refuses a fresh
transfer when the bound entity cannot be confirmed present; a committed receipt
still replays before a later lifecycle check. The owner journals the container
after-image, changed resident gear and replay receipt as one decision. A lost
reply replays the same operation id; the guest queries the current resident
equipment fence after the return, so resumed owner work cannot stale the
durable `Finish` intent and second native transition to `Civilian`. That owner
transition rejects a dead, converted or stale resident; only after its receipt
does the guest CAS-clear the role and issued kit. An unavailable destination
leaves the same resident demobilizing with gear intact. Identical replacement
stacks cannot be distinguished without an item-identity API; item resource,
enchantments, custom name, model and nondecreasing wear avoid seizing visibly
different personal gear. No plugin-owned inventory ledger, synthetic returned
item or second population identity is introduced.

Paid resident treatment is also a typed compound owner operation, not a
standalone heal. The settlement guest checks owner/steward rights and the bound
warehouse, reads the current native resident revision, and chooses a fixed cost:
one bread for three health points. Core checks the live villager, actual native
max-health attribute, physical stock and reservation floors. It plans the
conditional regional snapshot containing both new health and the plugin
transaction-revision marker, then the simulation owner prepares that regional
phase inside the same turn as the chest command, after the region tick. Only
then does it append the chest after-image, plugin receipt and exact health
intent in one world-inventory decision. The regional journal commits and the
shared writer flushes the native decision; the world journal then appends and
syncs a checksummed `WAM1` acknowledgement linked to that inventory decision
before either health or chest slots are published or the operation acknowledged.
A definite refusal aborts the prepared regional phase; an uncertain world append,
failed regional commit or uncertain acknowledgement stops publication and fences
admission until restart. Startup projects the plugin receipt but retains the
world decision, restores the regional entity WAL, and either recognizes its
transaction-revision marker or journals the exact expected-to-next health
transition. It flushes and records the linked acknowledgement before projecting
the world decision. An absent resident can be projected only if that
acknowledgement survived entity WAL compaction; without it recovery fails closed.
The world checkpoint retains and re-encodes acknowledgements with unprojected
decisions. The guest cannot create health or supplies through a separate path.

Resident combat remains a typed owner operation. The settlement component
checks owner/steward authority and serving role, then issues a bounded hold
order and returns one server-issued hostile reference with its policy and
expiry revisions in an attack order; it never submits damage or movement per
tick. The resident owner checks live identity, equipment, reach, line of
sight and a persisted attack cooldown. Melee damage uses the existing entity
damage owner; a bow consumes one canonical arrow only when it launches an
owner-and-target-bound projectile into the shared physics/visibility path.
The projectile kernel, not the order response, determines later impact and
publishes the one hit; scripted arrows cannot collide with other entities.
An admitted attack retains one authenticated target reference in its durable
order. The storage actor subscribes to simulation-tick notifications and
rechecks only indexed active attackers after their persisted cooldown; native
movement into reach can therefore finish the same order without guest polling.
A combat-progress journal entry advances ammo/cooldown while preserving the
guest-visible order/work revisions; cancellation removes the active index.
Pre-upgrade target-bearing receipts decode with zero policy/expiry metadata
and cannot authorize a fresh attack without a newly issued reference.

For CP-047 operational morale, core advances a resident's durable categorical
state at most once per newly observed physical event: own health loss, a
distinct allied death, first exposure to an enemy on the flank, or loss of
the assigned officer. Each event moves steady → shaken → wavering → routing;
repeated ticks, the same casualty and continuous flank exposure do not advance
it again. Routing replaces chase with a native walking goal to a separately
chosen, route-validated safe anchor; it never treats the defended post as safe
by default. Arrival at that anchor permits rallied; if the order names an
officer, that officer must still be live and nearby. A blocked route remains
routing: the current route helper also reports unloaded terrain and unavailable
world adapters, which cannot prove surrender. Native capture must establish
physical custody separately before a prisoner assignment. No numeric morale
meter, timer-decay, artificial immunity or abstract casualty refund substitutes
for these transitions. The guest chooses the officer and safe anchor; it cannot
publish native morale or motion directly.

Capture consumes a live routing attacker's public order revision, which native
morale progress does not change, only when another live, bound villager guard
is within three blocks. The native receipt and original resident order record
commit together: the original UUID and gear stay in place,
the attack stops, and the native assignment becomes prisoner/surrendered. An
unloaded, dead, converted or released guard/victim, or a distant guard, cannot
claim a prisoner.
The native marker denies fresh work, military orders and demobilization; it
does not name the political custodian. The settlement guest first CAS-fences
the victim out of its old military roster, persists the physical capture
operation id and guard handle, then replays that id after restart until the
native result is known. One final registry CAS records the captor settlement
on the original resident entry, without adding a second resident entry to
the captor's roster. Explicit native refusal restores the former role under
a CAS; unknown outcomes leave the intent fenced rather than re-enlist a
possibly captured resident. War legality and release/exchange remain the
separate treaty-policy boundary; custody does not authorize a later attack.

This is not a cross-journal atomic commit between order/ammunition persistence
and projectile spawn. The order receipt reports committed melee damage, not a
prospective bow hit; durable projectile outcomes remain a separate boundary.

Settlement `march` and `halt` name a bounded roster of hired aliases with
each member's current order revision. The guest re-reads owner/steward rights
and resolves those aliases to durable resident handles; it emits one native
group request, never one request per member or per movement tick. Native
admission fences every live member and commits the entire roster's order and
receipt together. That acceptance is atomic; walking to each assigned slot
is subsequent regional simulation and a blocked route reports its own member
without rolling back the other accepted movement goals. The owner places a
formation once per distinct anchor per order, then checks at most one route
per eligible member. Each accepted movement destination is journaled with the
member record so pending-admission replay restores distinct slots without
terrain re-query or stacking. A later subset order replans only its named
roster, while omitted members retain their previous order. Cancellation fences
and idles only the named members; it does not teleport lagging residents.

Ambulatory evacuation uses that same native Move admission for the original
wounded resident and a nearby hired medic. The guest checks a live wound and
four-block proximity, then requires both members to report `Applied` before
calling the order admitted. This is neither atomic arrival nor a coupled
escort: each regional entity walks its own route and may be interrupted after
admission. A partial route reports partial/blocked rather than claiming
transport. No patient copy, captive roster entry or second equipment ledger
is created; the separately journaled medical operation remains the only
healing/supply debit.

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
