# ADR 0013 - Core model v2

**Date:** 2026-10-08  
**Status:** Accepted (owner decisions Q1-Q10, 2026-10-08); current and sole core
model after [ADR 0014](0014-v1-core-retirement.md) Phase 5 (2026-10-15).
**Supersedes:** ADR 0004 (central `SimulationOwner`), ADR 0005 (regional owner
lanes), ADR 0006 (v1 module boundaries), and ADR 0011's v1 topology, threading
model and four durable streams. Historical receipts below retain their original
runtime and verification scope.

## Owner decisions

| # | Question | Decision |
|---|---|---|
| Q1 | Scale target | One machine, maximum out of it. Multi-machine is not a goal. |
| Q2 | Dominant load | Both: ~1000 players and hundreds of thousands of mobs at once. |
| Q3 | Spatial shape | Both crowds and spread. Parallelism is by data, not by space; space is used only for memory locality. |
| Q4 | Entity tick semantics | Snapshot + intents + deterministic merge. Vanilla-like, not vanilla-order exact. |
| Q5 | Block update chains | Redstone/fluids/pistons/neighbour updates run in vanilla order inside their zone of influence; independent zones run in parallel. |
| Q6 | Durability | One tick journal. A crash loses the unflushed tail, consistently: world, players and plugin data roll back to the same tick. No fsync on a reply path. |
| Q7 | Plugin mobs | Plugins compose mobs from native core building blocks (goals, sensors, parameters, work schedules) as data. Core executes them natively each tick. Plugin code runs only on rare decisions. Missing behaviour is added to the core library. |
| Q8 | Plugin timing | Plugins run outside the tick, read the last published snapshot, submit intents; intents apply at the next merge and may be refused. A plugin never slows the tick. |
| Q9 | Migration | New core beside the old one; port clean logic (protocol, worldgen, physics, data, world format). |
| Q10 | Cutover | When the new core carries ordinary survival (login, chunks, blocks, inventory, combat, mobs, crafting, saving) and beats the old core under load. Remaining mechanics and the plugin API are finished in the new core; the old core is then deleted. |

Earlier owner requirements stay binding: optimizations are generic (plugin
mob types scale like vanilla ones, no type-name branches), no activation-range
behaviour cuts (every mob keeps all its goals at any distance; only the AI
cadence varies, see Mob blueprints), remote physical tickets keep chosen areas
simulated without players, radical hardware-level optimizations are allowed
when measured and documented.

## Item and station custody (2026-10-23)

The native player retains independent gameplay RNG and enchanting-offer seed
fields. Only an accepted enchantment changes the offer seed; both fields are
included in reservation plans, WAL post-images and checkpoints. Vanilla `XpSeed`
is the offer seed; `SolarisGameplaySeed` carries native random-draw continuity.

Furnace used-recipe counts are authoritative inventory-adjacent state, not a UI
cache. Live state and journal images share an immutable count map; a completed
cook or XP withdrawal replaces that image. Chunk checkpoint projection and
hydration retain the same counts. Accepted container destruction emits banked
XP once before removing custody. Furnace recipe-book placement uses the ordinary
shared-container inventory transaction and reserved-item floors, never an edge
inventory or a separate ingredient store.

## Session admission and unverified identities (2026-10-23)

Edge admission reserves capacity across Login, Configuration, pending Join and
queued Leave. Its connection guard owns retirement before native submission.
Leave uses a credit reserved at admission on the existing bounded native FIFO,
never a second gameplay owner or a best-effort dropped lifecycle message.
Retired edges discard subsequent reliable output instead of reopening a stash.

When plugin login is required, native Join creates only a transient empty
presentation with a connection-scoped UUID. The claimed identity remains in
the connection context for the auth plugin; its saved player record stays in
native offline custody. Native unlock selects one authenticated claimant,
restores saved state and cursor/grid custody, and publishes the resulting
position, inventory, vitals and roster. Locked presentations never enter the
player WAL/checkpoint image and never overwrite an account on disconnect.
The authenticated session exclusion is checked across dimensions at the plugin
unlock boundary. Pending bodies cannot trigger redstone or dimension travel.
These transitions remain tick-owned; edge liveness deadlines do not mutate
native state directly.

## Mining and piston protection admission (2026-10-23)

Survival digging is native tick-owned, transient player state: Start selects
one target and block preimage; server ticks accumulate the authoritative
hardness/tool/effect progress. Finish is not an independent block edit.
Immediate Finish requires 70% progress. An earlier matching Finish occupies
one separate delayed-destruction slot and completes at 100%, without another
client packet; Start on another block does not erase that slot. Start credits
its first tick immediately. Abort, changed preimage, loss of reach, death or
dimension travel invalidate the delayed target. A committed break consumes
only matching position/dimension/preimage progress, not a newer target.
Build-hook preview may approve the pending break early; its single-use
authorization waits in that same slot, then passes ordinary progress and
native commit revalidation. Missing or negative hardness fails closed;
creative Start remains instant. Early finishes acknowledge and repair client
prediction while awaiting their authoritative completion.
Zero-vertical movement packets (including rotation/status-only packets) retain
grounded mining speed only when the collision-shape support probe confirms the
reported support; an unsupported ground claim still receives the airborne penalty.
The `solaris::mining` trace target exposes Start/Finish ticks, per-tick credit,
rate and authoritative grounded state for client timing diagnosis.

Pistons have no player actor. Their atomic move planner uses the existing
environmental protected-cell policy (also used by fire and explosions), not
zone membership or public-use permission. Protection covers base, head,
every moved source/destination and every destroyed cell before any edit or
drop is published; one protected cell refuses the whole push or pull.

## Optional Java distant terrain read boundary (2026-10-22)

`mc-core::terrain` owns bounded immutable detail-6 sampling: one 4×4-chunk
leaf plus a one-chunk halo (36 chunks maximum). It reads committed resident
images and retained unflushed edits before durable native coverage, and uses
the normal chunk source and missing-light bake only for genuinely absent coverage. Store errors remain
errors; sampling neither admits simulation residency nor writes a second
world store. The tick publishes shared immutable images; sampling and
compression run off-tick, without making the tick wait on a reader.
Like native `LodSource`, it borrows `ChunkSource` through a weak reference,
upgraded only for a sampling call. Retaining a stopped `Core` handle must not
keep the native database open or allow new samples after owner shutdown.

Opaque world identity and content publication stamps belong to this source,
not to a client, seed, filesystem mtime or encoder invocation. The UUID-v4
identity persists in `level.dat/Data/SolarisTerrainEpoch`; a fresh world gets
a fresh identity. Revision fences exclude partial tick publication and
invalidate derived results after authoritative edits, not idle ticks.
Content timestamps are strictly increasing and pinned per revision within
the bounded in-process stamp cache; reopening or eviction conservatively
restamps rather than claiming durable per-section modification times.

`mc-edge::dh` owns optional Distant Horizons protocol 16, dimension/session
epochs, cancellation, material privacy, resource-name presentation and the
format-1/Zstd DTO. It shares the native LOD worker semaphore but not its
heightmap protocol or seed/program export. Requests, bytes, cache and upload
budgets are server-enforced. A separate writer queue drains after ordinary
reliable and world packets; canceled or old-epoch fragments cannot commit a
response. Graphical compatibility remains a separate real Java/DH gate.

The independently authored protocol fixtures at
`crates/mc-edge/src/dh/protocol16-java-fixtures.json` were exercised in both
directions against the publisher-hash-pinned DH 3.3.3/26.1.2 jar's unchanged
Java message serializers. This includes the upstream server override when
chunk-only and surface-only plans conflict. Trackable test objects bypass
the constructor's live-world requirement, not the real encode/decode logic;
this is serializer evidence only, never a loader or graphical pass.
The production Rust format-1/Zstd envelope encoder was also exercised against
that jar's unchanged full-data DTO decoder and unit-test data source: 4096
asymmetric columns preserve orientation, heights, relative Y, both light
channels, generation/compression metadata, flags and timestamp. This does
not exercise Minecraft registry resolution or rendering. Local receipts are
`.analysis/DHCompat/oracle/result.json` and `dto-result.json`.
No DH or plugin source/library is linked, copied or vendored into the Solaris
server binary.

## Core stop diagnostics (2026-10-20)

Every tick-owner stop records its cause at error level before dropping the
broadcast sender: requested shutdown or fail-closed journal/plugin failure.
The server records SIGINT/SIGTERM before joining the owners, not only after
the final checkpoint, so a normal signal-driven stop cannot masquerade as an
unexplained broadcast failure. Journal/native channel loss and flush failures
also record their cause. Owner panics retain the standard panic hook and add
an error-level record containing the thread, payload and source location;
failed joins are not discarded. Error diagnostics include the source chain.
SIGKILL, process OOM kills and stack-overflow aborts cannot reliably execute
Rust logging and still require the kernel/service-manager evidence.

## Item farms and hopper lanes (2026-10-21)

Ground items merge after motion every twentieth tick using vanilla
`ItemEntity.mergeWithNeighbours` bounds: compatible item stacks/components,
`.5` horizontal and zero vertical box inflation, and the receiving stack's
actual component-aware maximum. The survivor keeps the maximum pickup delay
and minimum despawn age; a fully absorbed row is removed through the ordinary
same-tick entity tombstone path.

The native container owner retains an exact loaded-hopper lane rather than
filtering all block entities each tick. A due hopper that cannot transfer
sleeps until its own or an adjacent inventory changes; item motion wakes only
the bowls it can reach. Cooling remains an absolute eight-tick deadline, so
neither transfer cadence nor custody/journal ordering changes.

## Collision and explosion hot paths (2026-10-21)

The physics broad phase starts with the body footprint and advances a
deterministic three-axis DDA, adding only a leading cell slab when that face
crosses a grid boundary. It retains the existing candidate and scan caps.
The randomized collision regression compares normal and first-impact results
against the former full swept-volume scan; removing the DDA restores that
reference path.

Explosion exposure is memoized only for one tick and only by exact explosion
centre plus entity bounds. Priming TNT clears the cache because it immediately
removes a live-world occluder. Each explosion still runs in its original merge
order, calculates its own destruction rays, block effects and RNG stream, and
emits its own event; the cache never combines explosions. The release perf
fence is the ignored 1,000-TNT stack and fired-cannon scenarios in
`tnt_tests.rs`, measured only in a frozen serialized window.

## Native connection lifecycle (2026-10-19)

The edge reserves player admission atomically before Configuration: Joining,
Live and Closing consume a slot, with the existing operator bypass policy. Outbound
registration requires active Joining; closed or unknown IDs cannot recreate
pending join/reliable state. No lifetime tombstone set is needed.

`Core::join_session` reserves all four input credits before publishing the
negotiated view, locale and Join. Its single-owner session retains one credit for Leave
on that same FIFO. The edge lifecycle guard drops it on normal return, pre-Play
failure and task cancellation, and also releases admission/admin state and the
writer task. Cleanup never waits or retries a full ordinary-input queue. Full
initial admission and an already stopped core are distinct refusals.

Delivery overflow stops routing and pushes a cancellation event to the outer
connection owner, even when its socket writer is blocked. Only the final guard,
after queuing native Leave and clearing admin state, releases admission. Closing
delivery is not itself completion of the native session lifecycle.

Native input capacity has a minimum of four slots and includes these reserved
cleanup credits. Edge composition adds one credit per configured player slot to
the ordinary input budget. Gameplay/session removal remains native tick work;
there is no second player authority or persistence stream.

## G1-A event-driven allocator (2026-10-16)

Decision 008 replaces the per-tick population validator with a runtime dirty
worklist: resident row updates, per-resident-chunk activity/residency changes,
pushed POI chunk revisions and unload/reload events, changed claim owner sets,
and occupied-bed differences. Home, job and meeting memories subscribe through
the claim writer; boundary beds also subscribe to possible foot columns while
their facing is unavailable. A rotating 1/128 resident slice is the backstop.
Hydration indexes and queues every resident before the first allocation pass.

Missing claims alone enter a due queue. Due searches join a cancellable FIFO
admission order, so a budget-denied search keeps its retry deadline without
starving later residents. Due demand and settlement occupancy are incremental
projections, not population scans. The allocator refreshes the shared sensor/
resident cache once before the brain; actual allocator row changes update only
their cached brain profiles. Releases settle to a fixed point in the same tick.

Within each resident's pending search, transient cursors also rotate the
missing home/job/bell kind and its route candidates. Otherwise one unreachable
nearer home or workstation can exhaust all 4,096 path nodes and restart before
any reachable job is considered on every retry. These cursors reuse the existing
search-needs entry, survive retry-deadline changes and disappear with that entry;
they are neither durable claim authority nor another queue or path budget.
Every claim still requires a successful native route. Natural-grove workers
start their existing bounded radius scan at the certified entrance, and resource
workers haul a completed stack rather than waiting for all eight carry slots.
Route clearance for walking bodies treats water covering only the feet cell as
passable, like vanilla ground navigation; head-deep water and lava still refuse,
and spawn/placement clearance stays strict. Lumberjack searches skip columns in
chunks that are not actively simulated, since a worker routed there would stop
at the border with its target held.

Nonresident swap-removals repair only changed resident slots, preserving
stable brain indices and admission ranks. Hydration and genuine roster
add/remove still use the existing full rebuild/sort (Main's explicit scope
boundary); this is not a new scheduler representation. The 50k mixed-churn
fixture removes one prefix mob each tick, asserts zero roster rebuilds, and
reports roster maintenance cost separately. A mixed-removal oracle compares
each fast-path tick with a forced full rebuild.

No durable schema, movement cadence, collision resolution, or route proof
changes. Full-validator event-oracle, activity-churn, pushed unload/reload,
same-tick release and abrupt-reopen fences accompany the existing focused
villager/settlement/journal gates. Measured acceptance and exact command/binary
identities live in `.analysis/codex-logs/g1a/result.yaml`; these fixture receipts
do not replace the next real-scene/1–2-core native-client tick gate.

## Villager door route and collision proofs (2026-10-19)

Villager paths may traverse wooden doors, so a pure wooden-door `open` flip
does not invalidate their route. The world retains a per-chunk villager route
revision and pending structural-write set alongside its existing wire revision;
admission, unload/reload, door replacement/orientation changes and other block
edits still invalidate the route. Generic NPC routes keep the physical/wire
fence because they do not share the villager door interaction.

Physical motion/standing caches always observe real wire revisions and pending
block writes, including door flips. Preserving a route must never reuse an old
closed/open door collision shape. These fences are transient tick-owner caches;
block updates still use the existing client projection, WAL and checkpoint.
Forty-tick retries preserve progressing routes, but a non-progressing villager
replans from its current position. Door closure waits for other villagers whose
bodies or next two path cells occupy the doorway.

## G1/G2 measured row-custody costs (2026-10-16)

The frozen G2 journal compared all 11,557 scene rows each tick despite the
touched-set implementation. Opt-in, test-only `SOLARIS_TRACE_ROW_MUT=1`
attribution over the warmed 120-tick fixture identified portal timer custody:
`portals::travel::finish` took `row_mut` on every row, including all 10,000
villagers and inactive dropped items. The persistent despawn scan also reset
already-zero idle timers for all villagers every spawn cycle.

Portal scans now use read-only access when there is no contact and both timer
fields are zero. Despawn idle resets and generic entity cold-field writeback
take mutable custody only when a value actually differs; effect-free rows
also check for real effect work before borrowing mutably. These are no-op
proofs, not reduced simulation cadence: real cooldown/elapsed changes, portal
travel, timer decrements, melee resets and chunk-crossing marks retain their
existing owners and WAL coverage.

The warmed 50k moving-brain profile attributed 20.70% self samples to
`memmove`, 19.70% to allocator validation and 14.41% to claim lookup; the
full-decision cap was applied (256 admitted decisions per G2 tick).
Allocator validation now borrows world/registry/dimension separately and
updates each villager in place, rather than taking/restoring its large state
on every pass. Stable claim-index updates are omitted; geometry checks and
same-tick invalidation/releases remain unchanged. No new durable state,
admission exemption or approximate equality was introduced.

The correctness fences are failing-first journal touched-set regressions,
allocator collision/destruction coverage, and the existing villager/portal/
settlement tests. Controlled `-p` frozen/current timings, final remaining
cost attribution, exact commands and fingerprints live in
`.analysis/codex-logs/perf-g1g2/result.yaml`; the raw mutation attribution is
`touch-attribution.yaml` alongside it. These receipts do not establish an
exclusive server-load performance gate. If owner mutation rules change,
retain conservative `row_mut` custody for genuinely changing state; remove
the corresponding no-op proof rather than bypassing journal equality.

## Native military — Decision 009 (owner authorization, 2026-10-20)

The owner explicitly requested native barracks recruits, superseding the prior
S5 deferral below. Idle adult native residents fill certified barracks beds
(two per building level); they keep the original entity identity and become
Soldiers, projected with the vanilla armorer profession. Native equipment slots
hold their actual weapons and armor, and canonical fenced inventory commits
transfer equipment from settlement storage and return it on dismissal. Full or
unavailable storage refuses dismissal without destroying equipment.
The vanilla villager crossed-arms item layer displays the main-hand weapon;
equipped armor contributes to native defense but is not rendered on villagers.
The owner accepted that vanilla renderer limitation for this prototype; native
residents are not replaced by a different visual carrier.

Persisted resident military state holds the barracks identity, order and attack
deadline. Orders are Follow(player UUID), Guard(block position), Patrol within
the settlement radius, and Return to barracks. Local hostile sensing, existing
collision-aware villager navigation and the native melee/damage pipeline govern
combat; this adds no abstract battle or unloaded combat catch-up. Recruitment
is automatic only for loaded, unowned autonomous settlements. Owned settlements
are staffed explicitly through the Army dialog or `/settlement army recruit`,
so dismissal remains in effect until the owner recruits again. Mutations retain
settlement ownership checks.
An unowned autonomous settlement cannot be commanded until claimed.
Soldier attacks are produced in the entity phase as ordinary `EntityId`-targeted
damage intents, merged and committed in that same tick. The post-commit
villager brain only navigates: it must not leave dense-slot damage pending
across a tick boundary where natural despawn can compact entity slots.

## Native economy — Decision 006, S4 (2026-10-17)

Production, commerce, payroll and caravans are native settlement policy, not
plugin timers or plugin KV inventories. Policy lives in each settlement's typed
post-image and the native `settlements` checkpoint domain; actual goods and
emeralds remain in native containers, player inventories and the exact living
villager's carry/equipment columns. Inventory changes use the existing atomic
native inventory apply, including component identity, capacity, physical
double-container alias checks and revision fences. Policy and physical stock
are captured by the same tick WAL/checkpoint boundary.

Production uses the admitted native recipe catalog, real certified work points,
the original worker body's route, and simulation-clock deadlines. Tool wear,
ingredients, authored remainders and outputs commit together; missing inputs,
space or a reachable station pause without partial consumption. Caravan work
likewise moves the original bound body through the native collision-aware
brain. Only actual pickup and unload create their receipts; elapsed calls,
unloaded stock or a policy label cannot manufacture delivery.

Wages accrue and pay automatically in the native settlement tick at the
contract's existing due-period cadence. An empty treasury preserves earned debt.
Stopping employment stops future earnings, not payment of its outstanding debt:
the settlement remains active until the debt is paid, and only a fully paid,
stopped contract may be dismissed. A matching genuine caravan delivery
automatically pays its escrow recipient exactly once through the same atomic
inventory owner. Unreadable/unloaded escrow is unknown, not evidence of loss.
No independent payment queue or retry state machine is introduced.

Completed quote, employment and mandate IDs retire behind monotonic full-`u64`
creation fences. A fence rejects a new replay, never an older still-live
contract; completing that older contract cannot move the fence backward.
There are no authored lifetime-operation, population or active-job count caps.
Existing storage-byte, native inventory, clock and per-tick work/path budgets
remain. A fair ephemeral cursor bounds due work; derived body-goal and active
settlement indexes are rebuilt after recovery, not persisted as policy.
Unchanged/idle economy passes do not touch settlement journal images.

Customization continues through typed `query-settlement` observations and
revision-fenced `request-settlement-action` operations, with the existing durable
operation receipt. The native `/settlement` economy commands use that same action
owner and require the settlement owner or operator for mutation. The retired
settlements package is not a fallback or a second authority. Its six native
item identities and 56 recipes stay in core with vanilla item presentation;
generic NPC APIs and their independent consumers remain intact. Military/S5
is not part of this cutover and requires its separate committee decision.

Removing the retired item-model outputs changes the exact native startup recipe
identity. This is an unreleased clean cutover: old disposable native copies are
re-imported/recreated after integration. The admission identity fence stays
strict; no compatibility shim or silent catalog rewrite is introduced.

Durability coverage includes real native checkpoint and WAL reopens of automatic
wages, stopped debt after treasury refill and actual body-walked paid delivery.
The existing settlement SIGKILL harness also pays a native wage, retires its
contract, fsyncs the production WAL while the checkpoint is still at creation,
then kills the process before checkpoint. Fresh core recovery must retain both
original physical wallets and the retirement fence; ticking and replaying the
retired contract must not debit again.

## Autonomous generated villages and construction (2026-10-20)

The owner explicitly requested a Millenaire-style native prototype. Generated
vanilla villages are adopted from authoritative structure starts at their loaded
meeting-piece bell; identity is dimension plus start chunk. Generated entity
markers prove resident membership. Arbitrary nearby player bells cannot create
another village; absent/dead inhabitants do not erase a generated site. The native
settlement row owns an optional authenticated player UUID (`None` means
autonomous), stable worldgen provenance, bootstrap storage and the construction
project. Native settlement row/WAL post-images and villager membership columns
remain the only persistence authorities. Claim is `/settlement claim` or the
main dialog button, admitted only for an in-world player inside the radius and
only while ownerless; all other player management mutations require a claim
first, even for operators. Existing owned settlement permissions are unchanged.

Adoption deduplicates by the row's stored worldgen source before allocating.
Both founded towns and generated villages use the existing monotonic settlement
sequence; there is no hash-derived identity namespace or second ID allocator.

The owner explicitly permits one empty founding chest as adoption infrastructure
(like the meeting bell), without consuming a chest item. Its contents are never
fabricated. Existing generated geometry is certified only if it passes the real
building checks; natural level-one lumber/mining zones bootstrap real material
production without conjuring a workstation or stock. Subsequent construction
consumes real storage/carry items, including recipe-derived materials.

The same bounded planner handles autonomous and owned settlements. Repository
blueprints place actual blocks and consume actual settlement-container items;
finished geometry goes through the existing building certification authority.
Development and military work run only in loaded chunks. There is no off-screen
construction, recruit or battle simulation and no new abstract catch-up domain.
The typed settlement snapshot exposes `owner: option<string>` through WIT as
well as Rust: absence is not represented by an invented owner identity.

Owner build/upgrade requests require an idle settlement and a whole bill funded
by existing real storage ingredients, including supported crafting chains.
Busy or unfunded requests are refused immediately, never persisted as pending
work and never allowed to block autonomous planning. Accepted work uses the same
canonical project, structure ledger, Builder carry and checkpoint/WAL recovery.
The unshipped prototype `construction_queue` field is removed without a migration:
unknown fields still fail closed, and QA regenerates its prototype world.

## Native districts and stages — Decision 006, S3 (2026-10-17)

Each native settlement may bind a bounded district plan: requested branch and
level plus owner-scoped references to already committed native structures or
registered native worksites. The plan is part of the touched-only settlement
post-image and the native `settlements` checkpoint domain. It contains no
population roster, authored capacity, stock counter or persisted `Ready` flag.
The generic structure/worksite ledger remains the existing building authority,
not a settlement KV mirror.

`request-settlement-action` plans a district under the settlement revision fence;
`query-settlement` observes its current qualification. Qualification uses living
native residents, their distinct real native bed claims with complete bed pairs
and clearance, physically checked building interaction points, and actual food
in declared local containers (shared/double-container cells counted once).
Unavailable or destroyed geometry, missing stock or missing housing removes the
observed stage immediately; a saved plan never grants readiness by itself.

With housing and food for every resident, the observed stages are hamlet,
village (24 residents), and developed (64 residents). A developed settlement may
qualify as estate, fortress or town at levels one/two only with its real working
institutions. Choosing a non-empty branch fixes that branch; a later request may
change its level/buildings but cannot silently switch it. These are observations
of existing people and economics: no request spawns, replaces, teleports, ages
or fills a population range. Fortress qualification adds no military fields or
behavior (Decision 009).

The replaced plugin development domain, KV state machine, command/event hooks
and development-only overview page are deleted. The SDK, WIT and host expose
the same typed plan and observation contract; the actual SDK guest exercises
plan/query, stale revision refusal and idempotent replay through core dispatch.
Production/commerce/payroll/caravan are covered by the native S4 cutover above.

## Native households and demography — Decision 006, S2 (2026-10-17)

Household policy records now belong to each native settlement record. A
household identity combines its founding settlement ID and a monotonic local
sequence; relocation preserves that identity. The records contain origin,
revision, social home anchor and policy references, but no resident roster.
`VillagerState::household` is the sole membership authority alongside
`VillagerState::settlement`. Living household counts are derived, maintained
on row changes and rebuilt after hydration, rather than scanned by courtship.

Living household size includes members in every dimension, not only the
dimension that owns the policy record. It is not a population cap. Local
membership projections and the owner's global size projection are derived
caches rebuilt together after
recovery; neither is a second persisted roster. Actual membership transitions
enqueue affected household IDs. The dimension tail reconciles that workset
after all worlds have been restored, preserving foreign members during local
unbinds and propagating relocation before retiring globally empty households.
An idle tick does not scan the population or emit household journal images.

Native courtship keeps its start tick in the villager column, and native birth
provenance travels with the child. A birth joins its parents' household (or
forms their first household) in the same native entity/settlement WAL batch
that creates the child and its individual bed claim. Cross-settlement pairs
and different assigned households remain incompatible. There is no authored
maximum household count or member count: population-based admission is the
native allocator's actual free bed, complete head/foot pair and baby clearance,
not the retired plugin's 256-household/16-member limits. Existing native record
byte fences remain storage safety limits. No timer, plugin birth observer,
adoption request or plugin KV mirror is involved.

The remaining generic NPC birth query observes retained native bodies, not an
independent immortal birth-history ledger. Its persisted per-child cursor key
packs birth tick and same-tick ordinal, so body removals and wire-ID
reassignment do not shift pagination. Removing a body also removes that
body's birth observation; household membership is never reconstructed from
this query.

Relocation changes the original people and household's settlement together.
The sole POI allocator releases home/job/meeting claims and stale routes,
then its ordinary budgeted reachable-POI admission reacquires destination
claims. It does not teleport people, create substitutes, load destination
chunks or promise a claim for an unreachable destination. Household records
use the existing touched-only settlement post-images and per-record native
checkpoint rows; entity columns share the same durable tick boundary.

`query-settlement` exposes typed household observations with members derived
from living entity UUIDs; `request-settlement-action` forms and relocates
households under source/destination settlement revision fences. The native
`/settlement relocate <source> <household-id> <destination>` root requires
ownership of both settlements or operator authority. The replaced plugin household
domain/runtime are deleted. Legacy birth/courtship KV keys explicitly refuse
recovery rather than silently discarding provenance. S3/S4 are not implied by
this cutover; Decision 009 excludes military fields and S5 behavior.

## Native settlements — Decision 006, S1 (2026-10-16)

Settlements are a native tick-owner mechanic in `mc-core/src/settlements`,
not a first-party plugin authority. S1 covers registry records, housing/POI
allocation and binding the exact existing villager identity. Households and
demography (S2), districts (S3), production (S4), and military (S5) remain
outside this slice; removal of the old package does not imply those features
have native replacements.

Each dimension owns records keyed by globally monotonic `SettlementId`.
Names may change; deletion never releases an identity. The allocation high
water is shared between dimension ticks and retained durably even when the
last record is removed. A settlement record has no resident roster:
`VillagerState::settlement` is the sole membership authority. Present living
resident counts are derived from those columns and rebuilt on hydration.
Binding neither spawns a replacement entity nor takes plugin NPC custody.

The native housing allocator owns `home`, `job_site`, and `meeting_point`.
It runs in the commit tail immediately before `villager::brain::tick`, has
its own telemetry phase, and shares global-tick decision and path-node
admission with the brain through the single `tick_budget::Budget`, carried
between dimensions and reset only at the global tick root. Its existing
256-decision/4096-node limits are provisional, not performance acceptance.
The root partitions that same pool by dimension using previous-tick demand;
within each demanding dimension allocator and brain each retain a guaranteed
quarter (at least one unit each), and the rest is first-come within that
dimension. Completed passes return unused guarantees to the common remainder;
when both finish, the unused dimension remainder returns too. Cheap known
backlogs weight the next tick without iterating denied work. Admission uses
`try_charge(dimension, consumer, kind, amount)`; path probes borrow this pool
and preserve their existing blocked/retry outcomes instead of minting budgets.
The brain owner pays `Decision` once when admitting the full brain; movement
and courtship searches within that admission charge only their A* `PathNode`
expansions, never a second decision that could starve an already-paid brain.
It uses only resident chunk POIs, invalidates changed claims and retries
refused routes without loading chunks. Body execution remains the
villager owner's per-tick responsibility; allocator denial is not a body
execution exemption.

Only touched settlement post-images or tombstones enter the shared tick WAL,
under a `JournalEntry::Dimension` envelope. Checkpoints store separate
settlement rows, each limited to 16 MiB and carrying its dimension with the
record, plus the shared identity high water; there is no aggregate registry
blob. The native checkpoint global-key validator admits only canonical
`settlement/<u64>` record keys (nonzero, no leading zeros) and the validated
`settlement_sequence` row in addition to its existing closed schema; all
other unknown keys still fail closed. Pre-S1 empty native checkpoints start
with high water zero; settlement-bearing checkpoints require that row.
Membership columns are captured in the same
entity WAL/checkpoint transaction. A dimension-tail deletion fence clears
foreign-dimension member columns before the shared WAL is captured.
Recovery refuses missing settlement references and legacy settlement-plugin
state explicitly rather than erasing it. Legacy development worlds must be
re-imported; no plugin/native hybrid migration path is provided.

`/settlement` is a native command root. The existing startup item/recipe
configuration lives in the core repository and enters the existing startup
contribution pipeline; no production or military runtime is moved into S1.
The six native-owned item identities use their existing vanilla carriers and
do not require a Loader package, including with an empty plugins directory.
Loader item/artifact/permission validation applies only to plugin-owned
items; native ownership is resolved from the admitted core catalog, not
from an item's namespace.
Plugin customization exposes `query-settlement` and
`request-settlement-action` together, exercised by a real consumer; no
`settlement-changed` notification is introduced. The old settlements package
is removed from deployment and its S1 live-gate coverage moves to native
regression tests. S4 also retires its sibling source and remaining package assets.
Certified buildings persist with the settlement row: `building_sequence: u64`
and `buildings: BTreeMap<BuildingId, Building>` ride the touched settlement
post-image and per-record checkpoint row under `serde(default)`, so legacy
rows decode to an empty map. Building IDs are `(settlement.0 << 32) | local`
with 32-bit halves; row/WAL validation is shape-only (ID linkage, level
1..=3, bounded bounds inside the anchor radius, sorted in-bounds work points
with counts level..=64 for Storage / exactly level for jobs). The allocator's certified
work-point list is the only job-slot authority; certification never runs at
row validation time. Upgrade rechecks the reserved bounds; explicit
same-kind re-certification may expand those bounds while retaining ID/level.
There is no legacy job authority for empty building maps: an uncertified
bound job claim is released, while its profession and trade offers survive
until a certified slot can be acquired. Missing certification alone must not
erase the resident's profession/offers on later jobless validations.
Other existing NPC API consumers retain their current interfaces until the
last consumer is migrated; S1 does not remove unrelated NPC customization.

S1 replacement coverage is named explicitly below; these native tests replace
only the former package's registry/housing/binding scope, not S2–S5 gameplay:

| Former package S1 contract | Native regression |
| --- | --- |
| Registry identity and rename/delete | `settlement_ids_never_reuse_deleted_or_replayed_ids`, `refused_registry_mutations_do_not_spend_identity_or_touch_records` |
| Existing resident binding, no second NPC | `binding_preserves_exact_original_entity_and_has_one_membership_column` |
| Native command permissions and original identity | `settlement_commands_keep_native_identity_and_enforce_owner_or_operator`, `settlement_create_refuses_without_authenticated_actor_and_creates_nothing` |
| Housing authority and settlement bounds | `allocator_assigns_home_job_and_meeting_point_as_sole_authority`, `allocator_scopes_acquisition_to_bound_settlement_radius` |
| Crash durability and monotonic identity | `sigkill_child_reopen_keeps_identical_records_and_high_water` |
| Dimension-local records and retained membership | `two_dimension_equal_name_checkpoint_and_tail_roundtrip`, `dimension_tail_deletion_clears_foreign_member_without_reusing_identity` |
| Real SDK customization read and action | `settlement_consumer_query_then_bind_then_rename_through_real_dispatch`, `settlement_consumer_stale_id_refuses_after_remove_and_recreate` |
| Global owner/dimension admission fairness | `overworld_allocator_backlog_cannot_starve_nether_brains`, `allocator_backlog_cannot_zero_brain_guarantees_in_the_same_dimension` |
| Late dimension activation after a saturated history | `newly_populated_nether_receives_brain_admission_after_overworld_only_history` |
| Paid brain routing under a full decision batch | `admitted_brain_path_does_not_pay_a_second_decision` |
| Certified building persistence with settlement row | `nonempty_building_roundtrips_touched_wal_and_checkpoint_row`, `legacy_row_without_buildings_decodes_to_empty_map` |

The remaining complete-deployment catalog gate still checks the six retained
packages plus the native physical catalog. Town build protection remains
`real_six_town_claim_protects_tcp_builds`; it is not settlement housing coverage.

## Model

### Tick

One authoritative world state per tick `t`, published as an immutable snapshot.
Tick `t -> t+1`:

1. **Inputs.** Drain player packets (decoded off-tick by I/O threads), plugin
   intents, and remote-ticket changes accepted since the last tick.
   `tick.rs` owns input ordering, approval/fence gates and dispatch; input
   behavior lives in `tick/input/{session,movement,interact,combat,inventory,social}.rs`.
   `tick/damage.rs` owns committed damage application, hit admission, shields
   and knockback; the root merge orchestrator calls it before pickup and spawn
   effects.
2. **Entity phase (parallel by data).** Every simulated entity reads snapshot `t`
   and writes only its own next state plus intents (attack, pick up, break, place,
   breed, interact). Work is split into fixed chunks of entities over all cores;
   no entity reads another's next state.
3. **Merge (deterministic).** Each intent declares its complete footprint: the
   targets it changes (block, entity, item entity) and the sources it consumes
   (inventory slots, item counts, durability, hunger). Intents are applied one
   at a time in a fixed order (tick-stable actor id, then intent sequence)
   against the accumulated merge state: an intent whose footprint no longer
   validates is rejected whole, debit and credit and every dependent effect
   together. Two claims on one item give it to one claimant; two placements
   paying with the same last item succeed once. Merge applies serially today;
   applying disjoint footprints in parallel is future work and must equal the
   serial order (decision 011).
4. **Block phase.** Merge writes, random ticks and scheduled ticks settle on
   the native tick owner. `blocks.rs` composes stair after-images, plant/leaf
   neighbor updates, attached-block support cascades and falling starts after
   fluid/redstone writes. The former instantaneous gravity-column simulation
   is removed: `falling.rs` removes each source once and spawns a retained
   `falling_block` body, then advances gravity/drag over successive ticks.
   Collision-only displacement uses the shared fingerprinted shape kernel
   with a 0.98-block body and no step-up: empty attachments are not floors,
   partial-shape blocked landings drop rather than overwrite, and retained
   horizontal motion receives the same drag as vertical motion.
   Sand/gravel/anvil variants, dragon eggs, suspicious blocks, concrete powder
   and downward pointed-dripstone chains follow the OLD start/landing policy.
   Landings place once, blocked/expired bodies evaluate unattended block loot,
   suspicious blocks and stalactites shatter, and swept water contact hardens
   powder even at a dry landing. Missing chunks pause motion without consuming
   the body. Native `BlockState`/`Time` plus the water-contact latch accompany
   full position/velocity WAL images and checkpoints.
   Placement follows axis/facing/half rules, slab doubling, wall variants,
   cocoa attachment, stair corners, clicked replaceable vegetation and the
   second cell of doors, beds and double plants. Occupancy and neighbor reads
   are fenced in the same intent as the item debit. Exact sturdy attachment
   faces reject unsupported placement without debit. Complementary two-cell
   identities break whole and roll their primary loot once; breaking a
   waterlogged block preserves its source. Wooden doors, trapdoors, fence gates
   and levers toggle on hand use; a dig start breaks immediately in creative
   and on blocks without hardness.
   State-gated `set_count` loot functions retain the original item/count when
   their condition is false: single slabs drop one, double slabs two. Waterlogging
   does not alter that count; state predicates also apply to explosion drops.
   Physics classifies fingerprint-matched empty vanilla collision shapes
   as passable (including vegetation and signs), retaining fluid material
   separately; authoritative clipped player moves send a position correction.
   The edge applies a configurable per-Play-packet-id token bucket before
   decode, while `tick/input/movement.rs` owns the authoritative configurable
   airtime verdict: unsupported non-descending survival movement is corrected
   to its last safe position and then disconnected after the configured age.
   Flight permissions, active effects, fluid/climbable contact, vehicles,
   knockback and slime contact remain native state checks, never edge mirrors.
   Sprint input flags/commands and accepted movement charge hunger, and pickup uses the
   vanilla inflated player box rather than a sphere around the feet.
   A use of a client-predicted placement in the same batch waits until the
   following tick's accepted world. Placed signs open a connection-owned
   editor; updates revalidate position, side, block and current interaction
   range. Later writes revoke the editor, including same-state replacement.
   Plain-text NBT is broadcast, journaled and checkpointed in the chunk;
   support/flow removals tombstone older text in the same block tick.
   Hanging signs use their native editor and block-entity type 8 (ordinary
   signs use 7). Wall hanging signs attach at either bar end, with same-axis
   chains; ceiling signs require the CENTER underside support patch and retain
   vanilla attached/cardinal/free-rotation placement. Waterlogging preserves
   text, while accepted same-state replacement clears older text. Both ceiling
   and wall forms retain each wood's canonical hanging-sign loot and sequence.
   Respawn requests are accepted only while the player's entity is dead.
   Water and lava then run their due scheduled ticks (`fluids.rs`, vanilla
   `FlowingFluid`: water every 5 ticks, lava every 30, down first, sideways
   7 / 3 blocks; source conversion honors the water/lava conversion rules over
   a nonempty vanilla collision shape. Lava/water contacts harden to
   obsidian/cobblestone/stone, and water hardens adjacent concrete powder.
   Flow washes empty-collision blocks away with their loot but never replaces
   a waterloggable block. Waterlogged sources participate without destroying
   the enclosing block. The exact fluid identifier, absolute deadline,
   priority and insertion order persist in WAL/native checkpoint images;
   only offline Anvil export encodes `fluid_ticks` with remaining delays.
   Unloaded work retains its identity and deadline, and empty coverage prevents
   resurrection of consumed stored work. Block/fluid absent-chunk captures
   compose without
   simulation admission or overwriting the other queue. Stable lava with OLD
   ignition fuel requeues; isolated stable fluid does not perpetually poll.
   OLD's simplified all-open-side spread is retained, not a new slope search.
   Buckets use bounded voxel source raycasts or clicked `UseItemOn` intents:
   vegetation is transparent to pickup, existing sources refuse pouring,
   waterloggable blocks fill/drain in place, and full/partial cauldron rules
   share one inventory debit/replacement with the accepted block effect.
   Loaded furnace-family block entities cook every tick even with their menu
   closed; unloaded storage pauses. Input replacement resets cooking, result
   pickup/shift-click consumes the used-recipe XP record exactly once, and the
   taking player receives the award. Open viewers receive changed burn/cook
   data properties and a revised full slot image when fuel/output changes.
   Lighting changes preserve facing and enter the block change set.
   Hopper item collection intersects the item body with both the block above
   and the hollow bowl down to its 11/16-block floor. Items that fall into the
   bowl during the eight-tick transfer cooldown remain collectable afterward;
   collection does not wait for the player's thrown-item pickup delay.
   Container custody and consumed-item tombstones retain their existing atomic
   tick WAL and native-checkpoint ownership.
   Ender-chest custody is the player's retained `EnderItems` NBT field, never
   block-entity storage. A 27-slot menu decodes only on interaction; planned
   player/cursor and personal-container postimages commit together. Recovery
   rejects invalid personal slots rather than exposing an empty replacement.
   Book edits/signatures replace one writable inventory stack through native
   admission, retaining author, title, literal pages and other components.
   Frames own their displayed stack and rotation in entity metadata (9/10);
   checkpoints project vanilla `Item`, `Facing` and `ItemRotation`. Placement,
   interaction and support-write events drive updates; there is no idle frame
   scan. The edge alone translates hanging render positions to spawn BlockPos.
   Hopper transfer cooldowns persist as absolute tick deadlines, so the seven
   intervening cooling ticks emit no timer-only opaque WAL images. Native
   checkpoint and portable Anvil export derive a bounded `TransferCooldown`
   of 0..=8; export strips the native clock anchor. Cold unload/restart may
   expire that short cooldown, but never changes inventory custody.
5. **Commit.** The tick's changes form one change set: appended to the tick
   journal, applied to the authoritative state, published as snapshot `t+1`.
   Durable chunk dirtiness/checkpoint ACK fences are separate from wire
   content revisions. Private block-entity inventory and timer images remain
   durable without invalidating encoded client chunks; block/light changes
   and public block-entity payload changes advance the wire revision.
   Commit-tail native owners (villager harvesting, extraction, replanting and
   doors included) may write after the main block-phase fold. Their remaining
   block overlay is folded before immutable chunk publication, so same-tick
   block deltas and the chunks used for deferred delivery share one afterimage.
6. **Publish.** Network fanout (tracking, chunk/entity/inventory packets) is
   computed from snapshot `t+1` and the change set in parallel per viewer on
   the edge's own worker pool (never the core's), and handed to I/O threads.
   The edge pump is the only chunk sender (the join burst carries none): at
   most 16 chunk packets per viewer per publish: unseen chunks first, then
   stale full repairs, nearest-first within each group. Repair debt remains
   attached to held chunks until credit permits the newest full image. The pump
   yields to the runtime after every publish so connection writers it woke
   are never starved behind a catching-up pump. Every packet is framed once
   at plan time; encoded chunk frames are cached by the pump, keyed by the
   chunk revisions of their 3x3 light neighbourhood, and shared by every
   viewer and outbound queue by refcount: viewers plan in parallel against
   the read-only cache, then misses are encoded once per chunk and inserted
   by the pump alone (no lock). Anti-xray engine mode 1 (`[anti_xray]`,
   `crates/mc-edge/src/anti_xray.rs`) runs at that encode: an ore whose six
   neighbours are all opaque is encoded as the surrounding rock in section
   copies (the snapshot chunk is never touched), once per chunk for every
   viewer, and a hiding dimension's cache key adds the four side neighbours'
   revisions because enclosure is judged across chunk borders. Reveal rides
   the block-update path: once per publish the tick index lists the hideable
   ores a block change just un-enclosed (keyed by the ore's own chunk), and
   each viewer holding that chunk gets their real `BlockUpdate`. World packets are planned within each
   connection's credit, the free slots of its world queue read before the
   publish (only the pump sends there, so nothing planned is ever dropped):
   forgets, block updates, chunks, removals, spawns (nearest first), then
   appearance and motion in a scan that resumes where the credit last ran
   out. What does not fit stays unsent in the viewer cache: motion is
   planned by comparing the snapshot with what the viewer was last sent,
   and a deferred block update is sent later from the newest snapshot.
   G6 follows committee decision 007's updated G2 order: query only the viewer
   chunk square from the immutable spatial index; bucket block/entity/visual
   changes once per dimension before parallel fanout; retain the cached
   spiral/window and one delivered-entity map with a plan stamp. Own-entity
   and event-subject resolution use indexed identity lookups, not a scan of
   the population. Tracking remains vanilla: an inclusive horizontal range
   `min(type.tracking_range, view_distance) * 16` and the type's id-phased
   `update_interval`, with appearance/removal independent of movement cadence.
   Walking viewers reevaluate the range against their fresh anchor. There is
   no K cap, priority ranking, hysteresis or distance-cadence extension without
   a measured dense 1000-client budget failure and a separate decision.
   Measured (100 viewers in a 100k-cow herd, ~10.3k entities each, every
   entity moving; receipt `.analysis/codex-logs/v2-stage1/
   fanout-credit-bench.md`): planning every move cost 148 ms plan + ~110 ms
   routing per publish for 1.03M packets of which 95% overflowed the
   512-slot queues; within credit it is 17 ms plan + 1.5 ms routing for 51k
   packets, none dropped. Each connection reads and writes independently: a slow
   socket write never delays reading its inputs.

### Data layout

- Entity state is columnar (structure of arrays) keyed by dense slots in
  spawn order. Position, velocity, horizontal combat impulse, rotation, ground
  and `Body` are contiguous columns. `Body` alone owns identity/kind, alive,
  movement, dimensions, air/fire, pickup/attack timers and hurt time; those
  fields do not exist in `ColdRow` or motion hints. The full connection-bearing
  `EntityKind` stays together in `Body` (72 bytes, rather than a mirrored
  64-byte kind-class layout); contact inputs occupy its first 64 bytes.
  Spawn, swap-remove and dimension transfer move every column together.
  `body_mut` and mixed `parts_mut` retain the same journal/motion/brain/wire
  custody as `row_mut`; serial motion writeback retains its narrower air-only
  durable mark and transient-timer policy. Journal image schemas do not change.
  Contact classification and pickup selection borrow body/position slices.
  Quiet contact survivors still read cold appearance serially, because burning
  metadata is not a second authority in `Body`. Worker snapshots carry common
  combat/breeding inputs; rare effect/state owners keep cold access.
- Snapshots are immutable and versioned. The resident chunk list is shared
  with earlier snapshots until a chunk becomes resident, changes revision or is
  evicted; unchanged chunks share their `Arc` either way, and an edit copies
  only the chunk it changes. G6 wire views use persistent identity and chunk
  indices, updating only mutation-owned candidate IDs and sharing all other
  tree nodes. The `EntityStore` mutation boundary feeds journal/brain/wire
  consumers; wire column setters notify that same boundary, while journal
  durability retains its own cold/chunk-crossing policy. There is no periodic
  full-population change detector or second gameplay authority. Published
  revisions carry their base revision and changed IDs (including tombstones).
  A reader may keep a snapshot as long as it wants; it only keeps
  that version alive. The core never waits for a reader. Consumers that fall
  behind skip world state (chunks, entities, blocks) to the latest snapshot.
  Per-player results (`PlayerChange`: join, menus, health, acknowledgements)
  are never skipped: each connection receives them in order through a bounded
  queue, and a connection that overflows it is disconnected. The core appends
  a tick's per-player results before broadcasting that tick, so a consumer
  woken by tick `t` finds them. After a skip the edge reconciles each
  viewer's held world view with the latest snapshot.
- Immutable startup data (registries, collision shapes, recipes, mob blueprints)
  is `&'static`.
- **Revisions over sharing and locks.** Threads exchange owned values and
  immutable published versions; they do not share mutable state. Where a
  reader needs to know whether derived data is still valid, the producer
  stamps a revision instead of the reader holding a lock or a pointer:
  every snapshot chunk carries a world-unique `revision` that changes with
  its content (load, block edit, block-entity injection) and never repeats;
  every chunk load request carries a ticket, and the tick accepts a result
  only while its ticket is current, so superseded or unwanted loads are
  dropped on arrival rather than cancelled in place. A new mutable shared
  structure or lock needs an explicit measured reason recorded here.
- No `Mutex`/`RwLock` on the tick path. Phases hand data between cores by
  ownership of disjoint slices; the only synchronization is the phase barrier.

### Threads

- **Tick workers:** the core's long-lived rayon pool, one worker per usable
  core, running the parallel parts of the phases above with work stealing
  inside a phase. No threads are spawned per tick, and no other subsystem
  (edge fanout included) schedules work on this pool.
- **I/O threads:** sockets, framing, encryption, packet decode/encode. They own
  connection state; they hand decoded inputs to the tick and receive encoded
  outbound buffers. They never touch world state. A connection's writer joins
  the frames already queued (reliable first, then world, up to 64 KiB) into
  one socket write: one syscall per frame held 1.06 of 6 cores at 100 clients
  (tokio threads), batched 0.24.
- **Journal thread:** appends tick change sets (WAL batches and their fsyncs)
  and gates each checkpoint's WAL coverage. The tick freezes a checkpoint
  (dirty chunks by `Arc`, player/entity/block-entity
  records by value) and hands it over through a bounded FIFO queue
  (`JOURNAL_WORK_QUEUE` = 128: queued batches behind the one the journal thread
  is draining, 6.4 s of ticks). The depth exists only to absorb journal-thread
  stalls: an fsync waiting behind the native writer's multi-hundred-MB redb
  commit measured 0.1-1.6 s and parked a two-deep queue's tick for up to
  267 ms (mixed-survival-long r11, tick 11432). While the journal keeps up the
  queue is empty. The tick blocks on `send` only while the queue is full —
  queue admission, not the disk write — so a journal slower than the depth
  slows the tick but never drops or reorders accepted batches. The count cap
  alone does not bound memory (a TNT tick's `Append` is 12-28 MB, so 128
  queued ticks could pin gigabytes behind a stalled disk): the tick also
  reserves each `Append`'s estimated bytes from a shared `JournalBudget` of
  `JOURNAL_QUEUE_BYTES` = 256 MB in-flight payload before `send`, blocking
  while it would be exceeded (a batch larger than the whole budget is admitted
  alone, so that cap has this one exception). The estimate is a conservative
  overestimate computed on the tick thread with no serialization: entry slots
  and spare `Vec` capacity plus the owned heap of each entry, walked by
  capacity/length (item stacks and components, strings, NBT, plugin and
  JSON values, scoreboard teams and members, villager offers/gossips/inventory,
  vehicle slots, allay, projectile, raid, settlement and statistics payloads);
  only fixed-size records get a flat allowance. Ordered maps/sets are priced
  per element at 3x the inline key+value bytes plus a node allowance (nodes
  stay at least ~5/11 full, so up to ~2.2x slot slack). `Identifier` spare
  capacity is not exposed, so it is priced by text length plus an allowance:
  the figure is a conservative estimate, not an allocator-exact resident
  bound. The reserved number
  rides in the `Append` and the journal thread releases exactly it once
  `append` returned (the WAL writer accepted the batch; its fsync may still be
  pending). The budget bounds queued `Append` payload only: `Checkpoint` jobs
  are bounded separately by the single-in-flight checkpoint rule and
  `SaveGenerated` by `generated_busy` (one outstanding), not by the byte
  budget. The budget closes when the journal thread returns, failing a parked
  reserver like a failed `send`. No extra thread, no drops.
  `SyncSender::send` fails only on journal-thread death (receiver
  disconnected), which keeps the existing fail-closed halt. A crash loses at
  most the batches not yet written: normally the batch being appended, and at
  most the 128 queued ticks (or 256 MB of them), only while the journal thread
  is stalled.
- **Native writer thread:** the single redb writer, owned by the journal
  thread's scope (`mc-core-native`). It runs the one-transaction checkpoint
  commit and generated-chunk saves, in FIFO order, so a multi-second commit
  (measured 5.2-5.4 s at 12.5k entities under load: the former inline commit
  parked tick N+3's `Append` for that long) occupies no thread that drains WAL
  appends. The journal thread forwards a checkpoint only after `gate` returned
  (WAL durable through the capture tick) and with a plugin mirror frozen at that
  tick; it joins the writer on shutdown, so the final checkpoint is durable
  before `Core::shutdown` returns. In normal running at most one checkpoint
  and one generated save are outstanding, so its channel is unbounded but two
  deep; shutdown may start the final checkpoint with one in flight and queues
  its remaining generated saves, all drained FIFO before the join. The writer
  fails closed: a panic in a job reports failure on its `done`/`reply`, stops
  the writer and halts the journal thread like an append failure. An append
  failure drops the journal's receiver immediately, so the tick's next `send`
  fails without waiting for an in-flight commit.
  **Hot backup** (console `backup`, `Core::backup`) is a third FIFO job on this
  thread: after a forced checkpoint, the tick forwards the request through the
  same work queue, the journal thread fsyncs the WAL directory and records the
  highest durable tick (`through`), and the writer — idle between commits, so
  `state.redb` is exactly the last committed checkpoint — copies it plus the
  contiguous batches `(checkpoint, through]` (only this thread prunes, so none
  vanish) with SHA-256s, in 8 MiB sync steps so WAL fsyncs never queue behind a
  dirty-page burst. Ticks and appends continue; the next checkpoint and
  generated saves wait for the copy. A copy error replies to the caller and
  never halts the world. The edge writes the offline-compatible manifest last
  and publishes by renaming a `.partial` directory.
- **Plugin host:** one production Wasmtime component worker outside the tick,
  using `mc-plugin-host` and the existing bounded `mc-script` boundary. Guest
  callbacks never borrow native world state or execute on tick workers. The
  owner drains admitted commands without blocking and applies them against the
  current authoritative state; session and operation identities are revalidated
  at the native decision.
  Build admission freezes a native read-only plan and asks the component host
  outside the tick under one queue-inclusive 100 ms `mc-script` deadline ending
  at host answer receipt. Later native scheduling does not expire a timely answer;
  roster/generation, live source and single-use checks remain enforced. The
  native owner consumes a single-use Keep approval at the actual intent merge,
  after later inputs and the entity phase, only after rechecking the exact plan,
  full selected held stack, player/session/dimension, native protection/custody
  and handler KV versions. Click orientation/sneaking and ray origin retain the
  approved geometry while reach, occupancy and support remain live checks.
  Bone-meal randomness retains the preview tick rather than rerolling. Preview and
  consumption reuse the canonical dimension-global ownership transfer; plugin
  KV, protection and custody never acquire a second per-dimension authority.
  Denied/expired/stale plans publish reliable per-viewer block repairs, sequence
  acknowledgements and inventory contents, not world changes or block WAL.
  Physical structure advances retain at most 64 pending decisions and poll them
  without blocking; retries coalesce by exact operation identity.
  Read-only previews see applied inputs/merge. Each production preview carries
  a sequence fence through the same bounded native input FIFO; the native owner
  answers only after that fence has applied, closing cross-channel races with
  earlier hotbar/movement inputs. Fences share a startup-allocated frontier,
  not a per-request allocation. While idle, an explicit wake can retire only
  metadata fences and answer ready previews without adding a simulation tick;
  the first real input remains buffered for the normal tick. The ordinary
  20 TPS cadence and hook-free wait remain unchanged. Idle wakes never renew
  the host admission deadline; a timely answer ends that clock budget rather
  than extending it through another tick wait. Refusal repairs never replay
  click-time rotation over a later input's look.
- **Chunk generation/loading:** pure workers, owned input to owned output,
  joined at the tick's input step (`mc-core` `chunk_loader`: a fixed pool of
  half the usable cores, each worker with its own job channel; results return
  over one channel the tick owns). One worker per core let exploring players
  hold generation at 3.2 of 6 quota cores and the tick at ~110 ms; half the
  cores measured TPS 8.4 -> 13.6 (receipt `.analysis/codex-logs/v2-stage1/
  worldgen-cost.md`). Each tick requests the missing wanted chunks nearest
  player or ticket centre first, each worker's job queue bounded (one queued
  plus the one in production; requests that do not fit wait for a later
  tick); finished chunks wait outside residency until their still-current
  ticket and current want are rechecked. Tick admission shares a nearest-first
  whole-chunk batch with clean recent returns (bounded cache, 2048), then
  settles lighting before returning the resident/publishable batch. Startup
  `ensure_blocking` still joins the complete wanted set.
  Player view radii are connection-local: Configuration and Play
  `ClientInformation` negotiate the client/server minimum (floor two, capped by
  the server). Edge uses that value for cache radius, streaming and forgetting;
  ordered `SetViewDistance` inputs give the native connection registry the same
  transient value before Join and on updates. Residency unions each player's
  negotiated view with simulation distance plus the existing one-chunk margin,
  then retains the existing spawn/ticket/dirty-pin requirements. Dimension
  travel preserves the connection's radius; disconnect removes it.
  Generated chunks are saved in bounded batches of at least 64 by the native
  writer thread (the single native DB writer, ordered with checkpoints), at most one
  batch queued or running; while it runs the world holds up to 1024 more and
  skips saving beyond that (those are generated again when next wanted).
  A generated save fills only genuinely absent immutable bases, atomically
  checking imported coverage and gameplay records. It never overwrites imported
  air, checkpointed edits or stored corruption. Native reads compose the
  immutable imported/generated base with gameplay overlays; decode failures
  refuse admission rather than substituting generated terrain. The tick never waits
  for generation, and a column that is not yet resident blocks movement
  like any other unloaded column.

Worker counts are derived once from the machine, never from operator
percentages.

### Dependency-driven execution and capacity (decisions 010, 011)

Sources: `.analysis/committee/decision-010-1m-entities.md` and
`.analysis/committee/decision-011-capacity-model.md` (adopted text; per-member
answers under `q010/`, `q011/` are background). Single tick/intent-merge
authority and "no regions" are unchanged.

**Execution principle.** Dependency-driven execution, not distance tiers. Three
independent schedules: AI decision (due or invalidated), physical execution
(needed, or exactly quiescent) and replication (a client needs an update).
Distance changes only AI decision cadence and interest scope; it never
suppresses a needed physical reaction (motion/collision every tick, larger-dt
physics rejected: the mob AI cadence paragraph under *Mob blueprints*). Exact
quiescence comes first: a sleeping subsystem has complete wake dependencies and
deterministic deadlines, and a sleeping body stays in the collision and query
indexes. Workloads are reported by count: stored, resident, physically moving,
AI-due, network-changed, entity-viewer pairs.

**Order.**

- **Step A (in progress as native-100 rounds):** per-tick cost proportional to
  active entities, not resident ones. Iterate only what can act through the
  existing `villager_chunks`/`sensor_chunks` indexes (no third index without a
  measured replacement reason) or small per-feature lists; replace per-tick coin
  flips with absolute due ticks (deterministic RNG keyed by uuid and tick, due
  queue keyed `(due_tick, phase, entity_id, seq)`, worker wake requests admitted
  by deterministic merge); keep skip-relevant state in hot columns so rejecting
  a row never touches `ColdRow`; physics only for moving, unsupported or
  dirty-footprint bodies; AI goals always kept, only cadence varies.
  Native feature scans use `EntityStore` mob-type membership and sparse
  rider/leash memberships, maintained at spawn, removal/compaction, dimension
  transfer and their mutation setters. Queries retain dense-slot order; live,
  activity and feature-specific checks still run in the original phase.
  Component-gated feature scans (bees, allay, falling blocks,
  zombie-villager cure, copper-golem presence) filter exact type lanes,
  retaining each original component/alive predicate and dense-slot order,
  and mutate only after the full list exists. Random-tick sampling
  draws coordinates and classifies the block per chunk on the pool, then runs
  `tick_cell` serially over the ordered hits. Redstone/sculk/shrieker
  hydration skips residency bookkeeping while `ChunkWorld::active_revision`
  and the resident count are unchanged (every resident insert/remove bumps
  the revision); in-place chunk content hydration still runs each pass.
  `EntityViews::publish` builds a touched page by streaming the old cells and
  replaced cells into one right-sized vector; unchanged pages stay shared.
  Entity-phase mechanism (r14): a decision-tick mob (cow/pig/sheep/chicken, and
  player-only chasers while no survival target exists) whose complete
  `entity_motion::step` with zero drive is bit-identical to its input is
  admitted as resting, exactly like a between-decision rest, so it builds no
  wide row or worker result and takes no serial motion-result apply; motion
  clocks still advance. Hot flags (`MOTION_WANDER_REST`,
  `MOTION_TARGETLESS_WANDER_REST`, `MOTION_ARMORED`, `MOTION_QUIET_TIMERS`)
  handle admission/profile/timer prechecks without a cold-row or appearance
  lookup. The exact proof still reads body/effect state and runs a physics
  step per qualifying decision-tick row. Wire marks are a dense per-slot flag,
  not a hash set; a cheaper exact rest certificate remains unestablished.
  Zombie and zombie-villager decision rest additionally requires an empty
  resident target table, not just absence of live player targets. Admission
  reuses the sensor's current resident table, built before row preparation.
  Any candidate anywhere in either table conservatively disables decision
  rest; the normal 1/2/4-tick sensor cadence therefore catches same-chunk range
  crossings, spawn, teleport and dimension arrivals without a new wake index.
  Occluded candidates also keep sensing live (zombie chase has no sight-ray
  filter), so breaking a remote wall cannot strand a sleeper. Existing
  footprint, hurt/fire, fluid/air and exact motion-proof gates are unchanged;
  golem/zoglin multi-class targeting remains excluded. Removing this extension
  means excluding zombie types from `MOTION_TARGETLESS_WANDER_REST` again.
- **Step B (in progress as fanout rounds):** edge work proportional to changes.
  Plan from snapshot `changed` plus chunk-to-viewer subscriptions; encode each
  move once and share bytes across viewers with matching baselines; no
  per-candidate string compares. Distance-based update-rate reduction stays
  deferred (see Publish).
- **Tracking range cap (sim−1 chunk)** is a separate future tracking policy,
  not adopted. It shrinks the workload, so it is never counted as a speedup;
  native-100 rounds keep the original range and report tracked entities per
  viewer.
- **Step C (not adopted):** strategic/dormant simulation is a gameplay model and
  needs its own approved ADR revision after A/B are measured. Current catch-up
  is only *Abstract resident catch-up*. Any extension is approved per actor or
  process type with: named identities and resources conserved; rule-based
  despawn (being unobserved is not a reason); partition property
  `advance(a+b) == advance(a); advance(b)`; counter-based RNG keyed by stable
  ids; persisted cursors and pending operations in the same tick WAL; and
  transitions prepare, advance, validate, atomically switch, publish, never
  duplicating bodies or inventories. C2 (settlement kingdoms: unobserved
  work/trade/fights as outcome events, bodies materialized at schedule
  positions) is a separate ballot after native-100 reaches ≥10 TPS. It
  additionally requires persistent process records (operation id, mode
  generation, progress anchor, next event, dependencies, RNG cursor), real
  resource consumption and cargo on certified routes, and committed battle
  events with named casualties. Current S4 economy and debt catch-up are not
  that model.

Nothing here claims 1M simultaneously active or visible mobs fits the machine;
1M is reachable only if most of the population is quiescent or unobserved. The
temporary ≥10 TPS native-100 target is reported separately from 20 TPS
acceptance.

**Capacity model.** One shared-resource model, not per-tier ceilings: tick, edge,
generation and I/O work share the same cores, and the core latency
`T_core ≥ max(W/P_core, longest dependency span, memory bytes / attainable
bandwidth)` is only a necessary lower bound, not a p99 ≤ 50 ms proof. The serial
ceiling is not established. Hard caps are checked simultaneously: RSS, egress
bytes, client readiness, WAL lag and queue depth (the journal queue is bounded,
see Threads), view lag and chunk-time budget. Players and mobs trade off on one
budget. Population/player ceilings are published only from measured sweeps per
machine class (quota/affinity, physical cores/SMT, single-thread speed); a point
passes only if tick tails, readiness, bounded view/WAL lag and memory pass
together. Calibration (shipped priors, startup topology/quota detection, online
per-work-class estimates with bounded hysteresis) may only narrow budgets within
hardware maxima, reuses `tick_budget::Budget`, the chunk-admission EWMA and the
brain machine tier (no third mechanism), and never learns capacity from missing
gameplay or unready clients.

**Overload order.** (1) Exact quiescence always. (2) Reduce redundant
replication/prefetch and background work (chunk generation). (3) Stretch
explicitly elastic AI cadence within bounds; melee/hit range always decides
every tick. (4) Refuse new commitments (spawns, players via the existing
`server_full` path) before charging resources and before the existing overflow
paths (input drop, reliable-queue disconnect). Mandatory work completes. (5) Only
if mandatory work still exceeds capacity, slow the logical clock; that is the
last resort and never counts as a 20 TPS pass.

**Serial-work reduction and many cores.** Order: Step A/B; remove repeated
copies in serial prep/apply; parallel pure construction from frozen inputs
(dirty view pages, journal post-images) followed by one ordered seal/append, with
a serial reference proving equivalence; independent block zones (Q5) later. No
block-phase parallel zones exist today. Journal capture of tick N never runs
concurrently with simulation of N+1 (capture mutates baselines); fanout of N
already overlaps N+1. The WAL keeps fsync-before-publish, contiguous coverage
and checkpoint order (*Durability*). Many-core scaling is a deterministic task
graph with disjoint data-page ownership and one global tick seal after accidental
serial work is measurably removed, never independently ticking regions.

**Rejected.**

- Per-chunk aggregates replacing mob identities: lose the UUIDs and retained
  state that native custody, projectile owners and bound villagers need; they
  may exist only as derived indexes.
- Folia-style region threads: regions are already rejected as the parallel unit
  (Q3) and would split the single tick WAL.
- Full archetype-ECS migration: targeted hot-path columns give the win without a
  rewrite.
- Reduced-rate/larger-dt physics by distance: a 4x step is not four steps; use
  exact quiescence instead.

### Residency, activation and chunk debt (G5)

Chunk residency and gameplay activation are separate authority sets. The only
loader/eviction demand is the union of each player's configured client view
square plus a one-chunk margin, the active set, explicit tickets and dirty
checkpoint pins. Simulation distance never limits a client's view square.
Explicit ticket ranges also activate their chunks (forced-chunk semantics);
dirty pins retain data without independently permitting simulation.

Players enter the active set at the server simulation radius. Previously active
chunks remain active within a one-chunk exit band around any player; shrinking
the simulation radius immediately clips that band to the new radius plus one.
Activation also requires a resident, legible chunk. Gameplay queues, random
ticks, mob brains and clocks, machines and inhabited-time accumulation consult
activation; storage hydration, collision legibility and teleport readiness still
consult residency. Changing activation does not revise public chunk contents,
wire revisions or the edge's held-chunk state.

Committee amendments 001-d/e bind slice 0: the debt clock advances only while
active, never merely because an inactive chunk remains resident or wall/game
time passes while it is evicted. Activation, including resident-inactive to
active, is the catch-up boundary. Catch-up consumes prior authority before live
ticks and never overlaps their interval. Slice 0 forces abstract elapsed/effects
to zero: it introduces only the clock, absolute persistence images and
checkpoint/ACK custody fence, not item, furnace, crop or villager abstract work.
Active/inactive clock boundaries are absolute WAL intervals, like inhabited
time: empty later batches advance an active `last_sim` anchor but never an
inactive one or the authoritative `remaining` amount. Clean active anchors and
every nonzero-debt image remain in the checkpoint manifest. Nonzero debt is
authority, not a value reconstructed from an absent chunk's last load time;
owing an unchanged inactive amount does not itself dirty or rewrite a chunk.
Changed debt projections remain pinned until a matching checkpoint ACK; stale
ACKs cannot release newer clock changes or replay a consumed slice after restart.
Advancing a clean active anchor does not project native chunk extras again:
the atomic manifest already captures that clock. Activation, deactivation,
debt consumption and newer WAL authority still project dirty chunk images.
Recovery closes an active debt anchor and dirties that transition before a
later manifest may omit its now-inactive zero; otherwise its older native
`last_sim` could incorrectly recreate debt on the next restart.
Native debt extras are an indivisible pair of nonnegative NBT longs. Both absent
means a new chunk; a partial pair, wrong type, negative value or duplicate key
refuses admission/startup with a storage error, even when a newer WAL image
exists. Projecting an authority value beyond the native signed-long range also
fails before mutation or commit; neither decoding nor checkpointing normalizes,
erases or saturates debt.

Replay derives deterministic entity lifetime/countdown clocks from those same
chunk activity intervals, not global elapsed ticks or a new entity flag. While
folding a streamed WAL tail it retains transient per-chunk cumulative active
time through every activation/exit; compact entity anchors and the final
projection query this prefix, so multiple inactive gaps in one tail stay frozen.
Missing chunk-clock authority is inactive. Ordinary intra-chunk motion remains
checkpoint-only; crossing a chunk boundary emits the existing full entity image
and adopts its position on replay, preserving the clock's chunk attribution.
The transient comparison cache is seeded from the recovered position before the
first live tick, so a first-tick crossing cannot lose its attribution; advancing
that anchor with the emitted image prevents duplicate crossing images.
Villager refresh timestamps use the actual active intervals, never a synthetic
global timestamp formed from their summed duration.

The `native-v2-radius-full-view` graphical gate requires a live observed client
render distance of 8, reads one distinct cell in every chunk of the 17×17 view
square, and checks that none was forgotten on the wire. Cell reads are paced
with live ticks within the native MCP request budget; no request-limit bypass
or substituted server-side block read counts as client evidence.

### Villagers

Villagers and zombie villagers carry native `VillagerState`, including biome,
profession, offers, gossip, food, age, POIs, restock counters and conversion.
The villager brain handles daily activities, bed/job/bell discovery, panic,
food pickup/sharing, breeding and village iron-golem summoning. Zombie
infection and weakness/golden-apple curing retain identity and offers; changed
entity types are removed and respawned for viewers with the same wire ID.
Merchant transactions use the authoritative inventory owner and durable offer
uses; native inventory and offer items share the canonical component codec.
Unknown and rich NBT components survive typed item projection and checkpoints
without overwriting subsequent typed edits.

Workstation use and profession acquisition share a three-block reach, expressed
as squared distance so navigation's adjacent-cell endpoint cannot strand an
unemployed claimant. During work, villagers approach distant claimed job sites
and take short strolls nearby after arrival rather than standing still until the
next schedule change. Idle strolls and worker motion publish through ordinary
entity snapshots; profession changes publish `VillagerData` appearance deltas.
Relevant route-chunk revisions invalidate cached walking routes without postponing
their 40-tick target-selection deadline. Chunk admission and unrelated edits must not
keep an arrived worker's next stroll perpetually in the future; tracked motion
continues through per-viewer baselines while new chunks stream.
Focused controls cover native checkpoint plus WAL recovery, exclusive retained
claims, and Anvil `VillagerData` / `Brain.memories.minecraft:job_site` export.

An open merchant menu suppresses voluntary villager movement, sleep and work
restocking while retaining gravity, knockback and POI ownership; closing the
menu or disconnecting releases that live ownership. Transactions enforce
atomic costs, offer uses, restock and level progression. The shared trade
catalog retains old-core limitations: runtime-roll and second-cost trade rows
omitted by that catalog are not newly generated. Imported component-cost
predicates remain durable, but unsupported offers are omitted from the wire
menu rather than blocking supported offers or changing predicate meaning.
Displayed trade indices map back to the retained offer index before selection
or execution. Merchant input filling includes offhand stock and returns it
through the same authoritative inventory owner.

Offer uses belong to the villager, not its open menu: closing and reopening
without work preserves them. At a workstation during work hours, closing can
legitimately permit the first restock after even one trade. Economy admission
retains at most two restocks per day, requires strictly more than 2,400 game
ticks before the second, and resets that budget on the next day; focused
regressions distinguish this from offer regeneration on reopen.

Merchant `OpenScreen` titles preserve custom names as literal components;
otherwise villagers use their native profession translation key and wandering
traders retain their own entity-name key. Only the display name travels in the
packet: the 26.1.2 client composes `merchant.title` with `merchant.level.N`, so
the server does not append or duplicate the level label. Focused regressions
decode the opened fletcher and custom-named villager packets.

Focused regression coverage includes trade generation/execution, merchant-busy
brain gating, displayed-to-stored offer selection, all-filtered offer custody,
offhand payment conservation, exact-byte edge menus and canonical item NBT.
The integrated native suites exercise these paths; they do not certify
real-client villager parity or vanilla behavior beyond the covered contracts.

Villager journal images omit navigation caches, record only durable state
changes and recover together with checkpoint NBT. Generated-world entity
markers have permanent journal/checkpoint claims, so unload, death and restart
do not duplicate their original villagers.

Deterministic villager age/cure countdowns and continuously refreshed work,
sleep and panic memories use their committed tick anchors rather than an image
per tick. The comparison mirror and recovery apply the same projection; work
start/stop, restock, gossip transfer/decay, conversion completion and other events
still retain absolute post-images. Native `brain_clock` records loaded/NoAI
admission boundaries so paused resident brains do not accrue age during replay.
Missing legacy anchors default to stopped; the first live journal image admits
the clock explicitly. Dead brains, including curing zombies, remain stopped.

Resident full-brain admission spends the remaining **global tick decision
allowance** from `tick_budget`, shared with settlement allocation and every
dimension. The global root resets it once per tick; dimension dispatch moves,
never clones or locally replenishes, that authority. Admission rotates by
ascending entity ID within each priority band, never dense slot order.
The canonical budget owns dimension/consumer shares from prior full demand,
guaranteed minimums and a common remainder. The brain announces every ready
bit before selecting, uses its tuple's allowance for the near/outer split, and
charges each actual admission atomically; a refusal leaves its ready bit intact.
Every brain exit releases its unused share. No native scheduler reimplements
that cross-owner policy or charges a second decision for an admitted brain's A*.
The current canonical capacity is **256, provisional**: the G1.5 perf-lane
individual-decision p95 and moving-population gates remain red, so the earlier
2 ms / selected-decision-p99 derivation is not a claim about the current design.
Capacity is not an operator worker percentage, a runtime wall-clock cutoff, or
a bound on shared cache/physics cost.

The resident owner extends the ordinary mob's Chebyshev player-distance rings:
within 2/4/6/8 chunks the full-brain periods are 1/2/4/16 ticks, then 32 ticks
beyond (also when there are no players). Downgrade holds the prior ring for one
additional chunk; promotion occurs at its inner boundary. An open merchant
menu, recent damage, panic, courtship pursuit, a committed path or a potential
hostile-pursuit chunk forces period 1 and immediately releases an old sparse
deadline. These protections exempt cadence, not the hard per-tick cap.
Residents within two player chunks reserve `min(ceil(near / 20), 3N / 4)`
admissions; the outer bands retain at least `N / 4` under sustained population
overload. Unused capacity flows to the other queue. A retained due index promotes
outer work by its last actual service tick, before the 32-tick heartbeat expires
(64 on a one/two-core owner), never ahead of the near reservation.
New trade, hurt, claim and nearby committed/pending block-edit events use a
separate first-priority wake queue. Near interaction admissions count against
the near reservation; all wakes still obey the same hard cap.
Absent sustained interaction overload, the 20-tick near target applies when
`ceil(near / 20) <= 3N / 4`; more near bodies intentionally degrade cadence,
not tick capacity. The pre-G1.5 **N=256** 100-player, 25-quarter admission
regression measured near/outer maximum gaps of **20/74 ticks at 10k**
(3,200 near; shares 160/96) and **84/538 ticks at 50k**
(16,000 near; shares 192/64). Far-only gaps were **40/196 ticks** respectively;
these were measured queue delays, not a claim that an eligibility period alone
guaranteed the service target.
Trade, hurt, bed/workstation claims and an isolated pending door edit all wake
in the same invocation even at 50k. A burst larger than N cannot bypass capacity.
Denied deadlines are not pushed back. Existing POI/navigation deadlines,
schedule changes, growth and gossip-decay boundaries also wake full admission
rather than waiting for an unrelated cadence phase.

The correctness fence is the existing single resident owner: the generic
entity think gate still excludes these villagers. EntityStore maintains
chunk-to-resident and chunk-to-other-body indexes across spawn, movement,
swap-remove and dimension transfer; the brain visits only resident chunks
admitted by the current world boundary (`has_chunk` until active/resident radii
are separated). Food, gossip, courtship/birth and golem helpers use local chunk
candidates and admitted actors, never population-wide entity-store searches.
The stable ID/slot roster, POI claim owners and compact admission metadata are
retained between ticks. One EntityStore mutation hook feeds transient scheduler
invalidations; spawn/removal/transfer repair the roster, chunk crossing refreshes
the moved resident, and load changes refresh only the affected resident chunks.
Duplicate retained claims preserve the lowest-ID owner until it releases its
claim. Unloaded owners retain custody; new claims update both maps and
snapshots, and newborn reservations reconcile against live rows on refresh.
Clock-only lifetime ages and unchanged lifetime/conversion timers bypass
`row_mut` rather than invalidating every resident's brain cache.
Due deadlines are retained in an ID/version min-heap; ready bands are compact
ID-ranked bitsets with rotating cursors, not rebuilt per-resident lists.
Only changed profiles, due entries and changed player/sensor/terrain chunks
reclassify decisions. A separate execution bitset visits admitted actors and
residents whose clocks or committed motion can actually change. These replace
the two population-wide admission/execution eligibility passes; genuine clock
or physical work still scales with the bodies executing it. Schedule context
is computed once for adult, baby and nitwit classes, with class-transition
wakes phased by ID over 200 ticks (600 on one/two cores).
Player chunk crossings recompute loaded resident-chunk distances and reclassify
only changed near/ring classes. Bounded gossip and golem windows rotate across
admitted actors instead of always restarting at the highest-priority prefix.
Newly arriving hostile coverage raises an event wake, not just period-one cadence.
A skipped resident uses the existing `advance_clocks` projection through a
clock-only store accessor, not `row_mut`/a gameplay dirty mark; schedule and other
event transitions wait for full admission, including safe bed exit. No skipped
resident runs sensors or path search. Existing paths retain collision-resolved
movement and gravity/knockback through hot motion columns, but no skipped
decision relocates a body to its bed or any LOD anchor. Existing sleep continues;
bed entry/exit and appearance boundaries are handled by admitted brains.
Reached path steps are consumed through a transient, unjournaled store accessor;
skipped motion neither dirties a cold row nor oscillates at the first waypoint.
Pending-invalid paths stop being followed until full admission can replace them.
Committed and skipped motion resolve each next standing point once and retain
transient sampled block IDs/standing points only under the existing route proof.
New route capture clears this cache; the same revision, pending-edit and unload
checks invalidate it. Body-size and sampled-domain fences retain live sampling
outside the proof. Ordinary collision resolution, gravity and impulses still run
per tick. Cache clones are empty, so journal images do not copy its geometry;
skipped cache maintenance uses an unjournaled accessor rather than `row_mut`.
Short idle/play/work strolls use at most six straight cells with the same
step/drop, midpoint/headroom and unknown-block collision rules as A*, but no
A* search. They run inside an already-paid full decision and consume no
`PathNode` allowance because they expand no A* nodes; node-budget exhaustion
does not defer this bounded stroll. A blocked stroll retains its retry deadline.
Both stroll direction axes vary per retry, avoiding a permanent blocked-axis
bias while the tick counter still fits in its low bits.
Purposeful routes retain per-route chunk revisions, including geometry/support
halos; unrelated world edits do not invalidate them. Per-search standing results
reuse scratch memory and are discarded between searches, so geometry/world or
body-size changes cannot leak a stale clearance verdict.
NoAI, resident-inactive and unloaded residents remain stopped; active-set
revision changes invalidate cached admission profiles. Restart rebuilds only
transient admission/cadence state. Clock projection semantics and the journal format are
unchanged: committee 002-a dropped the obsolete claim that ordinary clocks
emitted full images; 002-b requires clock-only writes to remain outside dirty
gameplay mutation tracking.
POI geometry validation is cached per resident against exact memory, dimension
and head/foot chunk revisions. Pending edits invalidate immediately, while
claim custody, profession/trade locks and foreign dimensions remain live checks.
Player-occupied beds use a retained set; food/pickup points use numeric item IDs
rebuilt on registry replacement; threat decisions inspect only nearby sensor
chunks. Work keeps first offer initialization and live used-offer checks, but
does not retry an unchanged failed initialization on every full decision.
Zombie villagers remain threats through an incremental resident-hostile set,
including in-place infection/cure, without another population scan.

Receipts live in `.analysis/codex-logs/brain-budget/`: `before.json` preserves the
historical unchanged control; `g15-prototype.json` and `g15-standing-paired.json`
record the observed red gates and immutable-binary before/after controls.
`after.json` records the current functional coverage and explicitly unaccepted,
unmeasured stored-geometry performance gate; it does not claim a final speedup.
Only same-session `-p` perf-lane samples are compared, interleaved after/before
twice with uptime and scope recorded; older exclusive receipts are not an
environment-identical comparison. Full-brain scaling is not
a promise to bound total tick time: resident clocks, committed-path physics,
publication still scale with residents; journal comparisons follow G2's touched
rows rather than scanning the population. Cached claims and admission snapshots
are not persistent authority. S1's allocator owns the shared POI/route/node
scratch and all claim validation/acquisition. G1 supplies fair full-brain
admission and route-proven transient motion geometry, not another pathfinder.
`allocator::capture_route` clears motion geometry immediately after its chunk
proof; unchanged allocator validation and refused/empty acquisition leave
villager rows unmarked, preserving G2's custody boundary.
Removal path: replace this fixed admission capacity only after a fresh profile
supports a better bounded scheduler/regional owner; remove these transient
cadence/admission fields and keep the same schedule/clock, claim, merchant,
combat and no-teleport fences. Do not introduce cell-anchor poses or operator
thread percentages.

Home, job and meeting memories are qualified by the owning world's dimension.
Validation discards foreign-namespace memories before consulting local blocks
or claims, and invalidates navigation cached from those memories. Unloaded
same-world POIs remain retained; an absent chunk is not proof of destruction.

Villager scaling remains an explicit performance limitation outside the resident
decision owner: defense targeting still scans the population for each eligible
player. That path is not bounded at the ADR's thousand-player/hundreds-of-
thousands-of-mobs scale. Its authorized follow-up is retained chunk-local
defense candidates, preserving retaliation timing and deterministic order.

Wave-3 raids use occupied villager home/meeting POIs to convert Bad Omen into
600-tick Raid Omen, then retain difficulty-scaled 3/5/7 waves plus the
high-omen bonus wave in `raids.rs`. Raider village-assault goals reuse native
collision, pillager crossbows, witch potions, evoker fangs/vex and simplified
illusioner blindness/invisibility; victory grants Hero discounts and periodic
villager gifts, while resident death loses the raid. The offline converter's
26.1.2 `dimensions/minecraft/overworld/data/minecraft/raids.dat` codec fields and
raider `RaidId`/`Wave` are javap-confirmed; namespaced membership/omen custody
extensions and native checkpoint/WAL post-images resume once across
checkpoint/restart.
Native counters carry a namespaced world-tick capture boundary: WAL records only
phase/custody transitions, checkpoint retains current counters, and restart
rebases elapsed time while dropping ordinarily absent/dead member entities.
Hero gift searches run only on the 6,000-tick cadence, select each hero's
five-block candidates, and deduplicate villagers shared by multiple heroes.
The edge diffs reliable red/notched-ten raid boss-bar snapshots. This does not
claim structure generation, exact spawn-position search or full raider AI parity.

### Mob blueprints

A mob type, vanilla or plugin, is a blueprint: components, goal set with
priorities, sensors, movement/physics class, work schedules, loot, and
parameters. Vanilla mobs are blueprints too. The entity phase executes
blueprints natively; there is no per-type code path that a plugin type cannot
use. Plugin code is invoked only through events and decisions (for example
"assign work", "choose target set") outside the tick.
Shared behaviours are blueprint capabilities and data-defined types are admitted
under bounded, fail-closed validation; see [ADR 0016](0016-mob-capabilities.md).

Vanilla baseline registration (`species.rs`) composes every supported living
contract's exact registry dimensions and mob category with `MobBehaviorTable`
movement/combat families. Ground, flying, aquatic, amphibious and stationary
species use the same native controls rather than humanoid fallback physics.
Missing scalar attribute rows use an explicit supplemental gameplay parameter
table; these tunables are not protocol or NBT layout evidence. Breeding food
resolves the loaded species item tags once at startup, retaining the separate
horse golden-food and llama hay-block prerequisites. Loot table names are
admitted only when the embedded vanilla entity catalog contains that root.
Custom blueprints already registered by another slice are never replaced.
Special projectile/beam/boss families register their own attacks; absence of a
native special goal does not silently turn them into generic melee mobs.
`armor_stand.rs` owns item placement, player equipment exchange and the five-tick
double-punch break loop; mannequin interactions remain out of scope. Placement
rejects blocked/occupied bodies and the downward face, faces the player in
45-degree increments and debits one item only after admission. Equipment uses
the appearance's canonical six slots and full-stack inventory synchronization;
empty-hand extraction selects an occupied slot by relative click height. Name tags
and marker stands pass through to generic item interaction, allowing renaming
without placing the tag into equipment.
Hand slots require stored `ShowArms`; `DisabledSlots`, small and marker stands
retain their NBT behavior. Survival punches/projectile deaths drop the named stand
and all equipment once under `entity_drops`; environmental deaths release equipment
without a stand item. Equipment drops preserve full stacks, bypass mob drop chances
and armor wear, and exclude vanishing-cursed stacks. Creative and bypassing `/kill`/
void deaths produce no drops, matching local `ArmorStand.hurtServer`,
`brokenByPlayer`, `brokenByAnything` and `kill` bytecode. The five-tick punch window
is separate from environmental hurt and transient, like vanilla `lastHit`, without
another persisted owner.
Native checkpoints/WAL retain equipment through the existing `equipment` compound
and pose/arms through opaque entity NBT, including Anvil export. Stored pose/arms
project to clients but have no editing UI. Local 26.1.2 `javap` confirms
ArmorStand flags at data index 15 and six ROTATIONS accessors at 16–21 (three
float32 values each), and existing SetEquipment's VarInt entity id, slot byte
with bit-7 continuation and optional item-stack codec. Taming, mounts and full
species-special AI remain separate capabilities from baseline registrations.

Durable owner references resolve through the entity store's UUID-to-dense-slot
index, maintained alongside the wire-ID index on spawn, removal and swap fixup.
This avoids whole-population scans for projectile owners after wire-ID
reassignment on restart; live-owner lookup rejects retained dead rows.

Native movement (`entity_motion.rs`) dispatches on the blueprint movement
class: ground bodies use swept collision, step-up and obstacle jump controls;
flyers steer in three dimensions without gravity; aquatic bodies probe the
whole swept body against known water and try a fixed set of local alternatives;
amphibious bodies use swimming in water and ground controls on shore.
Stationary bodies never wander or integrate locomotion. Missing current
columns freeze bodies without consuming air; missing neighbouring columns
remain collision barriers, not climbable walls. Contract-derived spider
climbing is capped at 0.2 blocks per tick and clipped at ceilings. Body/head
yaw and pitch follow movement or the selected chase target through bounded
turn controls and are projected from the native entity columns.
Blueprints with `FleeFire` also execute continuous panic after a hit for 100
ticks (1.25x movement speed) or while burning (2x), ahead of chase/breeding.
A retained recent player attacker supplies the escape direction; otherwise a
deterministic episode heading replaces wander's idle window. This uses native
hurt/fire state and introduces no session-owned panic timer.
Fish retain grounded dry-land flopping; aquatic dry-air exhaustion emits
merge damage intents (300 air, damage at -20, then reset to zero).
`Body::air_supply` is retained by the entity journal/checkpoint and the
vanilla `Air` tag. Navigation here is deterministic bounded local steering,
not vanilla schooling, unbounded A* or full species-specific flight AI.

Mob AI decisions follow the distance to the nearest player, in chunks
(Chebyshev): every tick within 2 chunks, every 2 ticks within 4, every 4 ticks
beyond, ticket areas without players included; a mob is phase-shifted by its
id, so every tick carries an even share of each ring. Physics and collision
run every tick: between decisions a mob keeps executing its last one (its
horizontal velocity), so motion and interactions match a per-tick mob. A mob
resting on the ground with no standing motion is not integrated between
decisions while every chunk under its footprint keeps the revision the
previous entity phase read; any block edit, load or eviction there wakes it
in the same tick. Bound: a player reacts in 150-250 ms and sees mobs through
network lag (~50 ms) and client interpolation; at 20 TPS a 100/200 ms stale
decision + lag stays at/inside the reaction band, and melee/hit range always
decides every tick. Below 20 TPS the periods stretch with the tick; the cure
is the tick budget. Measured on 100k spread cows with one player
(`core-bench spread`): tick p50 25.1 ms without cadence, 14.0 ms when
physics was also thinned (rejected: a 4x step is not four steps), 14.7 ms
as built, entity phase p50 19.9 -> 10.2 ms (receipts
`.analysis/codex-logs/v2-stage1/core-bench-spread{,-lod,-lod-phys2}.log`).

A blueprint carries the `mc-data` vanilla mob category. Tick-owned natural
hostile spawning (`natural_spawn.rs`, `natural_spawn_hostile.rs`) attempts
one pack per eligible active chunk every tick before inputs, within 128 blocks
of a player. Y is sampled uniformly from the dimension floor through one block
above the column's highest block; failed solid/unsupported samples are not
retried into guaranteed surface births. Other categories retain 20-tick
passes and creatures their 400-tick cadence, sampling columns 24–64 blocks
away with their existing enclosed-floor/water habitat sampler.
All seven natural categories select weights and inclusive pack sizes from
the biome at the actual spawn height (`monster`, `creature`, `ambient`,
`axolotls`, `underground_water_creature`, `water_creature`, `water_ambient`).
Available blueprints are renormalized in table order; missing/empty biome
tables fail closed, with no plains or blueprint-weighted substitution.
Every member revalidates its own biome, full body clearance, fluid habitat,
entity overlap and distance from every observer. Land creatures require
grass or the shared old natural-ground fallbacks (sand, snow, mycelium and
the other data-defined surfaces), and brightness ≥9. Monsters require zero block light and the old
vanilla sky-roll/attenuated-brightness admission, so enclosed caves can
spawn during daylight while torches prevent spawning. Ambient cave bodies
require darkness. The authoritative light owner supplies raw light and
time/weather-adjusted brightness; natural spawning does not own a light cache.
Slimes use separate Overworld admission: below Y40 in seed-derived slime
chunks regardless of light (1/10 attempt chance), or in swamp/mangrove swamp
at Y51–69 with light ≤ a random 0–7, the eight-phase moon brightness check
and a 50% roll. They never spawn on ordinary plains/savanna surfaces.
References: [Mob spawning](https://minecraft.wiki/w/Mob_spawning) and
[Slime](https://minecraft.wiki/w/Slime). Exact pack RNG order is not a goal.


The embedded supported-biome tables include canonical creeper packs (weight
100, minimum/maximum four) alongside zombies, skeletons and spiders. These
entries come from the local 26.1.2 vanilla biome data for plains, forest,
taiga, savanna, desert, swamp, river and ocean; a supported creeper blueprint
alone does not grant natural spawning in an absent biome table. Hostile
spawn-breadth coverage uses an authored eligible plains floor and a creative
observer so worldgen terrain and combat deaths cannot mask biome admission.

Category caps are 70/10/15/5/5/5/20, from the shared contract. The global cap
is scaled by the union of resident eligible 17×17 observer chunk squares /289,
never multiplied for overlapping players. Each observer's local category count
includes mobs whose chunk center is within 128 horizontal blocks of that
player's actual position. A candidate chunk is locally eligible when at least
one player covering its center remains below the category cap; a capped player
does not veto an overlapping uncapped player. Persistent, named, tamed or
mounted bodies do not consume natural caps. Miscellaneous entities do not spawn
naturally. The spawn pass builds one reusable per-chunk census for all seven
categories, reusing observer-position and local-count storage between passes.
Global and local counters advance with each admitted birth, so later pack
members see the same pass's newborns without rebuilding the census.
Collision admission queries chunk-local
entity IDs, expanding by the largest existing half-width to retain exact
cross-border and giant-body overlap behavior rather than scanning the whole
population for every candidate.
`spawn_mobs` and `spawn_monsters` gate births, not despawning.
Nonpersistent non-creatures are checked every tick and despawn beyond the
category distance (64 blocks for water ambient, 128 otherwise), or with
1/800 chance per tick beyond 32 after 600 idle ticks; returning within 32
resets the idle timer. Peaceful removes monsters
even when persistent. With no nonspectator observers, distance despawning
pauses. Item entities retain their 6000-tick lifespan. Sunlight ignition
refreshes the shared retained fire timer rather than clearing unrelated
fire or generating duplicate damage; rain suppresses sunlight ignition.
Any nonempty head item protects a daylight-burning mob. Damageable head items
take the vanilla random zero-or-one wear and publish equipment changes; a
breaking item still protects that burn check, with ignition eligible on the
next check. Other armor does not absorb daylight wear.
Sheared sheep still graze to regrow wool.

Native persistence uses a compact `EntityStateImage` for persistence/idle,
air/fire, lifecycle/love, loot and absolute nonplayer protection deadlines.
Changed images are separate journal post-images, preserving birth/removal and
checkpoint motion conventions without repeating appearance blobs for timers.
Lifetime age advances once for every live entity at the start of a tick.
Full/state images anchor age at the committed output tick; vitals images and
replay's final tick advance older anchors by elapsed ticks. Age alone does not
make a state image dirty, avoiding a per-entity journal record every tick.
Newborns anchor age zero at their birth's output tick. Checkpoints carry vanilla
`PersistenceRequired`, `Air`, `Fire`, `InLove` and signed ageable `Age`; lifetime
age and nonplayer protection deadlines use separate native keys.
Recovery hydrates these values and child dimensions before simulation resumes.
Melee cooldown, hurt-window/excess-hit history and horizontal combat impulse
remain transient on restart. Loot attribution remains durable, and restoration
rehydrates the last-player-hit cache from that retained credit. Players are not
restored from entity images: join/respawn rearms their 60-tick protection rather
than claiming a durable player spawn-protection timer.

Full `JournalEntry::Entity` post-images box their retained villager/projectile
payloads and share the living-state image with the comparison baseline through
`Arc`. The baseline hash table holds only tick, chunk and that pointer, not the
1,216-byte retained image. Full and compact `EntityState` records capture an image
once; full images seed the baseline immediately instead of capturing it again.
Clock-sensitive projections use copy-on-write so queued WAL payloads stay frozen.
Absent villager/projectile payloads allocate nothing. These are in-memory layout
choices only: JSON retains its existing fields, defaults and validation, without
a disk-format version change.

Motion-capable bodies retain their last emitted position, velocity and full
yaw/pitch pose in a transient comparison cache. Merely being a vehicle is not a
motion change, and the batch tick is not part of pose equality. Resting imported
chest minecarts therefore do not repeat their unchanged inventory-bearing full
images each tick. Retained-state changes still use the existing projected-image
gate; births, appearance changes, chunk crossings and actual pose changes keep
their full images. Vehicle metadata and stable fuel/fuse/paddle writeback take
mutable custody only when a value changes. Clock anchors and the journal format
are unchanged; position/velocity pushes and rotation-only changes survive replay.

A mob melees once
per 20 ticks; every living row keeps vanilla hurt invulnerability: for
10 ticks after a hit only a stronger hit lands, and only its excess
(`EntityStore::land_hit`). Accepted fresh melee/projectile hits retain their
source context through merge and apply `combat::knockback_velocity`:
base 0.4 away from a melee source, or along a projectile's horizontal
flight (captured before its removal), plus player sprint/enchantment
knockback along attack yaw. Protected/equal/weaker hits do not push;
a stronger hit inside the window neither renews knockback nor emits a
fresh hurt animation. Walker physics retains the vertical pop and decays
horizontal combat momentum separately from AI locomotion, through collision,
so the next chase decision cannot discard it. Victim-player motion is a
reliable `PlayerChange::Motion` / `SetEntityMotion`, converted once from core
blocks/second to wire blocks/tick; it survives skipped snapshots. This fixes
a missing production call to the already-tested knockback helper, not a
combat-difficulty adjustment. A wool
blueprint (sheep) rolls a vanilla color at spawn into its wool entity data,
drops that color's wool loot, is sheared once (1-3 wool, shears wear) and
regrows its wool by grazing a grass block to dirt. Players (re)spawn on the
first dry surface column within 10 blocks of the world spawn (vanilla spawn
radius) and ignore damage for 60 ticks after join and respawn (vanilla
`spawnInvulnerableTime`). The world difficulty defaults to easy (the vanilla
dedicated-server default the join burst announces); mob melee scales with it
like vanilla `Player.hurtServer` (peaceful 0, easy min(d/2+1, d), hard 1.5d)
and `/difficulty` reaches every client.

`entity_loot.rs` owns contextual native mob death: the existing embedded entity
table retains burning, sheep wool, child/slime size, direct/indirect attacker,
player-credit and held Looting context. Equipment has one canonical owner in
the appearance's six slots, with body armor/saddles retained in native loot state;
slot-matched item attribute modifiers and retained
vanilla `attributes` bases drive mitigation, attack, speed and knockback
resistance. Armor wears on accepted damage, and death respects drop chances and
Vanishing. Native loot credit, attributes and slime size are checkpointed and
journaled with entity state, not held by sessions. Slime/magma cube death emits
two to four half-size children in the same commit as the parent's death;
the retained death guard and journal tombstone prevent repeat splitting on replay.
Embedded-data mode includes canonical tool/weapon durability, armor equipment
slots and complete modifier ids/operations from the local 26.1.2 item component
reports, including bows, crossbows, tridents and fishing rods. Those mechanics
use the same item-facts resolver as full-data mode; missing defaults must not
silently turn admitted equipment into unworn or nonmitigating items.
Body armor and saddles project from their retained owner to immutable appearance
only when changed; no per-tick appearance cloning is needed. Edge equipment
updates use all eight exact 26.1.2 slots, including Body (6) and Saddle (7).
An equipment change also sends explicit empty slots to clear removed armor,
while health-only appearance changes do not resend unchanged equipment.

`mob_equipment.rs` finalizes the requested vanilla held-item kits for every spawn
source; drowned tridents/fishing rods are natural-spawn-only and shells have a
guaranteed equipment drop. Zombie/husk/villager-zombie and skeleton/stray/bogged
armor uses the regional-difficulty factor, leather/copper/gold/chain/iron/diamond
tiers, helmet-first continuation and per-slot enchantment chances. Enchantments reuse
the native loot-enchantment selector, not an exact vanilla enchanting-table RNG.
The regional formula reads live chunk `InhabitedTime`, elapsed game ticks and
sky-time moon phase. Each dimension adds one elapsed game tick per resident
player-ticking chunk per tick, deduplicating overlapping players. Only chunks
within simulation distance and with their centre strictly within 128 horizontal
blocks of a non-spectator player qualify; creative players count, spectators and
remote tickets alone do not. This follows 26.1.2 `ServerChunkCache.tickChunks`
(`gameTime - lastInhabitedUpdate`) -> `tickSpawningChunk` ->
`LevelChunk.incrementInhabitedTime`; exact vanilla iteration cadence is not required.
Counters live outside the shared COW chunk images, so steady-state ticks allocate
nothing for accumulation, do not clone chunks or invalidate wire revisions, and
visit player ranges rather than every loaded chunk. Regional difficulty includes
the pending elapsed ticks immediately. WAL records only chunk entry/exit boundaries
of active intervals, with an absolute counter and boundary tick, in the existing
tick batch. Unchanged active sets emit no inhabited-time records. Recovery advances
each open interval through the last recovered WAL tick, so counters roll back to
the same tick as players/world/plugin data (Q6); closed intervals do not accrue
offline or catch-up ticks. Checkpoints fold changed interval boundaries into
the retained NBT `InhabitedTime` field and capture open intervals in every
dimension's manifest. Continued accrual in an unchanged open interval updates
only the manifest, not chunk dirtiness or the expensive chunk codec. Closing
an interval marks its newer counter for projection before that interval leaves
the manifest. Native restart and Anvil export (including an uncheckpointed
WAL tail) share that projection. Pending, active or captured-unacknowledged
counters retain chunk custody across eviction and stale acknowledgements.
The chances follow the requested equipment scope ([zombies](https://minecraft.wiki/w/Zombie#Geared_zombies),
[skeletons](https://minecraft.wiki/w/Skeleton), [drowned](https://minecraft.wiki/w/Drowned)):
normal equipment retains 8.5% plus one percentage point per Looting level;
picked-up equipment uses `drop_chances` 2.0 and survives custody/WAL replay.
`mob_equipment_pickup.rs` admits improved weapons/armor through one serial
exact-stack swap and the existing appearance journal. `CanPickUpLoot`,
`equipment` and `drop_chances` persist through native and vanilla Anvil fields.
All living `/summon` targets accept the seven ordinary equipment slots and
per-slot drop chances, sharing `/give` item-component admission; explicit
`equipment:{}` suppresses the default kit, while omitted equipment preserves it.

Accepted cave-spider melee adds Poison through the existing native effects API:
140 ticks on Normal, 300 on Hard, none on Easy/Peaceful; shielding and hurt
invulnerability refusals add none. These values and modern NBT keys are taken
from the local decompiled 26.1.2 Slime, MagmaCube, CaveSpider, AttributeInstance
and DropChances classes.

Damage commit publishes a surviving player's reduced health immediately as
reliable, self-directed `PlayerChange::Health` / `ClientboundSetHealth`; hurt
animation and journal post-images alone cannot update the client's HUD.
The same committed health updates viewer-visible appearance metadata.
An i-frame refusal emits no health change, while a stronger excess hit does;
lethal commit keeps `Health(0)` before `Died`. Passive chestplate mitigation
scenarios deliberately take a hit without attacking the zombie, so their
unchanged mob-health journal is expected.
Every admitted entity hit also publishes `EntityEvent::Damage` /
`ClientboundDamageEvent`, using the same vanilla damage-type identity as
advancement tags. Cause/direct wire IDs are captured before projectile removal;
the edge sends the event before `RemoveEntities`, including for its own player.
Stronger excess hits publish damage without a new hurt animation and journal
their changed mob vitals. Refused hits publish no damage event. Skeleton arrows
retain disallowed pickup and their flight/removal WAL post-images, so an in-flight
checkpoint cannot resurrect a subsequently removed arrow.
Accepted armor-reducible hits also wear each correctly equipped armor piece
by `max(1, floor(incoming / 4))` durability. Reduction uses the armor present
for that hit; a piece reaching its maximum damage then breaks, and the normal
inventory contents sync publishes the empty slot. Refused hits, armor-bypassing
damage such as falls, and creative/spectator players do not wear armor.

Hostile mobs beyond the zombie (`hostile.rs`, `explosion.rs`): blueprints
carry a natural spawn weight (zombie, skeleton, spider, creeper 100), whether
they burn by day (zombie, skeleton) and whether they only hunt at night
(spider, a zombie-like melee at 2 damage, speed 5.6). Movement stays in the
parallel entity phase (`RangedAttack` backs off inside 7.5 blocks, `Swell`
stands still on a lit fuse); what needs the world and every entity runs
serially right after it. A skeleton draws for 20 ticks before releasing its
arrow, then waits 40 ticks (20 on hard) before drawing again, at a visible
player within 15 blocks. Its launch speed is 1.6 blocks/tick with vanilla
triangular inaccuracy `14 - 4*difficulty`; the arrow flies the vanilla path (0.99
drag, 0.05 gravity per tick), hits the first body on its segment for
`ceil(speed * 2)` scaled by difficulty, else sticks (`IN_GROUND` data) and
despawns after 1200 ticks. A creeper burns a 30 tick fuse within 3 blocks of
a visible player (it keeps burning to 7 blocks, loses a tick per tick beyond),
its swell direction rides entity data index 16 and at 30 it emits
`IntentKind::Explode`: the vanilla ray blast (power 3, resistance from
`reports/block_explosion.json`, else bedrock/obsidian immune), entity damage
by distance and exposure, decayed block loot, and an explode packet with
per-player knockback (`ChangeSet::explosions`). Projectile ownership, custody,
payload and flight now belong to the retained native projectile module.
Entity data indices were counted on the local 26.1.2 server jar:
creeper swell dir 16, arrow in-ground 10. The edge sends no
health float for non-living entities (arrow) and converts core velocities
(blocks/second) to the wire's blocks/tick.

Native item projectiles (`projectiles/`) share one swept world/body collision
owner with skeleton arrows. Bow charge, Power/Punch/Flame/Infinity and crossbow
load/fire/Quick Charge/Multishot/Piercing use the current held-stack identity;
ammunition and wear change only after every spawned projectile is admitted.
Thrown tridents retain the worn item, return it once through Loyalty, gate
Riptide on water/rain and stage Channeling lightning only after accepted damage.
Snowballs, eggs, pearls, splash/lingering potions, firework rockets, experience
bottles and wind charges use the same native item-action and flight path.
Burning arrows prime the exact struck TNT cell through the native TNT owner;
the collision result retains the cell rather than rounding an impact face.
Potion/cloud UUID cooldowns, piercing hit ledgers, rocket lifetime, payloads and
fishing water/bite/hook timers are retained, not session-owned. Fireballs, skulls,
wind charges, shulker bullets, llama spit and dragon fireballs expose that same
projectile owner to species attack modules; species decision AI is separate.

Held-item use follows 26.1.2 `LivingEntity.startUsingItem`: additional native
UseItem packets do not restart an active draw, food/drink countdown or shield
raise delay. Ranged charge uses the original authoritative server tick and the
native release action; thirty ticks produce full-power bow flight. Decoded
item-use tests cover single-debit completion, potion/milk remainders, and
full-hunger golden apples (Regeneration II/Absorption), while ordinary bread
remains refused at full hunger.

Native special mobs (`special_mobs/`, parity wave 2) register dedicated timed
goals before the generic species front door: blaze three-shot bursts and close
melee, ghast charging siege fireballs, pillager crossbows, witch splash selection
and defensive drinking, and breeze wind charges with grounded recovery jumps.
Guardian/elder beams stage direct merge-owned damage after warmup and continuous
line-of-sight; the elder fatigue aura is range-gated independently of sight.
Warden sonic attacks use the OLD positional sensing/range contract, deliberately
pierce walls, and keep recovery/cooldown separate from acquiring a new target.
Evokers create supported ring/line fangs and bounded owner-linked, limited-life
vex summons. Stationary shulkers peek during attack and launch UUID-homing bullets
through the shared swept projectile authority; accepted hits apply levitation.

Pillager phase/deadline, witch potion choice, fang owner/warmup/life, vex
owner/lifetime and wither spawn invulnerability ride `EntityStateImage` compact
WAL images and checkpoint `solaris:SpecialMob`. Bullet target UUIDs ride retained
projectile images and vanilla `Target`. Other decision timers intentionally reset
on restart, matching OLD's retained/transient split. Restoration reprojects
retained crossbow/wither metadata and clears transient beam/drinking/casting
animations. Malformed native variants, timers, duplicate records, contradictory
vanilla projections and incompatible checkpoint entity types fail closed; no
corrupt entity is silently skipped.
NoAI omission from the active special-mob pass retains live-owner runtime state;
the presence of another active mob cannot reset it. Death/removal still prunes
that state. Retained NoAI/special-mob fields survive checkpoint projection.
Finishing a witch drink rehydrates retained movement attributes rather than
overwriting them with the blueprint default.

Sculk shriekers retain their vanilla `warning_level` and vibration `listener`
NBT, with absolute native response deadlines so replay cannot repeat a warning
or warden birth. Nearby players share the verified `warden_spawn_tracker`
integer fields, cooldown and decay; summoning requires `can_summon`, non-peaceful
difficulty and `spawn_wardens`. Warnings apply Darkness, and level four admits
one collision-checked emerging warden. Warden UUID anger (`anger.suspects`),
emerge/dig/sniff/roar phases and calm timeout survive WAL and checkpoint;
anger-gated melee reuses merge damage and ranged attacks reuse the existing
wall-piercing sonic authority. Spawn data 1 and pose/data indices are measured
from the 26.1.2 jar. Sniffing and targeting remain deliberately simplified AI.
Vibration delivery visits a sparse native warden index, with distance admission
before body inspection and occlusion; worlds without wardens do no entity scan.
The transient index follows spawn, restore, vanilla-import ID reassignment,
removal and dimension transfer.


The wither ritual consumes the seven skull/soul-sand-or-soil structure cells
and publishes one boss birth in the same tick journal. The 220-tick healing,
invulnerable spawn phase ends in a source-excluding explosion; three head muzzles
launch owned skulls on a 40-tick cadence. Below half health the armor phase rejects
ordinary projectile damage. Admitted hits trigger bounded, gamerule-gated block
griefing excluding protected cells. The existing once-only, retained death owner
emits the wither's custom nether-star drop (the vanilla wither loot table is empty),
respecting `mob_drops`. The edge's reliable snapshot boss-bar baseline handles
spawn progress, health changes, death/removal and re-entry; packet 0x09 and all six
operation layouts are measured from the local 26.1.2 jar, not inferred wire IDs.
Warden vibrations, raids and dragon fight authority are separate capabilities;
real-client validation remains a parity-program gate, not a unit-test claim.

Fishing casts/reels a retained hook owned by player UUID. Entity hooks pull the
target; water bites apply Lure, Luck of the Sea and the OLD open-water rule
(four homogeneous 5×5 water/air layers). Catch, rod wear, item drop and XP orbs
commit once before the hook tombstone. Full-inventory Loyalty retains custody
until the worn trident can be inserted; owner wire-ID reassignment is resolved
through the maintained UUID index.

With `ender_pearls_vanish_on_death=true`, commit resolves the death set to UUIDs
once and removes its owners' pearls in one population pass, before immediate
respawn can change those identities. The false rule preserves the pearls.
Crossbows validate every multishot blueprint and finite launch geometry before
publishing any birth or spending charged ammunition; rollback removes only
unpublished staging, never a removal tombstone. Orphaned fishing hooks are
removed immediately, independent of their age.

Each changed projectile journals current position/velocity/rotation together
with its payload and custody. Native entity records retain the measured 26.1.2
NBT keys and namespaced state; only offline export writes vanilla entity regions.
Restart folds projectile post-images over checkpoint motion, unlike ordinary
mob motion below. Arrow metadata flags/piercing/in-ground are indices 8/9/10;
Loyalty/foil are trident indices 11/12, and fishing uses indices 8/9. These are
local jar/existing protocol layouts; projectile entity appearance contains
owner spawn data and the edge does not emit living-only health metadata.

`item_motion.rs` retains item launch velocity instead of replacing it with a
vertical-only fall each entity phase. The shared collision kernel integrates
javap-verified ItemEntity gravity (0.04/tick), air/ground drag, and fluid drag
with slow-item buoyancy; fishing's existing retrieval impulse therefore carries
the catch from source water onto the bank and into the normal pickup path.
Selected-item drops use the vanilla eye-minus-.3 launch, yaw/pitch-directed
.3/tick throw, and forty-tick pickup delay; generic/death/block drops retain
their existing custody owners. Pond retrieval, Q landing, and nearby block-drop
regressions exercise the same entity-phase post-images without a fishing shim.

Malformed retained projectile, falling-body or shared entity fields refuse
world open rather than skipping custody-bearing records. Diagnostics include
region path, chunk, entity index, UUID/raw UUID, numeric and namespaced type
identity and the failing field. Failed-open regressions compare complete
entity-region bytes before and after refusal; no recovery rewrite deletes the
rejected record.

Native animal lifecycle (`entity_life.rs`) uses the existing feed/breed intents
and one shared harvesting intent; it does not own network sessions. Birth gives
the child 24,000 growth ticks, child appearance and a half-sized collision body.
Every simulation tick advances age, love and parent cooldown regardless of AI
cadence. Food accelerates a child's remaining growth by ten percent; only adults
enter 600-tick love mode. Same-species partners court for at least 60 ticks and
approach each other before birth; deterministic merge consumes both love images
and applies the 6,000-tick parent cooldown immediately, rejecting duplicate pairs.
The startup species catalog supplies breeding foods and offspring. Native
tamed/sitting/mounted prerequisites gate pets and horses; this module does not
implement taming or riding actions.

Cow/goat/mooshroom milk, mooshroom bowls and brown-mooshroom flower stew, sheep
dyes/shearing and mooshroom-to-cow shearing validate the complete held stack at
merge, including tool wear, then change entity state and inventory together.
Output overflow and mushroom/wool drops enter the normal journaled spawn list.
Adult chicken eggs retain their deterministic 6,000–12,000-tick countdown.
The WAL records clock initialization/reset, not each decrement: existing batch
ticks advance love, breeding cooldown, unlocked child/tadpole age and adult
chicken `EggLayTime` during replay, including across health-only images and
multiple recovery folds. Live comparison advances the same clocks without
allocating another retained state image. Egg birth and the new randomized
countdown remain in the same committed batch; babies and other species do not
advance the egg clock.
Children, love, cooldown, harvest countdown and stew state ride entity journal
post-images and vanilla checkpoint fields (`Age`, `InLove`, `EggLayTime`, `Type`,
`stew_effects`). Growth publishes both child and adult baby metadata; schema
selection follows the 26.1.2 contract and local class inheritance (AgeableMob 16,
Piglin 17), not an unconditional baby accessor on arbitrary entities.
Armadillo scute/brush harvesting was not established by the inspected OLD
authority and is not invented here.

### Per-tick target grid

Every thinking chaser (zombie, golem, zoglin, piglin, ...) scanned the full
player, villager and golem tables for its nearest target, O(chasers x targets)
per entity phase. The entity phase now builds one `TargetGrid` per target class
(players, non-gold players, villagers, golem targets, zoglin targets) once per
tick, sequentially, as chunk buckets (counting sort, O(n)) over the existing
slot-ordered tables; a member scans the chunk ring `ceil(sight/16)` and applies
its own filters. A class is indexed only when at least 8 chasers use it this tick
and its table has at least 32 entries; otherwise the grid is the old linear scan,
so scenes without chasers pay one pass over rows and no build. Raiders' own
64/16-block target uses the same grid; the goal paths inside `special_mobs::run`
and `hostile.rs` still scan slices. `villager::defense::targets` uses
incrementally maintained dense type lanes for monsters and golems plus exact
resident chunk memberships only within two chunks of an eligible player, rather
than walking the complete entity population. Gossip is summed per eligible
player UUID with the same thresholds; the derived target list has no client or
authority role.
Correctness fence: the result equals the linear first-minimum scan, ties broken by
chain order (players, villagers, golems, zoglins) then table index; seeded
random-layout tests (`target_grid_tests`, `villager/defense_tests`) compare against
replicas of the old scans, including lattice layouts with equal distances.
Measured (`core-bench horde100|horde1000|peaceful100`, release, perf lane
CPUs 8-11, interleaved before/after, receipts
`.analysis/codex-logs/target-grid/{before,after}.json`; host load varied 2-12):
entity phase p50 with 10k zombies + 5k villagers: 100 players 74.1 -> 13.5 ms
(two interleaved rounds: 76.4/71.9 -> 15.6/11.4), 1000 players 101.4 -> 14.7 ms
(one round, 100 ticks). No-regression scene (50k villagers, 20k cows, no hostile
mob): 16.2/15.0 -> 17.7/14.9 ms, noise-level. 100k cows (spread): 66.0 -> 61.9 ms.
The scene's total tick stays 80-110 ms because villager brains dominate it.
Removal path: replace `TargetGrid::nearest` by its linear loop (identical result)
and delete `target_grid.rs`; keep `core-bench horde*` as the A/B scene.


### Remote tickets

A ticket names an owner, an area, a reason and a budget. Ticketed chunks are in
the simulated set exactly like player-loaded chunks. Over-budget requests are
refused explicitly. Tickets are part of the journaled state; restart restores
them with no downtime catch-up.

Vanilla level-31 tickets (`chunk_tickets.rs`, `forceload.rs`; 26.1.2
`TicketType.FORCED`/`ENDER_PEARL`, `ChunkMap.FORCED_TICKET_LEVEL`) reuse this
model. `/forceload` persists a radius-0 ticket `minecraft:forced:<x>:<z>` in the
owning dimension; forced and pearl tickets both expand to a two-chunk loading
ring and a one-chunk simulation ring (vanilla loads FULL at distance 2 and ticks
blocks at distance 1; native simulation is one set, so entities in the ring also
tick). Pearl tickets are derived each tick from live pearls whose owner is online
in any dimension, time out 40 ticks after the pearl leaves a chunk, are capped at
32 distinct chunks per owner and are never journaled or checkpointed. A pearl
whose owner is offline is parked in the checkpointed entity table instead of
being moved into the player file. The WAL records durable ticket changes only:
a post-image when a ticket appears or moves and a `TicketRemoved` tombstone when
it disappears, so `/forceload remove` cannot be resurrected by a stale image.

### Durability

- **Native authority and generation identity.** Core v2 persists one
  `<world>/native/state.redb` database for all three dimensions and global
  domains, plus `<world>/journal/batch-<lo>-<hi>.bin` tick WAL. The DB stores
  required versioned generation and dimension records, binding its world
  identity to the seed, generator revisions, mode, ore profile and geometry.
  `Core::open` validates them against configuration before owners start.
  There is no v2 `solaris/world.json`, child DB, runtime Anvil fallback,
  dual-write, or automatic historical-schema migration. Imported coverage,
  including all-air chunks, wins over generation; only genuinely absent
  coverage may generate. Stored read/identity/version failures fail closed.
- **Journal.** Batches of whole ticks, each framed with its tick range, length
  and checksum. Writers fsync a `.bin.tmp` file before atomically publishing
  its `.bin` name; crash-left unpublished `.tmp` files are ignored. Any
  incomplete header/payload, checksum mismatch, invalid header identity,
  extra trailing bytes, or JSON/serde decode failure in a published `.bin`
  fails `Core::open` closed, including the final batch. Diagnostics identify the batch path,
  zero-based record index and byte offset, entry kind when recoverable, and
  serde error for payload failures. Recovery does not rewrite corrupt files
  or checkpoint a silently shortened replay. Entries are
  absolute post-images (block state at a position, full slot contents, full
  entity/player record, and a tombstone for every removal). Replay
  applies every complete batch after the committed native checkpoint in tick
  order, so each key ends at its newest journaled image over one atomic base.
  Recovery requires a contiguous published WAL tail starting at checkpoint
  tick + 1; a gap refuses startup with the missing tick range and no rewrites.
  Recovery decodes, validates, routes and folds one published batch at a time,
  retaining only the newest domain images and the current batch. WAL length
  therefore does not multiply recovery heap usage; damage still refuses the
  entire open without modifying the checkpoint or published files.
  Retained entity clocks are projected once after the complete tail, not by
  scanning every entity after each batch; intermediate absolute/vitals images
  advance only their own anchors.
  The edge opens the synchronous core on Tokio's blocking pool: restoring
  durable block overlays may wait for chunk workers before listeners start.
  The journal writer inherits that exact validated pre-open tick interval while
  the exclusive world lease is held. Its first checkpoint reuses that coverage
  instead of decoding the recovered tail again; normal unverified writers and
  uncovered intervals still validate published files before checkpointing.
  A player or block entity is journaled only when its
  image differs from the last one journaled; an unchanged one is covered by
  an earlier retained batch or the checkpoint.
  **Journal cost is O(touched), not O(entities).** Owner dirty marks are the
  custody boundary: `EntityStore::row_mut` / `body_mut` / `parts_mut`,
  spawn/transfer-in, a chunk-crossing `set_pos`, and block-entity custody (`BlockEntities` get_mut/insert/remove/
  entry/retain/mark_dirty) list the row or cell once per tick; `journal_touched`
  drains that set and compares only listed rows with their last journaled image.
  Equality stays the final arbiter (a conservative borrow costs one comparison,
  never an image), so there is no hashing and no per-field setter. Deterministic
  clock images are projected lazily from their own anchor when compared. Live
  lifecycle decrements use unmarked row custody: egg initialization/reset,
  growth and embedded-projectile appearance changes still mark the row.
  Only dead clock-sensitive rows need per-batch rechecks: their stopped live
  clocks diverge from replay's projection, so the existing absolute images
  remain necessary. Activity changes and chunk crossings still rebase anchors;
  the journal-derived villager `brain_clock` is re-evaluated at those boundaries.
  Rebasing is chunk-local: actual activation/deactivation snapshots identify
  affected IDs through the existing sensor/villager chunk indexes. Loading an
  unrelated chunk, changing nonresident demand or consuming catch-up debt cannot
  refresh every entity baseline. Boundary snapshots are separate from debt
  post-images so consumption cannot truncate an older continuously active
  projection; both are cleared at the existing end-of-batch debt WAL drain.
  Eviction/quarantine also lists the affected chunk: removed authority requests the
  existing absolute reanchor/brain-clock stop, without inventing an interval
  or adding another WAL stream.
  Idle creeper metadata compares before mutable custody, preserving real wire
  changes without repeatedly marking every unchanged creeper.
  Evidence: 100 steady r1 native batches contained no `EntityState` entries and
  a median 117 full images per batch. An eight-species 800-mob reproduction
  attributed all 100 steady no-op owner marks to idle creepers; unrelated chunk
  churn compared all 100,000 rows despite emitting no entity images. Before/after
  receipts are `.analysis/codex-logs/opt-journal-r3-{before,after}.log`,
  `opt-journal-r3-attribution-{before,after}.log` and `opt-journal-r3-wal-summary.json`.
  One isolated before/after pair (four warmups, 24 samples) reduced journal p50
  from 52.939 ms (unrelated residency) / 51.702 ms (unrelated activity) to below
  1 us for both, with comparisons 100,000 to zero and no entity WAL images in
  either version. The 800-mob owner reproduction likewise fell from 100 to zero
  comparisons/no-op marks, still emitting no images. These are journal-only
  microbenchmarks, not native-100 TPS measurements.
  Movement writeback takes marked custody only for retained air changes;
  impulse and attack/pickup timer updates do not trigger image comparisons.
  The journal returns its drained touched-vector allocation for the next tick,
  updates occupied baseline entries in place and pre-reserves the first table.
  `journal_touched::tests::moving_rows_journal_microbench` isolates 100,000 moving
  rows with pose-only, transient-writeback and retained-state workloads; run it
  in release mode through `mem-run.sh -p`. The before/after receipts live under
  `.analysis/codex-logs/opt-journal-r2-{before,after}.log`. One before/after pair,
  each with four warmups and 24 measured samples, gave journal p50 47.522 ms to
  below 1 us for transient writeback and 100.194 to 75.109 ms for real retained
  changes; pure pose-only motion was already below 1 us. Baseline table values
  shrink from 1,232 to 24 bytes, excluding the still-allocated heap image and
  hash-table overhead; this is not total per-entity memory. The clock-insensitive
  cow scene measures compact state updates, not full-image storms or clock COW.
  It excludes mutation setup, output disposal, journal writer I/O and server/edge
  work; it is not a TPS claim.
  Removal boundary: restore ordinary marked movement custody and independently
  owned retained payloads.
  Rule for owners: do not take `row_mut` for a
  read or an unconditional pass over a population (the 10k villager bench's
  ~8.5-10 ms was equality scanning of all rows, see
  `.analysis/codex-logs/journal-touched/attribution.md`); a saturated touched
  set degrades back to the old cost. Removal path: iterate all slots as before.
- **Required player checkpoint records.** Every UUID named by the native
  checkpoint's player map must resolve to its exact readable native UUID
  record at the recorded capture tick, no later than the checkpoint tick.
  Missing records, malformed NBT, unsupported versions and canonical
  player-field decode errors refuse `Core::open` with DB/domain/key diagnostics,
  without rewriting records or dropping offline players from later checkpoints.
  Captured players merge into the previous map, preserving offline custody;
  an unlisted new player has no checkpoint record requirement.
  Cursor/crafting-grid item-codec errors also refuse recovery with the custody
  field and slot index; valid empty compounds remain empty slots rather than
  converting corrupt items into empty or omitted contents.
  Checkpoint encoding preserves every crafting-grid cell, including leading,
  interior and trailing empty cells; sparse custody never becomes a dense recipe.
- **DB admission.** Required checkpoint, generation, dimension, scoreboard,
  level-metadata and conversion-extra records must decode and validate before
  adoption, including all child state, persisted border and block/fluid queue
  images. Entity payload UUIDs must match their native record keys before
  hydration; mismatch refuses startup without rewriting custody.
  Saturated sequence counters, duplicate ordering identities, invalid
  geometry, missing required domains and inconsistent coverage fail closed,
  never defaulting to a new world. A missing DB alongside any persisted
  history (Anvil, old JSON manifests/contracts or WAL) refuses startup with
  the explicit `world import-anvil` instruction. Only a genuinely empty
  directory receives a complete initial native DB checkpoint before owners
  start; a failed read is never fresh-world admission.
- **Ordinary mob motion is checkpoint-durable only.** Position, velocity and
  rotation of non-projectile mobs are not journaled (measured ~76 MB per tick at
  50k mobs in the first journal format). After a crash mobs stand where the
  committed checkpoint saw them, which can be inside something built later.
  All captured mob motion is committed atomically with the other domains:
  crashes cannot mix positions from partially written checkpoints.
  Other entity changes (spawn, removal, health, carried items, equipment,
  variant) and all player state including position are journaled as post-images;
  lifetime age is reconstructed from committed tick anchors. Replay reaches
  the last complete batch for journaled state, but ordinary mob motion remains
  at its last checkpoint capture. This exception is not full motion rollback
  to the replay tick and does not claim per-tick mob-position durability.
- **Idle despawn counter is checkpoint-durable only (2026-10-18).** Advancing
  or resetting `no_action_time` alone does not dirty an entity WAL post-image.
  Native checkpoints still capture its current value; meaningful retained
  changes also carry the current counter incidentally. A crash may therefore
  restore a counter preceding increments or resets, moving idle-despawn
  eligibility earlier or later, rather than emitting a roughly 1 KB state blob
  per mob per tick. Lifetime
  age remains derived from existing tick anchors; lifecycle clocks retain their
  existing projection/event fences. Air, fire, protection, persistence flags,
  inventories, equipment, health, spawns/removals and all other retained
  gameplay changes still select their normal WAL images. The owner-world
  tail contained 353,186 `EntityState` records / 340,211,412 bytes; 351,484 of
  352,806 consecutive per-entity comparisons changed only age and this idle
  counter. Excluding the idle counter from equality is the primary growth fix,
  not a quota or a change to crash consistency of gameplay custody.
- **Atomic native checkpoints.** The tick freezes immutable chunk/revision,
  player/entity and root/child global images; the native writer thread owns DB
  writes (the journal thread owns the WAL), with one checkpoint in flight. The
  journal thread gates durable WAL coverage (fsync) through capture
  tick `c` **before opening checkpoint writes**, then stage all participating
  bases, overlays, metadata, dictionary/applied watermarks, player UUID records,
  full entity membership/tombstones, scoreboard and globals in **one Immediate
  redb transaction**. All dimensions share tick `c` and that commit; no separate
  chunk/entity commit may publish part of it. Only after durable commit does
  the writer publish committed tick `c`, ACK captured revisions/queue images,
  and prune batches wholly at or below **that newly durable checkpoint** `c`.
  Pruning does not depend on delivery of the ACK: the native transaction, not
  the tick owner's receipt, is the recovery authority. A crash before pruning
  merely leaves redundant batches that recovery ignores. These steps run on
  the native writer, so later WAL appends proceed during the commit.
  Checkpoint chunk work is dirty-only: unchanged active inhabited/debt clocks
  are manifest anchors, not reasons to recapture all resident chunks. A new
  interval boundary or real content mutation still uses the revision-fenced
  chunk path. This removes recurrent resident-scale encoding from checkpoint
  commits; it does not remove WAL backpressure. The existing count/byte-bounded
  handoff still waits if the independent WAL writer cannot keep up (including
  disk fsync contention). No queue growth, dropped batches or persistence-order
  bypass is permitted; residual wait requires a matching capacity-ladder
  measurement before changing that boundary.
  In normal running, the next gate reads the tick published before the previous
  ACK. A shutdown checkpoint may gate against an older tick while a commit is
  in flight; the live writer's validated pre-open interval and contiguous
  append coverage make that gate independent of concurrently pruned files.
  Pre-commit failure rolls back every domain, publishes/prunes nothing and
  retains dirty/empty queues. Post-commit ACK loss still recovers DB tick `c`;
  prune failure only retains extra WAL. Chunks cannot evict dirty captured
  revisions before their successful ACK.
  Explicit save requests queue FIFO behind an active capture; the next capture
  starts only after the preceding checkpoint ACK, so saves cannot overtake it.
  The default checkpoint cadence is **1,200 ticks** (one minute at 20 TPS),
  formerly 6,000. With successful checkpoints, the journal retains at most
  that interval plus checkpoint commit/queue lag, rather than an additional
  entire previous interval. This is a tick/time bound, not a fixed byte cap:
  emitted gameplay images determine bytes, and failed commits retain all
  required history. No WAL is deleted to enforce a quota before a durable
  covering transaction. The 2026-10-18 owner-world investigation and measured
  copy-only receipts are in `.analysis/codex-logs/wal-growth/`.
- **Recovery** = one validated committed native checkpoint + every complete
  WAL batch strictly after its tick, routed across all dimensions in one fold.
  WAL at or below the checkpoint is ignored even if pruning or ACK failed.
  No Anvil domain files or JSON manifest participate in runtime recovery.
- **Plugin compilation cache (2026-10-18).** The host enables Wasmtime's
  built-in persistent compilation cache using its default cache configuration
  and OS cache directory. Wasmtime keys compiled components by source content,
  engine/compiler revision and compilation settings; changed inputs miss the
  cache, and corrupt entries are recompiled. This cache is disposable host
  acceleration, not world/plugin authority or part of a backup. Unavailable or
  malformed cache configuration warns and retains normal compilation, rather
  than refusing an otherwise valid deployment. Guest validation, fuel/epoch
  budgets and Cranelift's `Speed` optimization remain unchanged. The copied
  owner-world startup profile attributed 47.79% self cycle overhead to
  regalloc2's main allocator alone, versus 0.04% to WAL batch decoding;
  unchanged plugin recompilation, not replay of covered history, was the
  observed startup bottleneck.
- **Plugin native authority (2026-10-14).** `solaris-v2` discovers the complete
  `[plugins].directory` and runs the production `solaris:plugin@0.7.0` component
  host, not the v1 simulation runtime. Invalid packages, duplicate ids/commands,
  failed guest startup and unsupported startup declarations refuse startup.
  Manifest-declared `before-build` hooks register automatically in deterministic
  package-id order with Deny failure policy. Unsupported kinds (including
  `before-damage`) fail before component compilation and native world admission.
  Coverage matches the v1 build-hook call sites: player block break/placement
  and paired/construction cells, bone-meal growth, successful composter fill,
  clicked/raycast bucket edits, and server-owned edits (`SetWorldBlock`,
  plugin/NPC physical structure portions and NPC harvest/tree-cut/mine/replant).
  Ordinary interactive block use, hoe/ignition operations, environment and
  damage are not newly hooked. No-handler actions retain the direct native path.
  The package directory is an explicit deployment selection; no first-party
  roster or product-domain policy is compiled into core.
  The immutable deployment command catalog is shared by native dispatch and the
  edge tree/permissions. Manifest-owned roots take precedence over available
  native roots; native aliases remain independent. Retirement removes live guest
  admission, not that reservation, and therefore cannot activate native fallback.
  Versioned plugin KV, tombstones, idempotent operation receipts, zones and
  native domain custody share root-global authority with the associated physical
  effects. Each tick journals only absolute post-images of touched keys and
  domains, new receipts, explicit domain removals and the global revision;
  it never copies the accumulated state into an ordinary WAL batch. The journal
  worker applies successful appends to its committed plugin-state mirror and
  serializes that mirror at the FIFO checkpoint boundary, off the tick. Child
  dimensions use the same authority, and recovery merges each delta once over
  that checkpoint. Operation receipts remain durable for the world's lifetime;
  age/count pruning must not make an existing id executable again. Late retries
  retain their original outcome and conflicting reuse is refused after recovery.
  Known cost: receipt memory and checkpoint size grow with the world's total
  operation count, because operation ids are free-form plugin strings and the
  host cannot tell an expired id from a new one. Planned fix (next WIT minor,
  `solaris:plugin@0.8.0`): the host issues per-plugin monotonic operation ids;
  receipts older than a documented horizon are dropped and a persisted
  low-watermark refuses (never re-executes) ids below it.
  Receipt memory and checkpoint size remain intentionally unbounded, while
  serialization stays off the tick. Safe eviction requires a durable older-id
  refusal rule; no such horizon or id-reuse contract is implemented.
  No plugin side database, Anvil fallback or independent durable stream exists.
  New checkpoint manifests list versioned `plugins` and `plugin_startup` domains
  and require both records, including an empty deployment. Missing or corrupt
  listed records fail closed. A pre-port manifest that lists neither domain
  proves no plugin state ever existed: recovery starts with empty plugin authority,
  and the next checkpoint writes both records and version markers. Fresh worlds
  and offline Anvil import likewise emit both in the same atomic checkpoint.
  Inventory transactions validate every debit, credit and storage CAS
  before applying any; reservations retain their items in the real source, and
  destructive native writes invalidate the corresponding source identity.
  Reservation authority caps active/structure-bound references at 4096 globally
  and retains only the latest 256 unbound terminal references in a FIFO. Bound
  committed references stay pinned for structure stages; new admission returns
  `Busy` at protected capacity. Cancellation releases the binding so terminal
  references can be evicted. Source/custody indexes avoid rebuilding the ledger
  and scanning connections on each physical input. Changed image/reservation,
  index and metadata keys journal independently instead of rewriting the ledger.
  NPC custody persists one record per domain key. Native advancement resolves
  entity UUIDs through the existing index (including dead rows awaiting removal),
  visits each owned record once and journals only changed records plus revision;
  it does not scan every entity for every record. The obsolete whole-map NPC
  image is unsupported and fails closed rather than losing custody.
  Plugin menus, NPC work/orders, structures and authored physical item profiles
  execute through native owner decisions rather than guest per-tick simulation.
- **Plugin completion ordering.** The tick seals targeted operation callbacks,
  command replies and command-result traces with the batch containing their
  native effects. It releases those messages only after the journal worker ACKs
  successful append, without waiting in the tick. This ACK is not an fsync:
  Q6's existing flush cadence is unchanged, and an unflushed crash tail may
  consistently roll back an already acknowledged effect. No reply-path flush
  is introduced. Prepared player commands join the required retained FIFO;
  completion callbacks, command replies and traces are never shed. Terminal
  failure or exhausted required capacity fails closed.
  Observations instead use separate 256-event queues per registered plugin with
  round-robin dispatch. Overflow drops the newest observation and increments
  `ScriptBoundary::observation_events_dropped(plugin_id)` without halting native
  simulation. Observations remain gated by their sealed batch's journal ACK.
  Guest timers consume pushed advancing native ticks, not wall time. The host
  receives the recovered native tick before startup callbacks, anchors `init`
  timer delays to it once, and receives each acknowledged tick before that tick's
  result callbacks. Runtime readiness and counters reflect actual execution.
- **Journal failure halts the core.** A failed `send` (journal thread gone,
  receiver disconnected) stops commit and
  publication: every connection gets a terminal `Disconnect` through its
  reliable queue and the tick thread exits. A full queue is not a failure:
  the tick waits for admission, preserving FIFO checkpoint/shutdown ordering
  and accepted-batch durability. Only the tick thread owns the tick
  broadcast sender, so its exit closes every subscriber (`Closed`), including
  subscriptions made afterwards; nobody waits forever on a halted core.
- **Shutdown and edge readers.** LOD holds a weak reference to the chunk source
  and upgrades it only for an active tile/program request; retaining a stopped
  `Core` handle does not retain the native database. Requests after source
  teardown fail closed.
- **Reliable outbox handoff.** Each tick the tick thread takes its
  per-connection queues and sends them as one owned batch over an mpsc
  channel before waking subscribers; it never locks a shared store. The single
  edge drain coalesces batches in FIFO order per connection and never extends
  a queue already sealed by `Disconnect`. The queue key is the only address of
  a `PlayerChange`.
- **Block log (vanilla-top50 G2, 2026-10-21).** Each dimension's tick owns a
  bounded CoreProtect-style history (`crate::block_log`): the merge loop, fire,
  plugin block sets and container write-backs note first-writer pre-images,
  and the tick's journal pass seals them into numbered records. New records and
  rollback marks are one `JournalEntry::BlockLog` delta in the same batch. The
  native writer upserts queued deltas into the `block_log` table (keys
  `[kind][dimension][seq]`; record values are NBT that names block states as
  `{Name, Properties}` and items as vanilla item compounds, never runtime ids,
  so rows outlive a game-version change) in its own durable transaction before
  the checkpoint commit that may prune that WAL, and deletes rows below the
  tick-captured retention floors. Rows are idempotent by key, so a failed write
  stays queued and a failed commit after a successful write loses nothing;
  recovery folds table rows, then the WAL tail. A row whose block or item no
  longer exists is skipped; missing or retired properties resolve like vanilla
  `NbtUtils.readBlockState`. This audit table is history,
  not gameplay authority: `/co rollback|restore` re-enters as one tick-thread
  command that fences each record on current block type or exact slots, stages
  writes like `/fill` and journals ordinary post-images. Anvil export omits it.

#### Offline Anvil conversion

The existing serving syntax remains `solaris-v2 --config PATH`; omitting the
option uses `server.toml`. Conversion commands dispatch before any network,
runtime, tick, chunk or journal owner starts:

```sh
solaris-v2 --config PATH world import-anvil --source ANVIL --destination NATIVE --receipt JSON
solaris-v2 --config PATH world export-anvil --source NATIVE --destination ANVIL --receipt JSON
```

Both commands use `mc_core::world_conversion::{import_anvil,export_anvil}`
with `(&CoreConfig, &Path, &Path, &Path) -> Result<ConversionReceipt, CoreError>`.
Configuration supplies canonical registries and missing-base generation
settings, not a serving storage-mode flag. Stop the source first; conversion
requires an exclusive world lease, distinct paths and a destination that does
not exist. Sources are never modified. Region chunks, entity-region records and
level.dat accept Java Edition 1.15 (DataVersion 2230) through 26.1.2 (4790).
Pure, version-ordered NBT transforms run before the existing 26.1.2 readers.
Legacy continuous packed block/height arrays are realigned before the 1.18
height, biome and chunk-layout transitions. Pre-1.16 level generator fields
migrate into dimension-based `WorldGenSettings`; unknown generator names fail
with a field-level error rather than silently selecting another generator.
Legacy DFU retains mixed byte/int section Y values and does not invent a missing
root yPos. The reader uses authoritative dimension geometry; only the current
writer canonicalizes these coordinate widths.
Player/item and map roots accept all older integer record-local versions,
including negative stamps, as vanilla's historical schema dispatch does.
Missing player versions use vanilla's -1 default; missing map versions use its
saved-data default, 1343. Record-local
versions may differ, as in a partially visited Java world. Missing versions on
region/level roots, and malformed, duplicate, inconsistent or out-of-range
versions on interpreted roots and saved-data records, fail closed. Saved data
without an implemented pure-NBT transform retains its original bytes and
DataVersion, relocated to the path modern vanilla reads; vanilla owns any
load-time DFU. POI `r.X.Z.mca` and external `c.X.Z.mcc` files are opaque copies:
only paths/filenames are validated, not chunk contents or versions. No NBT
conversion or gameplay-parity claim is invented for these deferred domains.

The importer admits historical root/DIM-1/DIM1 dimension and playerdata paths
without changing source custody. The 4772 level split supplies canonical saved-data
payloads to native metadata, including per-dimension world borders. Receipts carry
record paths, source/target versions, applied fix/version pairs, preservation
reasons and split saved-data paths; rejected fields retain their owning fix/path
diagnostic. Text-number precision is local to borrowed serde_json RawValue tokens,
not a workspace-wide change to Number representation.
Vanilla4772 relocation includes dimension-local raids/POI, global map IDs under
`data/minecraft/maps/` (with `last_id.dat` for idcounts), scoreboard and command
storage. Canonical destination collisions fail closed. Export retains deferred
file bytes instead of overwriting them with generated defaults; conflicting
active native-owned state refuses export rather than disappearing.
Entity UUID first-wins admission does not authorize merging conflicting inline
region and `entities/` records for the same chunk. Overlapping entity bodies
must be byte-identical after upgrade, before UUID filtering; otherwise import
fails closed. Diagnostics identify chunk files relative to the source root.
Any rejected record leaves staging unpublished and inadmissible to runtime.

Native metadata owns nonnegative integer `warning_blocks` and `warning_time`
losslessly: checkpoint/export retain imported values, including a vanilla4772
split's `warning_time=15`, rather than normalizing them to 5/300. Runtime border
geometry and packets are unchanged. Geometry remains shared across dimensions:
nonzero legacy centers yield different Nether centers after vanilla's /8 split
and still fail closed. Import receipts list that known gap explicitly.

Native runtime and converters share an exclusive directory lease. Anvil import
also holds a read-only POSIX record lock on Java's `session.lock`, interoperating
with `FileChannel.tryLock`; it never rewrites lock bytes. Reserved
`.solaris-conversion-*` staging paths, including symlink aliases, refuse runtime
admission before any directory, database or owner writes.

Unknown entity fields, including `Silent`, `NoGravity`, `Tags`, `LeftHanded`,
`CustomNameVisible` and `Glowing`, and safe unknown player fields remain in
resident opaque fields and WAL post-images. Recovery and checkpoint capture
preserve them without consulting stale database records. Fresh typed fields
override retained values; clearing an optional typed field cannot resurrect its
old value. Opaque retention preserves interchange data, not unsupported gameplay.
The converter-owned auxiliary envelope obeys the native per-record 16 MiB
budget. Oversized auxiliary retention fails with its domain/path diagnostic
before terrain import; it never silently omits a file.
Modern `custom_boss_events`, `random_sequences`, `scheduled_events` and
`stopwatches` saved-data files are validated as readable NBT and preserved
verbatim, including nonempty contents. Core v2 does not simulate their boss
events, vanilla random-sequence state, scheduled functions or stopwatches;
retaining and re-exporting them is not a gameplay-parity claim.

Import stages a unique sibling directory, builds immutable imported bases and
one initial all-domain checkpoint at the source's nonnegative `Time`, assigns
missing globally unique positive wire IDs above retained IDs, and preserves
offline player custody and safe opaque NBT/auxiliary data. Typed current fields
override preserved templates. Export opens native state read-only, folds the
complete WAL tail without owners, ticks, repair, source writes or pruning, and
enumerates stored/WAL-referenced chunks rather than unbounded procedural terrain.
If redb reports an unclean writer shutdown requiring repair, export refuses with
`world was not shut down cleanly; start and stop the server once to repair it,
then export`. Runtime admission may perform redb's built-in recovery as the sole
writer before semantic validation and owner startup; export never repairs the
source or manufactures a temporary repaired copy.
Only export writes Anvil regions, entity regions, player `.dat`, `level.dat` and
modern saved-data files; export emits no native DB/WAL/JSON contract sidecars.
Absolute deadlines remain native; export rejects delays above `i32::MAX` rather
than clamping them.
Negative Anvil relative tick delays are valid overdue work, not malformed data:
import derives the absolute deadline from the source game time plus the signed
delay, saturating at zero, and the native owner executes overdue work next tick.
The authoritative native fluid queue retains each exact fluid ID (`water`,
`flowing_water`, `lava` or `flowing_lava` in the `minecraft` namespace), not just
its family. WAL, checkpoint capture and Anvil export preserve that ID: vanilla
executes a scheduled fluid tick only when it matches the cell's exact fluid type.
Runtime scheduling likewise selects source IDs for sources/waterlogged cells
and flowing IDs for non-source fluid levels.
Native `SolarisJournalLsn` chunk metadata is excluded from Anvil export and
discarded on import: Java may preserve it as an unknown field, but an offline
import starts a new native journal lineage rather than trusting source decisions.

Both directions close/fsync staged data and the receipt before one destination
rename and parent fsync. Errors produce a failure receipt and publish no
destination; interrupted staging cannot admit runtime startup. Receipts record
status/error locators, paths/versions, hashes, capture/replay tick, dimension/domain
counts, reminted IDs, normalizations and opaque retention. Only a complete
conversion is PASS, not a synthetic roundtrip. Operators may set `world_dir` to
the imported native destination after conversion.

### Farming

`farming.rs` retains palette/revision-cached section eligibility and hashed
`random_tick_speed` sampling: unchanged chunks never rescan block palettes.
`plants.rs` adapts the shared OLD 26.1 plant rules to authoritative native
state, retaining stems/fruit, cactus and sugar-cane three-block limits, bamboo
seedling/foliage/stage and sixteen-block limit, nether wart, berries and cocoa,
and oak/birch/spruce/jungle/acacia/dark-oak growth (including the OLD two-by-two
spruce/jungle/dark-oak shapes). Bonemeal produces one `Cultivate` footprint
covering every read and output plus exactly one inventory debit; failed room
checks do not spend it. Natural multi-cell growth validates recorded reads
before applying any output. Plant support cascades and detached stems settle
from native block changes, not session ownership.

`leaves.rs` repairs neighbour distance through the shared persisted native
block-tick queue, propagates repairs to neighbouring foliage, and defers
missing chunk-edge inputs without destroying leaves or losing work. Natural
nonpersistent distance-seven foliage decays through the random sampler with
the OLD independent sapling/stick/apple drop chances and `block_drops` gate;
player-persistent foliage never naturally decays. Block outputs and spawned
loot use the existing tick journal and chunk/entity checkpoint paths.
Crops and saplings retain authoritative day/weather-darkened light gates.
Rain hydration, trampling and grass light rules remain separate.

### Scheduled blocks and redstone

`block_ticks.rs` owns deterministic scheduled block work ordered by deadline,
priority and insertion sequence. Chunk imports precede input handling, in
sorted chunk order. Unavailable cells retain their original pending work;
draining never loads a chunk or drops the unprocessed frontier. Absolute
per-chunk queue post-images (including empty queues) share the tick WAL batch
with block and inventory mutations. The native checkpoint retains absolute
deadlines and sequence identity; only offline Anvil export encodes `block_ticks`
in the existing `i/x/y/z/t/p` layout with remaining delays. Checkpointing an
unloaded queue captures its native chunk state without simulation admission.
Native fluid queues keep their independent clock representation.

Empty block/fluid coverage is evicted only after a successful checkpoint ACK
for the captured empty image, with no newly queued or dirty work and an
unloaded, clean world column. Merely emitting a WAL post-image is not a flush
ACK; retaining that fence prevents stale stored work from being resurrected.

`redstone/` runs inside the native block phase, not a legacy `mc-net` session.
Dust settles connected horizontal/rising/descending wires with strength
attenuation and registry-exact connection states. A shared 1024-cell tick
budget deduplicates component seeds; changed frontiers continue on later ticks
while stable networks sleep. Torch inversion, repeater delays/side locking, comparator
compare/subtract and analog container fullness, observer leading/trailing
edges, button releases, entity-driven pressure plates, powered doors and
trapdoors, lamps, daylight sampling, powered rails and detector-rail occupancy
use the retained scheduler. Levers, button pressing, diode settings, daylight
inversion and note tuning enter through ordinary hand-use block intents.
Trapped chests emit viewer counts independently of their inventory signal.
Pistons resolve the entire attached structure before applying any cell, sharing
one twelve-block limit across forward collisions and slime/honey branches.
Slime and honey do not adhere to one another; glazed terracotta is pushed but
not dragged or pulled. Unextended pistons travel with the structure, while
extended pistons and block entities remain immovable. Immovable destination
collisions refuse a push (or leave a failed sticky pull behind); immovable
side neighbours do not adhere. Fragile destination blocks pop with ordinary
block loot, never as moved blocks. Sources clear before retained destination
states publish through the existing tick block-write/WAL path.
The reaction oracle fixture covers all 1,168 block identities from the local
26.1.2 server (all states agree on reaction, block-entity and hardness flags).
Ordinary copper blocks remain movable; item/opaque block entities retain the
conservative refusal policy even where vanilla would destroy them. Comparator
output metadata is disposable and follows vanilla's destroy reaction.

`vibration.rs` dispatches common native action events with the measured 26.1.2
frequency table, wool/carpet damping, wool acoustic occlusion and distance-delayed
listener selection. `sculk.rs` owns normal/calibrated sensor power, comparator
frequency, active/cooldown phases and calibrated back-input filtering; adjacent
amethyst retransmits the frequency. Nearby entity deaths transfer XP once to a
catalyst's bounded cursors, suppressing orb awards and spreading sculk/veins over
the native replaceable tag. Exact vanilla `listener`, `last_vibration_frequency`
and `cursors` codecs use opaque BE WAL/checkpoint images; namespaced absolute
deadlines preserve in-flight vibrations and phases without restart reactivation.
Retained malformed images fail closed. Spreading is bounded local gameplay,
not vanilla RNG ordering or ancient-city generation.

Block-family replacements invalidate native storage at write admission; the
shared removal journal event also tombstones opaque imported-base images before
final replacement images. Explosions and unsupported pops use that same path.
Recovery does not re-journal already-durable removals; coalesced sculk recreation
discards image-less transient owners. Opaque property changes retain the
wave-three block entities. Unloaded sculk
owners/listeners are evicted and rehydrated from durable images; quiet pending
vibrations write no per-tick WAL image, while root and dimension checkpoints
refresh vanilla remaining travel time from the retained absolute arrival fence.


Idle wooden-button, plate and detector-rail sensing backs off to ten ticks.
Each sensing pass rebuilds one reusable balanced AABB census from current
alive bodies; bounded local queries prune distant populations while preserving
giant/border intersections. Natural spawning's separate census is not reused
for these current-pass checks. Obsolete idle polls cannot shorten a pressed
plate/rail's twenty-tick tail.

Rising-edge note playback publishes a reliable block-state update followed by
native protocol `0x07` BlockEvent action/parameter `0/0`. The vanilla client
reads the authoritative note/instrument state and supplies sound and particles;
instrument facts include property-copy variants, head instruments and copper
variants. Ordinary notes require air above, and sustained power is silent.

Repeater locking requires a sustained side-diode output. Inspection with
`javap -c -p` of local Mojang 26.1.2 `DiodeBlock` and `RepeaterBlock` in
`.analysis/decompile/client.jar` confirms that neighbor input changes schedule
an unlocked diode transition, a powered diode without input turns off at its
deadline, and `isLocked` requires a positive alternate signal. The native
`repeater_delay_and_side_lock_are_authoritative` fixture supplies the side
repeater's north input and checks unlock/turnoff after that source is removed;
an initial `powered=true` state alone is not a persistent locking source.

`redstone/containers.rs` owns one-item hopper push/pull, eight-tick transfer
cooldowns, powered disable, component-preserving item suction, dropper
container delivery/ejection and dispenser dispatch. Hopper, dropper and
dispenser menus expose their native retained slots. Inventory post-images and
vanilla opaque block-entity NBT retain components and hopper cooldowns across
native checkpoints; powered-property changes preserve the
same block entity instead of clearing its contents. Projectile dispensing
uses the native projectile owner; custody is debited only after admission.

Container signal invalidation belongs to `redstone/custody.rs`: mutable native
inventory access, creation/removal, and viewer transitions mark positions through
one dirty-position recorder. Read-only map access cannot bypass this boundary.
Hydration marks only newly admitted custody; chunk residency and block-shape
changes also invalidate affected positions and paired chest halves. Redstone
drains that work in position order before scheduled components, preserving the
previous next-tick observation of automation. It no longer sums every loaded
container's slots on idle ticks. Mutable borrows conservatively invalidate even
when the caller ultimately leaves slots unchanged.
Residency invalidation includes a one-block halo so the still-loaded half of a
double chest crossing an x/z chunk border also updates when its partner leaves
or returns. A separate resident-signal set preserves the original lifetime
reactive-section hydration fence: signal reloads do not rescan/reseed every
reactive block.

The boundary is signal wake detection only, not journal/checkpoint authority or
container automation. Test-only full-scan equality and menu, transfer, viewer,
removal and recovery regressions fence it; production has no full-scan fallback.
`core-bench containers` measures the same 20,000-container scene before/after,
with idle and four-position mutation workloads. Shared-machine sanity receipts
are not exclusive performance evidence. Reverting this optimization means
restoring the original signal scan and removing the tracked custody wrappers,
not retaining a second inventory authority.

Non-exclusive 2026-10-14 sanity runs (300 ticks per workload; release build,
20,000 chests/338 loaded chunks, zero players) observed tick p50/p95/p99 in ms:

| Workload | Before | Dirty-driven |
|---|---|---|
| Idle | 40.142 / 41.658 / 45.393 | 2.649 / 3.056 / 3.262 |
| Four inventory mutations/tick | 40.683 / 42.491 / 44.268 | 2.660 / 2.992 / 3.063 |

Receipts are `.analysis/codex-logs/core-v2-bench/containers-20k-ticks300-baseline-nonexclusive.json`
and `.analysis/codex-logs/core-v2-bench/containers-20k-ticks300-post-final-nonexclusive.json`.
These shared-machine measurements are sanity evidence only, not final capacity
or an exclusive A/B performance claim. The benchmark excludes journal I/O,
mutation setup, and output disposal; other tick/block work remains included.
Saved same-scene release executables are `core-bench-redstone-baseline` and
`core-bench-redstone-post-final` in that receipt directory. After the main agent grants
an exclusive window and pauses competing QA/graphical loads, rerun A/B/A/B with
1,000 ticks per workload and fresh receipt labels:

```sh
.analysis/tools/mem-run.sh -x -m 8G bash -c '
  .analysis/codex-logs/core-v2-bench/core-bench-redstone-baseline containers 1000 --label exclusive-baseline-a &&
  .analysis/codex-logs/core-v2-bench/core-bench-redstone-post-final containers 1000 --label exclusive-post-a &&
  .analysis/codex-logs/core-v2-bench/core-bench-redstone-baseline containers 1000 --label exclusive-baseline-b &&
  .analysis/codex-logs/core-v2-bench/core-bench-redstone-post-final containers 1000 --label exclusive-post-b
'
```

These are OLD-core gameplay ports, not an exact vanilla update-order claim.
Pistons use atomic block updates rather than moving-piston animation; queued
piston cycles survive native checkpoints and tick-WAL replay. Block entities
retain the conservative refusal policy. Dust conductor/shape
rules and powered-rail connectivity remain approximations; minecart entity
motion belongs to the vehicle slice. Daylight sampling currently models
exposed clear-sky time rather than sky occlusion/weather. Native note events
cover the tested sound/particle path, not a general music playback subsystem.

### Block and sky lighting

`mc-core/light.rs` owns a world-level cache of the shared vanilla light engine.
Chunk workers bake missing arrays before generated chunks enter the native
save queue; admission reconciles the resident 3x3 boundaries. Committed block
edits run removal/addition BFS against each individual post-image, never a
whole-chunk recomputation. Skylight occlusion can reseed the edited vertical
column; lateral propagation and emitted light are confined to the neighboring
3x3 chunk window. Identical emission/opacity/sky-propagation substitutions
short-circuit the light work.

Development builds optimize `mc-world` at level 3, alongside the existing
worldgen override, because admission reconciles lighting on the tick thread.
The remaining core stays debuggable; this does not change lighting authority,
admission ordering, or release-build behavior.

The cache and immutable snapshots share packed COW light sections. A changed
light-only neighbor receives a content revision and dirty retention just like
a block edit. Native checkpoints retain section and boundary light arrays;
offline Anvil export emits vanilla `SkyLight`/`BlockLight` arrays. Journaled block
post-images reproduce derived light on replay and admission, so light is not
an independently drifting session-owned state.
Gameplay samples raw channels from `ChunkWorld::light_at`; local brightness
subtracts day/weather darkening from sky only, leaving block emission intact.
The daylight curve is the old 26.1.2 periodic-spawn piecewise timeline
(`11867–13670` sunset, `22330–24133` sunrise), not the earlier Minecraft
celestial-angle cosine. Monster storm admission separately uses the old
ten-level sky subtraction while thunder is active.

`mc-edge/light.rs` publishes the same baked arrays in full chunk packets and
credit-safe `LightUpdate` deltas, including zero-section masks when sources
are removed. It uses the existing 26.1.2 protocol layout and shared light mask
encoder: 2048-byte nibble arrays, padded boundary sections, and VarInt chunk
coordinates for packet `0x30`. No edge-side recomputation or guessed wire
layout owns gameplay lighting.

#### Bounded whole-chunk admission experiment (2026-10-16, G3)

**Bottleneck and boundary.** The existing loader result drain and recent-cache
returns could admit an unbounded batch on the tick. Admission's synchronous
light fixed-point solve was a known hotspot (the earlier MIX-01 log recorded
29.55 ms in the chunks phase). The G3 release replay measured a 73.30 ms login
chunks maximum and 84.581 ms login wall-tick p99 before this change. These are
new nonexclusive 6 GiB runs, not a restatement of the earlier timing.

The first G3 experiment limited tick admission to **K = 1 whole chunk**;
decision 004 below replaces that fixed limit with a measured time budget.
Batch order remains current nearest-player/ticket want priority, across
finished worker results and recent hits. Remaining results stay outside resident
storage. Ready results plus
queued/running loader jobs share a hard cap of **32**; draining the result
channel transfers, rather than releases, that capacity. Dispatch pauses at the
cap. Stale/unwanted completions release their own tickets without clearing a
newer request or admitting twice.

**Correctness fence.** The existing `relight_admitted` runs synchronously to its
fixed point before `ensure` returns and the chunk can be published. There are
no deferred seams, edit fences or off-thread light jobs. Startup's unlimited
`ensure_blocking` still solves the complete joined batch. Hydration now drains
mutation-owned chunk positions instead of scanning every resident each tick.
Eviction checks newly changed chunks, removed wants and matching checkpoint
ACKs; the existing dirty/inhabited retention predicates are unchanged.

**Measured effect and stop rule.** On the same MIX-01 command (100 clients,
10,000 villagers, seed 42, view 6, 20/11/20-second windows), the minimum batch
produced a 36.80 ms login chunks maximum (p50 4.92 ms versus 15.98 ms before)
and 56.708 ms login wall-tick p99. The 2,079 login samples contain 2,060
admissions, every batch exactly one; admission plus its fixed-point solve alone
peaked at **36.60 ms**. Ready plus pending jobs peaked at 32.
Thus `floor(8 / 36.60) = 0`: no positive whole-chunk batch meets the 8 ms
boundary, and K=1 is the required minimum rather than a percentage-based tuning
knob. **The G3 performance acceptance fails.** The after replay subsequently
aborted when the burst cohort missed its all-client readiness deadline; its
windows are marked invalid and it is not a full MIX-01 correctness/performance
pass. The raw login observations establish the minimum-batch budget miss only.
Initial login also grew from 41.247 s to 104.355 s; K=1 is not throughput-safe
for this cohort deadline and is not a deployable remedy.

**Focused attribution.** A release in-process benchmark admitted each of the
frozen fixture's 2,819 native chunks into an empty world and into a growing
resident world holding 10,000 live villager rows, with a published snapshot held
across admission. Native read/bake was outside the measured admission; there was
no entity-brain, journal or edge execution. The worst populated admission was
chunk `(33,31)` with 1,752 previously resident chunks:

| Phase | 10k-villager resident world | Same chunk, empty world |
|---|---:|---:|
| Queue, accept and initial revision | 0.011 ms | 0.004 ms |
| Targets and context construction | 0.037 ms | 0.012 ms |
| Boundary seeding | 0.081 ms | 0.554 ms |
| **Fixed-point relaxation** | **46.144 ms** | **2.341 ms** |
| Relaxation queue pops | 676,995 | 41,991 |
| Cache finalization | 0.043 ms | 0.004 ms |
| Install light (including chunk clone) | 0.117 ms | 0.009 ms |
| Chunk COW clones | 8 | 0 |
| Clone payload bytes, lower bound | 35,696 | 0 |
| Clone time (included above) | 0.084 ms | 0 ms |
| Light revision bumps | 0.002 ms | 0 ms |
| **Total ensure** | **46.482 ms** | **2.963 ms** |
| Block-entity hydration | 0.021 ms | 0.003 ms |
| Village notify | 0 ms | 0 ms |
| Eviction checks | 0.014 ms | 0.001 ms |
| Snapshot rebuild (outside ensure) | 0.150 ms | 0 ms |

Across pairs, populated ensure p50/p99/max were 3.957/19.781/46.482 ms;
empty-world values were 1.781/3.041/7.363 ms. The resident neighborhood, not
the presence of villager brain work, creates the expensive seam relaxation.
Chunk section palettes/indices are themselves shared by `Arc`; the clone byte
counter is a payload lower bound, not allocator-exact accounting of nested
metadata. No sample cloned for more than 0.118 ms. This attributes the
minimum-batch miss to fixed-point relaxation rather than deep chunk copies or
hydration. Temporary subphase timers and the benchmark inclusion were removed;
their source snapshots and raw log are retained only in ignored evidence.


Receipts (relative to the admission worktree):
`.analysis/codex-logs/admission-budget/before/summary.json`,
`.analysis/codex-logs/admission-budget/after/summary.json`, and each run's
`MIX-01/rung-1-100/{result.json,server.log,login-samples.jsonl}`.
`.analysis/codex-logs/admission-budget/run-replay.sh` records the identical
release command, `MC_CORE_PHASE_LOG=1`, nonexclusive `mem-run -m 6G` scope and
canonical fixture parameters; binaries and source/gate receipts remain beside
the measurements.
Focused receipt: `.analysis/codex-logs/admission-budget/
single-chunk-profile-summary.json` (table and scope), `single-chunk-profile.log`
and `single-chunk-profile-result.json`.

**Handoff.** The measured K=1 failure triggered committee decision 004 below.
Queue backpressure, mutation-owned hydration/eviction and the pre-publication
fixed-point fence are retained; resumable seams and worker light jobs were not
authorized.

#### Two-phase admission lighting and adaptive batches (2026-10-16, decision 004)

**Measured cause.** The same 2,819-pair release fixture, now with counters,
recorded 119,823,641 channel decreases versus 11,407,725 increases in the
populated world. The old FIFO counted 217,476,661 pops over 59,003,907
per-admission unique cells (3.686 pops/cell). At the original worst chunk
`(33,31)`, it counted 525,776 decreases, 13,726 increases, 77,436 unique cells
and 676,995 pops (8.743/cell); ensure took 40.671 ms and relaxation 40.430 ms
in that counter-instrumented run. Empty-world admissions had no decreases.
Receipts: `.analysis/codex-logs/admission-budget/solver-before-{result,summary}.json`
and `solver-before.log`. These are fresh nonexclusive measurements, not a
claim that instrumentation reproduces the earlier 46.482 ms exactly.

**Solver.** Seam residuals now seed two waves, following mc-world's incremental
edit-light pattern. Removal carries each channel's previous level and clears
cells that depended on that level to their intrinsic source; it does not count
down through every intermediate level. Independent emitters, direct sky and
the existing outer open-sky boundary survive. Only after removal finishes does
addition propagate strict improvements from surviving sources and boundary
cells. The one-chunk halo, mixed/uniform section classifiers and virtual-air
semantics are unchanged. Scratch halo corrections are never published or cached:
only the original resident targets get authoritative light/revision updates.
Each channel with any nonzero seam residual is first reset to its intrinsic
source, even for an apparent increase. Scratch-only halo corrections can leave
neighbouring cached gradients out of sync: an increasing residual may mask lost
old support with another stale-high peer, escaping equality-based removal.
The chained dirty-halo eviction/re-admission oracle test covers this case.
The full-recompute oracle remains the correctness contract, including roofed
neighbour decreases, missing diagonal air, removed emitters and stale resident
light after eviction followed by a different neighbour's admission.

**Tick budget.** One nearest-first batch runs per tick. Its limit is
`clamp(floor(8000 / estimated_microseconds_per_chunk), 1, 32)`.
The estimate starts at 8,000 µs/chunk and uses an EWMA with weight 1/4 for
the latest measured admission cost per chunk, rounded up; timing includes the
synchronous light install/revision fence. Empty batches do not train it.
There is no operator knob, percentage budget or deferred publication.
One chunk remains the progress floor even if it exceeds 8 ms, so measurement,
not the estimator alone, decides whether this implementation meets acceptance.

**Focused after measurement.** The same counter-instrumented release bench
completed all 2,819 pairs with the new solver:

| Populated-world metric | One-phase before | Two-phase after |
|---|---:|---:|
| Ensure p50 / p99 / max | 3.977 / 18.273 / 40.671 ms | 2.663 / 5.575 / 9.281 ms |
| Relaxation p50 / p99 / max | 3.834 / 18.123 / 40.430 ms | 2.502 / 5.387 / 9.055 ms |
| Channel decreases | 119,823,641 | 19,515,928 |
| Channel increases | 11,407,725 | 25,220,688 |
| Queue pops | 217,476,661 | 116,062,113 |
| Per-admission unique cells, summed | 59,003,907 | 57,576,054 |
| Pops / unique cell | 3.686 | 2.016 |
| Original worst chunk `(33,31)` ensure | 40.671 ms | 5.954 ms |
| Original worst chunk queue pops | 676,995 | 125,855 |

Removal avoids repeated count-down but addition restores some conservatively
cleared light, explaining its larger increase count. The empty-world ensure
p50/p99 changed from 1.895/3.027 ms to 2.634/3.713 ms; two phases are not a
universal per-chunk speedup. The new worst populated chunk was `(51,47)` at
9.281 ms, so this instrumented bench alone does not establish the 8 ms gate.
Receipts: `.analysis/codex-logs/admission-budget/solver-corrected-{result,summary}.json`
and `solver-corrected.log`. Subphase counters and benchmark inclusion were removed
before building the replay binary; instrumented sources remain in ignored
`solver-corrected-instrumented/` evidence only.

**Bounded release replay and stop decision.** One authorized MIX-01 replay used
the uninstrumented frozen release binary
`e82b908a8136629eb7b8ac444edb3144a7a6f2d9b61e314e13d5c037a16e5e87`,
the same 100-client/10,000-villager scenario and the same frozen bots as the
before run:

| Login metric | G3 before | Two-phase + adaptive batches | Decision-004 gate |
|---|---:|---:|---|
| `chunks` phase max | 73.30 ms | 12.83 ms | **Fail**, <= 8 ms |
| Tick wall p99 | 84.581 ms | 48.947 ms | Pass, <= 50 ms |
| Readiness elapsed | 41.246668694 s | 40.452425402 s | Pass, <= 41.2 s |
| Burst cohort readiness | 75/75 | 75/75 | Pass |

The replay completed all valid measurement windows without correctness or
infrastructure errors, and both the initial 25 clients and the 75-client burst
cohort completed their schedules with zero disconnects. Login admission batches
reached four chunks, with `ready + pending` capped at 32; the worst measured
admission batch was 12.65 ms for three chunks. An 8 ms EWMA prediction is not a
hard non-preemptive execution bound. Login `chunks` p50/p99 were 6.13/10.78 ms.
Steady/burst/recovery tick p99 were 39.484/80.430/48.227 ms, at
20/19.4545/20 TPS. The overall result remains `PERF_FAIL`: burst TPS and p99 also
miss their thresholds. These short windows are bounded replay evidence, not
stable long-tail estimates.

The subsequent native, windowed `core-client` gate passed against the same
frozen binary, proving its documented server/own-chat subset. Its retained
frame shows lit nearby terrain and trees, but incomplete distant terrain;
`geometry_ready=false` and 800 pending client mesh sections prevent claiming
the separate no-dark/no-missing visual acceptance criterion. A chat smoke pass
must not be reported as that visual gate passing. The current harness selects
v2 through `SOLARIS_SERVER_BINARY`, not the obsolete `--server-variant` flag,
and the existing native fixture environment must be exported.

Receipts: `.analysis/codex-logs/admission-budget/solver-mix/{summary,acceptance}.json`,
the nested MIX-01 `result.json`, and
`.analysis/validation/20261016T045844-core-client-ivwcmbbi/{result.json,screenshots/native-final.png}`.
Host load and the memory-gate wait are recorded in the `solver-window-*.json`
receipts. Because the 8 ms gate still fails, decision 004 stops this work here:
no second MIX replay, no off-thread publication, and no further budget tuning.
Any missing-neighbor-dark semantic change must be separately authorized before
the subsequent resumable tick-owner admission work with commit/revision fences.

### Beds, chests and item prediction

`sleep.rs` handles bed use, sleeping and waking, night skipping and bed
respawn. The bed respawn position is persisted with the player's journal
post-image and checkpoint; a missing bed falls back to natural spawn.
`chests.rs` joins compatible neighboring chests, opens their combined
54-slot storage and separates the surviving half when one is broken.

Every joined player's `UseItem` sequence is acknowledged through
`BlockActionsAcked`, including successful bucket fills/pours and refused
uses, just as for digging and block use. The edge encodes this reliable
change as `BlockChangedAck`; bucket block writes still publish through the
ordinary committed block-change path.

`bottles.rs` owns glass-bottle `UseItem` source-only fluid rays, sharing the
bucket ray and pose-based eye height rather than trusting client prediction.
It exchanges one bottle for a component-bearing water potion without draining
the source, synchronizes inventory custody and drops survival overflow once;
creative input is retained and identical results are not duplicated. Water
cauldrons exchange one bottle and one level, including empty-cauldron filling.
Inventory/block after-images use the ordinary journal and native checkpoint,
and verified fill/empty sounds reuse the direct-holder sound-entity packet.
Bottle and cauldron exchanges award per-item use and the existing vanilla cauldron counters.

`pov_ray.rs` clips both bucket and bottle POV rays against the fingerprinted
26.1.2 `OUTLINE` oracle, not full voxels or movement collision. Filled buckets
ignore fluid shapes and pour relative to the actual hit face; pickups also clip
source-fluid surfaces. The outline blob uses the collision extractor, directory,
binary format and startup identity checks, with native positional selection
offsets applied only where `getShape` uses them. Non-lattice selection bounds
(lectern books) round to 1/4096 block, at most 1/8192 coordinate error. Exact-pose
fire, grass/flower, torch-adjacent, partial-block and shared-bottle regressions
exercise the server decision; no actual-client verification is claimed here.


### Block interaction reach

Dig and use-on-block inputs are gated before emitting intents or opening menus:
player eye to the closest point of the unit block AABB must be strictly inside
4.5 blocks in survival, 5 in creative, plus vanilla's 1-block server buffer.
The 26.1.2 patched sources establish this in `Player.isWithinBlockInteractionRange`,
`Attributes.BLOCK_INTERACTION_RANGE` (4.5), `ServerPlayer`'s creative modifier
(+0.5), and the dig/use packet handlers (buffer 1.0). Reach and bucket rays
share vanilla pose-based eye heights, including ceiling-forced crouching after
the sneak key is released. This also covers block use, farming, and placement;
bucket use already raycasts only 4.5 blocks. Refused actions
still acknowledge their sequence and publish the authoritative clicked block
(and the adjacent predicted placement cell for use-on-block), without effects.
This is input admission, not an anti-cheat subsystem.

### Exact static collision geometry

Startup resolves the embedded vanilla collision shapes into a dense per-state
table of physics boxes. Player admission and entity workers borrow that same
immutable table and emit its boxes without per-tick allocation or conversion.
Empty shapes stay empty; slabs, stairs, doors, and carpets retain their exact
geometry. The sampler advertises the table's maximum box height so fences and
walls retain their 1.5-block collision above the block that owns them.
Missing chunks and unknown states remain conservative full-block obstacles.

### Entity interaction admission

Melee and entity-use inputs share an eye-to-target-AABB closest-point gate,
before swing cooldown, weapon wear, exhaustion, feeding, or shearing effects.
Vanilla 26.1.2 defines entity range 3.0 (`Attributes.java:34-35`), adds 2.0 in
creative (`ServerPlayer.java:219-220,650-655`), and admits packets with a 3.0
buffer (`ServerGamePacketListenerImpl.java:1826,1854-1855`). The strict squared
distance comparison follows `Player.java:2059-2062`; pose eye height uses the
same helper as block reach. Refused inputs produce no interaction effects.
Accepted survival melee wear uses the held-tool damage path, including breakage
at maximum durability and authoritative slot sync; creative attacks do not wear.

### Own-player equipment attributes

After inventory mutations commit, core diffs equipped stacks and publishes changed
attribute bases and complete item modifier ids/operations through the reliable
player outbox. Edge uses the existing protocol UpdateAttributes codec so join,
selection, inventory edits and breakage update the client's cooldown and armor
surfaces, including restoration of bare-hand bases. This mirrors vanilla
`LivingEntity.collectEquipmentChanges` / `handleEquipmentChanges`; attribute
registry ids follow 26.1.2 `Attributes.java` registration order.

### Campfire cooking

Core owns the four independent campfire slots and cooking timers in a focused
module. Food admission uses the shared `campfire_cooking` recipes and actual
stack ingredient semantics; only lit fires with a free slot accept one item,
and survival inventory debit is published through the existing player path.
Each resident tick advances cooking (or cools an unlit fire by two ticks).
Recipe completion pops an ordinary item entity; breaking the fire releases
its uncooked contents. Normal and soul campfires use the same block entity.
Slots and progress are persisted as vanilla `Items`, `CookingTimes`, and
`CookingTotalTimes` NBT in the existing opaque block-entity WAL/checkpoint path.
Insertion and completion publish reliable `ClientboundBlockEntityData` with
the vanilla campfire type and `Items`; chunk publication also carries that NBT.
The join recipe update includes the expanded, deduplicated `campfire_input`
property set: vanilla's client uses it to consume food-on-fire interactions
instead of falling through to `Pass`/ordinary item use.

### World fire and ignition items

`mc-core/fire.rs` owns loaded-block ignition and the native scheduled fire
transition, not living contact damage or burn timers. A dense startup table
resolves fire ages/attachment states, sturdy support, waterlogged fuel and
campfire/candle lighting. `ignite_block` returns true only on a changed cell;
the committing caller carries that cell's post-image. Native use-on input
charges fire-charge count or either-hand flint-and-steel durability only after
successful lighting/priming; creative use does not pay. TNT ignition calls the
TNT owner instead of replacing the block with fire.
Priming retains helper-owned spawn/removal staging for input and scheduled
callers alike. A centralized post-redstone drain commits late block writes in
the same tick; fire does not duplicate TNT publication or consume its staging.

The migrated old random-tick rules retain age increments through fifteen,
water-neighbour extinction and seeded one-neighbour combustion and air spreading.
Fuel membership uses the 207 exact per-block ignite/burn pairs from 26.1.2
`FireBlock.bootStrap`, embedded in `mc-data/fire_flammability.rs`, rather than
name suffixes. Stairs, slabs, fences, carpets, plants and scaffolding participate;
waterlogged states and unregistered blocks (including wooden doors and Nether
wood) do not. The approximate probability rule is unchanged. Normal fire additionally
requires support or adjacent fuel, extinguishes in exposed rain, and retains
eternal netherrack fire; soul fire requires soul-soil/sand support and does not
spread. Roof exposure reads current world columns, not side-propagated sky
light. `fire_spread_radius_around_player` gates aging and spread by a
non-spectator player's strict three-dimensional distance from the integer
block position: zero admits no cells and **-1 removes the player-radius gate**,
including in a world with no players. The radius special case, distance
predicate and 30–39-tick delay were read from `ServerLevel.canSpreadFireAround`,
patched 26.1.2 `ChunkMap.playerIsCloseEnoughTo` and `FireBlock.getFireTickDelay`;
the old age/spread algorithm remains an explicit old-parity rule, not a claim
of exact vanilla probabilistic fire behavior.
Air spread is a separate bounded pass: a loaded, initially-air neighbour needs
adjacent dry fuel, the same player-radius admission and no exposed rain, then
receives both a fire post-image and a scheduled tick. Solid support alone cannot
spread fire; flint-and-steel use over the client QA wool arrangement is covered
through 900 native ticks rather than a command-only or manually dispatched tick.

Fire uses the existing native `block_ticks` queue, its absolute WAL post-images
and native checkpoint; vanilla Anvil `block_ticks` encoding is offline export
only. Loaded due work is dispatched by the shared scheduler; cell/neighbor
edits wake fire without loading absent chunks. Stale block identifiers cannot
resurrect removed fire.
Sibling behavioral regressions cover successful/refused item debit, both-hand
wear/breakage and creative use, age/spread radius boundaries, support/water/rain
extinction, soul/eternal fire, and WAL/checkpoint queue restoration. These are
source regressions until the parent runs the consolidated checks.
The old lava ignition roll and seed-independent B08 fuel admission are also
ported: the native fluid owner invokes `lava_tick` for due lava, publishes a
successful adjacent-air ignition and requeues stable lava with nearby fuel.
Isolated lava gains no perpetual ignition polling; ignited fire uses the same
durable block-tick owner as item ignition.

### Brewing stands

Core owns five shared brewing slots in the ordinary block-entity inventory map
and loaded-tick timers in `brewing.rs`; menus never own batch progress. Menu
type 11 uses bottle slots 0–2 (one item each), ingredient 3 and blaze-powder
fuel 4. The shared 26.1.2 station catalogue supplies potion and container
recipes. One powder supplies twenty charges, one charge starts a 400-tick
batch, and completion transforms all eligible bottles while consuming one
ingredient. Changing ingredient identity or removing all eligible bottles
cancels before the final decrement without refunding the spent fuel.
Dragon breath returns one glass bottle, retaining it in an emptied ingredient
slot or spawning an ordinary item entity when unused ingredients remain.

Generic native clicks, shift-click, cursor return and disconnect/rejoin retain
single custody: closing a view does not refund shared slots. Brewing publishes
menu timer/fuel properties and updates all three `has_bottle_*` block properties.
Bottle-property writes reuse the shared redstone deduplication helper, so the
commit publishes one final state per brewing position rather than duplicates.
Break removal uses the existing atomic container contents/drop/close/tombstone
path, with an opaque NBT tombstone so old timer post-images cannot resurrect.
Persistence uses vanilla `Items`, short `BrewTime` and byte `Fuel` through the
opaque block-entity journal/checkpoint path, plus a namespaced WAL ingredient
fence. Checkpoint capture freezes current timers; converted native imported
bases hydrate slots and timers independently, and opaque publication removes
typed shadows before native capture or offline region serialization. Unloaded
stands pause. Sibling behavioral tests cover recipes, cancellation, remainders,
NBT, native checkpoint and WAL
recovery, shared menu accounting, deduplicated bottle state and break tombstones.
Integrated native tests passed; no real-client verification is claimed.

### Shield blocking

Shield use shares the player's existing transient item-use state for either hand.
The use packet's look direction is committed before starting use, as vanilla's
`handleUseItem` requires; subsequent movement/look inputs update the blocking arc.
After five ticks, melee sources in the front horizontal hemisphere and incoming
projectiles in that arc are removed before hurt invulnerability, victim motion,
health reduction or armor wear. Blocks of at least three damage wear the shield
by `1 + floor(damage)` and publish inventory contents; a broken shield stops use.
Melee attackers receive vanilla 0.5-strength knockback. Player-held disabling
weapons (vanilla axes) stop blocking and publish the shared shield cooldown for
100 ticks, preventing reuse of either hand during that interval.
A shield broken by the hit has already stopped use and does not impose that
cooldown on a replacement shield, matching vanilla's post-wear blocking-item check.

The 26.1.2 source (`LivingEntity.applyItemBlocking`, `BlocksAttacks`, `Items.SHIELD`)
uses shield block/break sounds, not entity status 34 (which is
`STOP_OFFER_FLOWER`); old shield status 29/30 no longer exists. Core emits
`EntityEvent::ShieldBlock`; edge encodes an entity-attached sound with the direct
SoundEvent holder, avoiding numeric sound-registry coupling.

### Sword sweep melee

Charged (`strength > 0.9`), grounded sword hits that are neither critical nor
sprinting, with last accepted horizontal displacement below 0.25 blocks/tick
(`2.5 *` the core's 0.1 movement-speed base), emit ordinary damage intents for
other living entities intersecting the primary target's box inflated
`(1.0, 0.25, 1.0)`. Sweep reach uses strict
feet-to-feet distance below the player's 3-block survival / 5-block creative
range, without the primary packet admission buffer. Self, primary target, item
entities and projectiles are excluded. The existing item-data combat calculation
supplies `1 + level / (level + 1) * strength-scaled weapon damage`, then applies
Sharpness and the attack-strength scale as vanilla does; a full-strength,
unenchanted sweep deals 1.
The embedded item tags include vanilla's seven-entry `minecraft:swords` list
from `data/minecraft/tags/item/swords.json`; sword eligibility is data-driven
in both embedded-data and full-data mode.
Merge admits the primary hit before its secondary hits, so shield blocks and
hurt invulnerability suppress both sweep damage and visuals. Secondary hits
use the same armor, shield, hurt and death pipeline, with 0.4-strength yaw-directed
knockback instead of a sprint/enchantment knockback bonus.

26.1.2 reference: patched `Player.java:1002-1023,1094-1103,1203-1229` defines
eligibility, box inflation, damage, reach, successful-hit knockback and particle
placement. Team friendly-fire rules exclude allied players; marker armor-stand
behavior is not implemented. The movement gate uses accepted client
displacement (`ServerPlayer.java:2374-2376`), not the server's knockback velocity.
`SoundEvents.java:1280` identifies `minecraft:entity.player.attack.sweep`;
`ParticleTypes.java:101` registers `sweep_attack` (id 67). Core emits a sweep
event only when the primary hit lands; edge publishes that entity-attached sound
and the parameterless directional particle at half the attacker's height and
one block along its yaw. `GameProtocols.java:180` gives particle packet 0x2F;
`ClientboundLevelParticlesPacket.java:44-67` specifies its wire order.

### Vanilla player advancements

Core startup loads the locally extracted 26.1.2 advancement tree, including
recipe advancements: 1,617 definitions, 1,492 recipe definitions, 3,546
criteria and 54 trigger types. The extraction tool extends the ignored
`data/vanilla` sidecar; extracted Mojang JSON is not repository source.
Requirements are AND-of-OR groups. Accepted gameplay events, inventory changes
and periodic position checks feed indexed criteria; unsupported fields or
missing authoritative predicate evidence never grant a criterion.

Player criteria timestamps and unknown JSON keys live in the existing native
player record, alongside the unlocked recipe book. Tick and replay player images
share both documents using copy-on-write; full-player WAL entries include either
document only on creation or when its shared identity changes. Omitted fields
retain prior replay state, while legacy complete images remain readable.
Checkpoint/restart and contiguous WAL replay share that state. Offline Anvil
interchange reads/writes vanilla `players/advancements/<uuid>.json` (also
accepting the legacy root `advancements` directory), preserves `DataVersion`
and unknown keys, and keeps orphan files opaque. Recipe advancement rewards
drive recipe-book unlocks instead of granting the entire recipe catalog.
Live unlocks highlight recipe displays and honor recipe `show_notification`;
join replacement never replays unlock toasts.
Experience, recipe and compiled advancement-loot rewards use native custody;
completion chat obeys `show_advancement_messages` (`announceAdvancements`).
Join sends a tree reset and progress, changes send progress deltas, and valid
root tab selection sends the selected-tab packet.

Explicit 26.1.2 gaps (criterion counts, not disjoint advancement totals):
`construct_beacon` 2; `used_totem` 1; `thrown_item_picked_up_by_entity` 1;
`thrown_item_picked_up_by_player` 1; `fall_after_explosion` 1;
`fall_from_height` 1; `nether_travel` 1; `ride_entity_in_lava` 1;
`slide_down_block` 1; `spear_mobs` 1; `target_hit` 1;
`using_item` 3; `player_sheared_equipment` 1. These thirteen trigger types
(sixteen criteria) require absent gameplay or retained origin/thrower evidence.
`impossible` intentionally has no automatic award, as in vanilla.
Seven schema-unsupported criteria across six definitions use entity
`type_specific` (looking-at/lightning fire counts) or location-check offsets.
Five criteria need unavailable structure membership. Twenty-four criteria
need unprojected cat/frog/wolf variant or ominous-banner/name components.
Ordinary eagerly hydrated chest loot has no first-player loot-table provenance;
vehicle lazy-loot opens do. Decorated-pot and special repair crafting paths
lack an accepted recipe identity. Function reward execution remains unsupported
(the vanilla tree contains no function or loot rewards). These gaps fail closed;
operator `/advancement grant` remains an explicit grant, not predicate evidence.

MIX-01 cohort finalization uses startup criterion dispatch rather than evaluating
the entire recipe/advancement trigger list for each arriving player. Mandatory
inventory item/tag, unlocked-recipe ID and location dimension/biome/structure/
position discriminators select candidates; every selected criterion still uses
the same fail-closed evaluator, progress delta, reward and WAL owners. Alternatives
and unconstrained predicates remain candidates, and rebuilding a data-pack tree
rebuilds its dispatch indexes. Periodic location checks always reread live
position, biome and blocks, including stationary block/property/tag changes;
only immutable per-BlockStateId properties/tags are cached. Equipment, effects,
flags and other mutable player evidence are captured live when required.
No player join limit or loading-queue policy is introduced. The 75-arrival,
10k-villager in-process receipt reduced the spike tick from 126.713 to 42.634 ms
(core plus journal capture); periodic advancement cost at 100 players fell from
16.161 to 1.829 ms. This excludes edge fanout and journal writer I/O, not a claim
that other MIX-01 owners meet budget. Receipts are under
`.analysis/codex-logs/cohort-tail/localize/`. Removal path: remove the dispatch
indexes and immutable metadata cache together and retain the unchanged full
evaluator/benchmark as the correctness and A/B oracle.


### Vanilla gameplay commands

The typed v2 command parser and tick executor cover game mode, teleport, give,
clear, kill, time, weather, difficulty, gamerules, summon, block edits, experience,
enchant, spawn points, and announcement/private/emote chat. Selectors support
`@p/@a/@r/@s/@e` with type, distance, limit, sort, name, absolute `x/y/z`
origins and old-supported per-axis `dx/dy/dz` volumes. Named/UUID non-player
targets share the executor, while player-only commands retain their player
filter. Teleport relative coordinates use the executor's origin, not each target's.
Block edits use the existing commit/journal/container paths and reject fills
above `max_block_modifications` (32,768 by default) before mutation. Forced spawn points and world spawn rotation
survive checkpoint/recovery. Command feedback uses vanilla translatable components.

`/advancement grant|revoke <targets>` supports `everything`, `only <id>
[criterion]`, `from <id>`, `through <id>` and `until <id>`. `from` selects the
named node and descendants, `until` its ancestors and itself, and `through`
both, without pulling in ancestor siblings. Unknown definitions and criteria
fail before mutation; player-only targets reuse global dimension routing and
the native advancement engine. The permission-filtered command tree exposes
the vanilla advancement resource-key argument and greedy criterion string.

`command_items.rs` admits native `id[component=SNBT,...]` and `!component`
patches for `/give` and the retained `/debug give` fixture, preserving the
complete stack through inventory splitting, drops, WAL images and native
player checkpoints. `/clear` supports registry-backed item/tag/wildcard
predicates with component equality, presence, negation and alternatives.
Command admission reuses the canonical item NBT codecs and shares the explicit
supported-component table with wire emission; unsupported components and
subpredicates report errors before mutation. Removed components retain vanilla
`!minecraft:name: {}` NBT markers and the exact added-count/removed-count wire
patch layout. The advertised parser IDs remain vanilla `minecraft:item_stack`
(14) and `minecraft:item_predicate` (15); `/item replace` is not advertised.

The command packet uses the exact 26.1.2 parser registry IDs and properties,
executable argument nodes, suggestion providers, and alias redirects. The
enabled vanilla gamerule schema is typed and canonicalized; common simulation
rules gate clocks, drops, spawning, griefing, regeneration, damage, death chat,
inventory/XP retention, and immediate respawn. `players_sleeping_percentage`
uses the old ceiling threshold over active non-spectators (at least one deep
sleeper even at zero); `spawn_monsters` suppresses hostile natural births
without disabling creature spawning or existing-mob despawn.
All canonical overrides, including rules for absent gameplay subsystems, are
stored/queryable and survive absolute WAL post-images plus native checkpoint
recovery. Queries and unchanged writes do not copy the rule map per tick.
Registering a rule does not claim a portal or other absent system.
Player-death XP rewards remain outside the current orb sources.

**Composter use and maturation.**

Native `composter.rs` preserves the old-supported compost chance table and
seeded rolls: the first layer always succeeds, successful fills debit only the
selected hand once, and failed rolls or full composters do not debit. Creative
fills retain their item. Serial uses read the committed overlay, while the final
cell value goes through the canonical block phase for neighbor/comparator
propagation and journaling. Level seven schedules one generic persisted block
tick twenty ticks later; only a still-full composter becomes level eight.
Empty-hand collection resets it to zero and produces one ordinary bone-meal
item entity, honoring `block_drops`. Block state, the scheduled deadline and
inventory debit recover together; a stale maturation cannot resurrect a replaced
block, and a collected checkpoint cannot produce the output again.

Native `weather.rs` retains rain and thunder fade levels independently of the
clear/rain/thunder target. Commands replace the target and remaining duration
without jumping either level; both fades move by the old core's exact `f32`
`0.01` step each tick, even with `advance_weather` frozen. This is the nominal
100-tick ramp (the inherited floating-point accumulation clamps on tick 101).
Snapshots, including login, expose the actual retained levels. The edge sends
ordinary level changes first, then the old 20% crossing START/STOP event and
duplicate full-level sync; fade ticks do not repeatedly restart rain.
The single duration clock and clear/rain natural alternation preserve the old
core's scope: thunder remains command-controlled, not a new independent natural
thunder scheduler. There is no natural lightning/ignition scheduler (old-core
parity matrix row 186); item/channeling lightning is a separate projectile path.
Absolute climate post-images carry the exact level bits, kind, remaining duration,
`day_time` and effective time/weather clock controls through the WAL and the
atomic native checkpoint. Recovery folds the journal tail over that captured
image rather than regenerating the clock from the simulation tick.

`/team` and `/scoreboard` support dummy, death-count, player-kill-count and health
criteria, styled team chat and viewer synchronization. Team friendly-fire gates
player melee, sweep and arrows. Vanilla `scoreboard.dat` is a gzip-NBT checkpoint
domain with absolute WAL postimages; joining players receive its current state.
The scoreboard packets follow 26.1.2 IDs `4F/62/6A/6D/6E`.

### Player statistics

Core owns the complete 26.1.2 custom-stat registry and the mined, crafted, used,
broken, picked-up, dropped, killed and killed-by counters. Successful gameplay
outcomes award counts; rejected intents and clipped movement do not award predicted
distance. Movement rounds accepted distance to integer centimeters. Time counters
use committed-tick anchors rather than requiring statistics-only WAL patches per
idle tick. Warden-warning cooldown/decay likewise derives from a committed-tick
anchor: idle healthy survival players emit no Player or Statistics postimages.
Shrieks and connection/activity transitions still journal their durable anchors;
checkpoint/replay derive exact vanilla counters and offline time remains frozen.
Anvil export materializes `warden_spawn_tracker` and omits native anchor metadata.
Living kill credit uses the existing recent-player-hit window; exploding creepers
and projectile blasts capture their living source type before the source leaves.
Ownerless projectiles and nonliving blast sources do not become killed-by types.
Statistics-only changes use compact absolute counter patches in the existing tick
WAL; checkpoints retain statistics inside the native player record. Offline
conversion consumes both historical `stats/<uuid>.json` and canonical
`players/stats/<uuid>.json`, exports only the latter, and preserves unknown JSON
statistics, envelope fields and `DataVersion` semantically. Native statistics
metadata never enters exported vanilla player NBT.

`REQUEST_STATS` emits all nonzero known counters through vanilla AwardStats
(clientbound id `0x03`; request is ClientCommand `0x0c`, action `1`), with the
VarInt map size followed by type/value/count VarInts verified by local javap.
The existing scoreboard has no incremental statistic-criteria award hook;
statistic-based objectives remain unsupported. Cake eating, bell ringing,
cartography menus and item-on-stick boosting have no existing authoritative
gameplay outcome to instrument, so these counters are registered but those
mechanics are not implemented by this change.

### Experience orbs

Core v2 spawns `minecraft:experience_orb` entities for player-attributed mob
kills (including vanilla's 100-tick kill-credit window), correctly harvested
coal/diamond/emerald/lapis/redstone/quartz and sculk blocks without Silk Touch,
furnace result extraction, breeding, fishing catches and experience bottles.
Furnace extraction no longer credits XP directly.

Orbs use vanilla value buckets, gravity and collision, nearest-player attraction
within eight blocks, same-denomination/40-group local merging, a two-tick player
pickup cooldown, and expiry at age 6000. Mending repairs randomly selected
damaged equipped items before the remainder reaches the XP bar. The edge sends
AddEntity, the 26.1.2 orb-value metadata (index 8), TakeItemEntity on each collected
unit, and SetExperience. Absolute journal images preserve merged counts; vanilla
entity-region checkpoints preserve `Value`, `Count`, `Age`, position and motion.

### Enchanting and repair workstations

Enchanting tables, anvils and grindstones use the ordinary owned menu/click
framework. Their input slots belong to the player menu, not shared block
storage; closing or checkpointing preserves custody without retaining preview
outputs. Enchanting offers use the player's durable vanilla `XpSeed`, the
two-height outer bookshelf ring and transmitter gaps, Java legacy randomness,
and startup-compiled enchantment JSON/tag eligibility, weights and costs.
The edge routes offer buttons and anvil rename text to the tick; menu data
properties carry table costs/seed/clues and the anvil cost.

Native disconnect closes the live menu before capturing the offline player
image. Real station inputs and the cursor return to the inventory, overflow
becomes journaled item entities, and preview results never become custody.
Repeated Leave and rejoin cannot refund those transient slots a second time.

Successful enchanting debits lapis and offer levels together and refreshes the
seed. Anvil extraction applies material/sacrifice repair, compatible enchant
merges, prior-work and rename costs, the survival 40-level ceiling, and a 12%
block-damage chance. Grindstones preserve curses, repair compatible pairs,
reset prior work and spawn refundable XP through the shared experience-orb
owner, so Mending and pickup use the same path as other XP sources.

Smithing, loom and stonecutter menus also own their inputs in the player menu.
Native block use opens them, and the existing edge container-button/click
route selects filtered stonecutting offers or loom patterns. Smithing uses
the shared 26.1.2 recipe/tag catalogue, copies the base stack's components
through netherite upgrades or trim replacement, and refuses identical trims.
Loom appends the selected pattern/dye color up to six layers, consumes banner
and dye, and retains the reusable pattern item. Stonecutter selection follows
the advertised filtered recipe order and resets when the input item changes.
Output projections are read-only: successful extraction atomically debits the
corresponding inputs, shift-click repeats complete crafts only while the
entire next output fits, and close/disconnect return inputs and cursor once
without returning the virtual result. The local 26.1.2 MenuType javap oracle
fixes smithing/loom/stonecutter IDs at 21/18/24 and corrects the legacy native
grindstone mismatch to 15. Hopper/dispenser storage menus use IDs 16/6 and
five/nine persisted block-storage slots rather than player-local inputs.

### Contact hazards and special consumables

The native contact owner samples bounded living bodies against the resident
world and the exact startup suffocation table. Suffocation, cactus, moving
grown sweet-berry bushes, magma support and lit campfires emit typed damage
intents through the ordinary armor, effect, hurt-window and death pipeline.
Sneaking exempts magma/campfire contact, Frost Walker exempts magma, and the
fire-damage rule gates all fire-family hits. Lava/fire extend a retained burn
timer; water clears it and the shared burning flag. `Fire` short NBT and
absolute player/entity WAL images preserve remaining burn time across restart.
Accepted flaming projectile hits ignite only after shield, PvP and hurt
admission; disabled PvP rejects attributed player hits before health, wear,
knockback or ignition.

Milk clears effects, honey clears only poison and returns one glass bottle,
and ominous bottles apply component-selected Bad Omen. Completed chorus fruit
consumption resolves up to sixteen native-world teleport candidates within
eight blocks, requiring loaded supporting terrain and an unobstructed,
liquid-free player body. Success updates native position, clears fall distance
and reliably corrects the client; failure still consumes the fruit. Generic
item cooldowns are player-local transient state, tick once with item use and
publish the existing exact 26.1.2 identifier/duration cooldown packet. Chorus
fruit imposes twenty ticks even when no safe teleport destination exists;
cooldowns reset on reconnect, matching the old connection-local owner.

### Living status effects

Core v2 owns active effects in `effects.rs`: player-local state and cold mob rows
carry vanilla registry IDs, amplifier, finite/infinite duration, ambient/particle/
icon flags, hidden weaker effects and remaining absorption. Tick transitions
cover regeneration, poison's one-HP floor, wither, instant healing/harm, hunger
and saturation. Existing attribute synchronization carries movement/attack
modifiers; mining progress and melee use the same effect state. Resistance,
fire resistance, water breathing, absorption and safe falling affect survival;
levitation/slow falling also affect basic mob physics. Client effect packets
supply jump boost, vision and ongoing player movement/mining prediction.

Drinking potion contents (base and custom effects), golden/enchanted apples,
suspicious stew and milk use the ordinary item-use/remainder path. `/effect
give|clear` uses the shared selector/permission path and vanilla argument syntax.
Cave-spider accepted melee supplies its native difficulty-gated Poison rule.
Self update/remove packets, absorption metadata and other
viewers' effect particles/ambient/invisibility/glowing use 26.1.2 layouts.
Vanilla `active_effects`/`AbsorptionAmount` and absolute journal postimages persist
players and mobs, including hidden effects, through checkpoint and replay.

Player air supply uses the same authoritative vitals state as drowning damage.
Each depletion/recovery change sends reliable own-entity integer air metadata,
independent of health changes and world-stream credit, so bubbles can warn
before damage begins. Join, respawn and dimension transfer resynchronize the
current supply; unchanged air emits nothing and does not clone appearance rows.

Wave 3 admits mace smash only above 1.5 fallen blocks outside gliding, adds
tiered fall damage and Density, and applies Breach before armor enchantment
protection. Accepted hits reset falling, push nearby living bodies, and Wind
Burst supplies its level-scaled bounce; refused hits do none of these.
The next-landing guard uses vanilla's nullable double-list
`current_explosion_impact_pos` in player `.dat`, WAL images and rejoin hydration
(26.1.2 `LivingEntity`/`Vec3` javap), rather than a restart-unsafe flag.
Bogged arrows retain custom Poison for 100 ticks. Oozing deaths birth two
medium slimes, Infested admitted hits have a 10% chance of 1–2 silverfish,
Weaving deaths place supported cobwebs subject to mob griefing, and Wind
Charged deaths emit a non-damaging burst. Births, removal and cobweb writes use
the ordinary same-batch post-images; all four potions use the existing
canonical awkward-potion brewing recipes.


### Command delivery and administrative lifecycle

The v2 edge publishes the permission-filtered vanilla command tree at Play
entry and republishes it after `/op` or `/deop`. Serverbound suggestion requests
are decoded off-tick; the same tree metadata supplies permitted roots,
selectors, online names, and startup registry identifiers. Execution checks
the current persisted operator level rather than trusting the client tree.

Server-authoritative position syncs are fenced on the core tick when created,
not when the off-tick edge writes them. The connection-local teleport sequence
starts at 1 on join, advances for every correction, command teleport, respawn
and dimension transfer, and follows local 26.1.2
`ServerGamePacketListenerImpl.teleport` bytecode's wrap to 0 before `INT_MAX`.
Only the matching confirmation clears the newest pending sync; stale confirms
and movement while pending cannot overwrite its authoritative pose. The edge
preserves core-assigned ids through batching/replay. Rotation/status-only input
resolves absent coordinates against core state rather than a pre-teleport
decoder cache.

Vanilla-format `ops.json`, `banned-players.json`, and `banned-ips.json` reside
beside the explicit server config (otherwise in the server working directory),
separately from world checkpoint domains.
Mutations write a temporary file, fsync, rename, and fsync the directory before
publishing the new in-memory list. Login admission follows authentication but
precedes `LoginSuccess`; UUID/IP bans send the vanilla translated disconnect
component, and operator `bypassesPlayerLimit` controls full-server admission.

`/save-all [flush]` requests a tick-owned capture through a control channel.
The journal's checkpoint writer acknowledges only after the all-domain
Immediate native transaction is durable; the edge reports success afterward.
The tick performs no direct disk I/O, but it may block on journal queue
backpressure while the queue is full. `/stop` closes admission, reliably sends the
vanilla shutdown disconnect to live players, drains connection tasks, and
invokes the existing core shutdown path (final checkpoint and journal join).

### Primed TNT

Native `tnt.rs` owns successful priming, ballistic motion, retained fuse and
explosion intents. Block priming replaces only a resident TNT block after an
entity is created; the overlay changes immediately to reject duplicate ignition.
Item priming never removes a block, and dispenser/item owners debit only on
success. Scheduled fire and dispenser air post-images join the current tick's
canonical block writes before neighbor/fluid scheduling and journal capture.
Powered placement/neighbor changes and newly resident powered TNT use the
existing redstone event/hydration scan, without making TNT a dust connector.

The 26.1.2 local `PrimedTnt`/`Entity` bytecode fixes metadata indices 8/9
(integer fuse/block state), 80-tick default fuse, 0.04 gravity, 0.98 drag,
0.7 ground friction, launch impulse and ground bounce. Generic living motion,
damage, health metadata and death loot do not own TNT. Fuse expiration removes
the entity exactly once and emits the existing native explosion intent. Blasted
TNT primes with a 10–29 tick chain fuse instead of producing TNT loot;
`tnt_explodes` gates priming and expiration, while `tnt_explosion_drop_decay`
selects radius-dependent block loot decay independently of mob explosions.

Native entity-region checkpoints retain lowercase `fuse` (short), `block_state`
(vanilla named block/property compound) and nondefault `explosion_power`.
Absolute journal post-images advance fuse, position and velocity together,
so restart/replay cannot reset a fuse or rewind its explosion location.

### Taming, leashes and animal mounts

Native `taming.rs`, `leash.rs` and `mounts.rs` own pet UUID ownership, sitting,
collar dyes, ocelot trust, inherited fox trust, horse temperament, leashes,
name tags and animal riding. Their retained state extends the existing animal
lifecycle image rather than introducing a second entity/session owner. Tame
roll counters survive checkpoint and WAL replay, so reconnects cannot reset
the roll stream. Sitting and riding suppress autonomous decisions without
disabling gravity; leashed animals follow a live holder and release one lead
on detach, holder loss, death or the twelve-block break distance.
An ordinary interaction by the current UUID holder also detaches without
secondary-use/sneaking, including an empty hand. Exact-stack merge validation
drops one lead once; the resulting empty holder publishes reliable SetEntityLink.
The embedded bootstrap registry includes the canonical animal food tags as
well as the full-data registry; parrot taming and food-gated riding therefore
do not depend on an external vanilla-data directory.

Interactions enter the ordinary exact-stack intent/merge path. Saddle/body
equipment, wolf-armor repair and shears removal share the existing retained
equipment and death-loot owner; a rejected or repeated prediction cannot
spend the last input twice or recreate removed equipment. Fence anchors use
native leash-knot entities and retain the 26.1.2 `leash` codec's UUID compound
or three-integer block position. Owner, collar, temperament, love cause and
trust use vanilla entity keys; native roll/rider extensions remain namespaced.
Knot entities restore stationary native movement and vanilla's attached render
offset; their add-entity packets publish integer fence coordinates rather than
the render position.
Malformed present fields and duplicate passenger custody fail closed with
field/UUID-specific storage diagnostics.

The edge retains secondary entity use, full PlayerInput flags and MoveVehicle.
Only eligible saddled mounts admit client vehicle movement: horse-family
control requires taming, llamas cannot steer, and pigs/striders require the
correct item-on-stick in either hand. Native collision resolution commits the
mount pose and rider snap together and reliably sends the exact vehicle
correction packet. Sneaking and teleports dismount. A restart-local reverse
link avoids entity scans for each rider movement and is rebuilt on join.
The movement owner admits at most one displacement/correction budget per rider
per tick. Dismount, auto-eject, death and respawn clear both links and steering
bits; input from an unmounted player cannot steer a later mount.
Fence leashes share one reusable tick-local knot index rather than scanning the
population separately for every animal and knot.

Snapshot appearance carries passengers and leash holders, including explicit
empty-link updates, so skipped publications and late pairing converge through
SetPassengers/SetEntityLeash. Pet owner references use the protocol's optional
living-entity-reference serializer; animal body/saddle equipment uses slots
6/7. Boats and minecarts reuse these passenger/input links under their native
vehicle owner.

### Boats, rafts and rail vehicles

`vehicles/` owns non-living boat/raft and minecart bodies on the serial native
tick owner; the parallel living motion/drowning pipeline does not advance them.
All 26.1.2 wood boat/chest-boat variants and bamboo rafts place from their own
items; minecart, chest, hopper, furnace and TNT carts place only on rails.
Placement checks block shapes and entity occupancy before consuming one item.
Mounts owns passenger admission, input and rider synchronization; boat steering
and paddles use that input, while minecarts follow all ten rail shapes, slopes,
curves, powered braking/acceleration and activator behavior. Collision-only
movement uses the shared shape kernel, not client vehicle coordinates.

Chest boats/rafts and chest carts use vanilla 27-slot chest menus (an intentional
extension over OLD's chest boats without container behavior); hopper carts use
five slots. Menu clicks and hopper transfers share native item/component rules.
Every debit and credit stays on one custody owner; full item suction and expired
TNT bodies defer removal until merge commit, so swap-removal cannot invalidate
staged entity slots. Container destruction takes contents once before emitting
variant-item and contents drops. Furnace fuel/push, hopper enable/cooldown and
TNT fuse/power are retained rather than reconstructed after restart.
If `tnt_explodes` is disabled while a cart is primed, fuse expiry defuses the
live cart without deleting its custody or marking its death handled.


Vehicle state, inventory and position/velocity are checkpointed and journaled
together each tick, including unchanged controls with changing motion. Native
26.1.2 `Items`/`Slot`, `Fuel`, `PushX`/`PushZ`, `fuse`, `explosion_power`,
`explosion_speed_factor`, `Enabled`, `LootTable` and `LootTableSeed` types follow
the local vanilla entity codec/javap evidence. Unopened mineshaft carts retain
their seeded loot until open, hopper access or destruction; supported production
chest tables reuse the existing chest loot compiler. Invalid item custody,
variant/container mismatches and unsupported saved loot tables fail recovery
with an exact diagnostic rather than silently dropping an entity-region record.
Boat paddle metadata, furnace fueled metadata and TNT status 10 use the local
26.1.2 serializer/index evidence. Physics is wiki-level, not vanilla-order exact;
command-block minecarts are outside this slice.

### Wave-2 partial parity closures

Generated village markers initialize the same age/profession schedule and
merchant catalog as native villagers; babies do not open trade menus. Contextual
death loot uses retained baby/fire/sheep flags, and guaranteed animal equipment
drops respect Vanishing. Falling-block items obey `entity_drops`, independently
of `block_drops`. The existing OLD-supported boolean/integer gamerules retain
their canonical 26.1.2 names and aliases; registration alone is not a claim of
an absent patrol, insomnia, raid, spawner or administration subsystem.

`command_border.rs` owns finite border center/size, native checkpoint/WAL
post-images and offline `world_border.dat` export, plus exact initialization packets;
deferred reliable publication does not advance a viewer's baseline. `/time query`
distinguishes day, daytime and gametime. Bounded SNBT summon preparation
validates supported native fields before
allocating an entity: malformed values or duplicate UUIDs create nothing.
`entity_controls.rs` retains NoAI, Invulnerable and FallDistance. NoAI suppresses
autonomous goals, not physics; Void bypasses Invulnerable. Summon animal owner,
trust, leash and saddle fields reuse the native animal codec.
Summon accepts native `PersistenceRequired`, literal `CustomName` text components,
villager and tadpole `Age`, and zombie-villager `IsBaby` through their existing
retained owners. Fields without v2 authority (name visibility, silence, gravity
override, intrinsic glowing, command tags and handedness), unsupported species
fields and styled names fail before allocation with a named sender-chat error.

Everyday equipment enchantments share the ordinary item/intent/damage owners:
Unbreaking resolves independent durability points with the armor variant and an
already-retained player XpSeed; Smite/Bane/Impaling use the authored sensitivity
tags. Bane applies Slowness IV and Fire Aspect ignites only an admitted hit,
using the pre-wear swing stack. Thorns stages non-recursive reflected damage
inside that same merge/death commit. Respiration skips air consumption rather
than recovering air; Aqua Affinity removes only submerged mining's penalty.
Depth Strider, Swift Sneak and terrain-gated Soul Speed publish equipped-slot
attributes. Soul Speed boot wear uses Unbreaking. Frost Walker freezes exposed
source water via canonical block intents and ages/melts frosted ice through the
existing checkpoint/WAL-backed block-tick queue. Binding fences armor removal
clicks outside creative; Vanishing filters inventory and closing-menu death drops
without affecting keep-inventory custody.

Existing native paths continue to supply Sharpness, Knockback, Looting, Fortune,
Silk Touch, Efficiency, Protection/Fire/Blast/Projectile Protection, Feather
Falling, Power/Punch/Flame/Infinity, Loyalty/Riptide/Channeling,
Multishot/Piercing/Quick Charge, Lure/Luck of the Sea and XP Mending. Fishing rolls
also include active Luck minus Unluck. The embedded bootstrap includes the same
canonical sensitivity and Soul Speed block tags, rather than disabling these
hooks when a full content sidecar is absent.
Canonical embedded ore and common Silk Touch block tables use the same vanilla
loot compiler as full sidecars, retaining ore-specific Fortune formulas, Silk
alternatives, random sequences and explosion behavior instead of flat drops.

Production biome spawn-table coverage exercises all authored entries against
registered blueprints; the table's spawn category is authoritative, including
ocelots authored in a category different from their entity classification.
Misc/non-spawning blueprints remain ineligible, so a projectile entry cannot
become a creature herd. Guardian selection coverage reads the canonical monument
override separately: guardians are not authored in biome spawn tables. This is
entry/blueprint coverage, not a claim of an additional monument spawn scheduler.
Natural spawning still uses native habitat/light/observer admission. Dolphin's
Grace, Conduit Power and Hero of the Village were assigned to the later parity
wave, not certified by the wave-2 spawn coverage.

### Native dimensions (wave 2: D1–D9)

`dimensions.rs` closes the registry to the canonical Overworld, Nether and End.
One native tick owner advances three isolated `TickState`/`ChunkWorld` instances;
the child worlds do not own sessions or separate WAL streams. Overworld uses
-64..320 geometry; Nether/End use 0..256. Runtime geometry is bound to the
native DB's required dimension records. Minecraft 26.1 Anvil conversion stores
**all** dimensions, including Overworld, under
`dimensions/minecraft/{overworld,the_nether,the_end}/`; `region/`, `entities/`,
`poi/` and dimension-scoped `data/` paths belong to this offline layout, not
runtime storage. The exported End fight record belongs at
`the_end/data/minecraft/ender_dragon_fight.dat`. Exported `level.dat`,
`players/data/`, `players/advancements/`, `players/stats/` and global saved
`data/` remain at the world root. Legacy `playerdata/`, root `region/`, `DIM-1`
and `DIM1` layouts are rejected by import rather than inferred or generated over.

The converter uses 26.1 `Data.spawn` RespawnData (`dimension`, `pos` integer
array, `yaw`, `pitch`) and lowercase player `respawn` with optional `forced`,
never legacy SpawnX/SpawnDimension or Solaris respawn aliases. Native checkpoint
state is authoritative; offline export derives `level.dat` (`Time`, `DataVersion`,
`Version`, `version`, modern `spawn`) from recovered native state and retained
templates. Vanilla 26.1 climate, game rules and world generation use
`data/minecraft/{weather,world_clocks,game_rules,world_gen_settings}.dat`, not
obsolete level.dat fields. Rules encode typed boolean/integer values; clocks
encode `total_ticks`; generated export worldgen settings use the normal vanilla
dimension preset and native seed. Imported worldgen settings and safe unowned
metadata survive in native records and are re-exported. Scoreboard export uses
`data/minecraft/scoreboard.dat`. Modern `Data.difficulty_settings.difficulty`
maps to native global difficulty while preserving `hardcore`, `locked` and
unowned settings. No checkpoint publishes these filesystem projections.
Vanilla's lazily created zero-byte region files represent empty regions;
nonempty files with incomplete headers still fail closed as truncated data.
Native chunk decoders require the dimension's authoritative geometry, never
infer it from serialized section count. Light-only sections exactly below/above
the block range are retained and re-emitted; any other out-of-range section or
an out-of-range block/biome payload fails closed.

`tools/harness/vanilla_world_roundtrip.py --eula-file <already-accepted-eula.txt>`
drives the ignored Core integration test through a real Java-created world,
explicit import into native storage, native fixture creation and Anvil export
into `server/world`, a real vanilla 26.1.2 dedicated-server `nogui`
open/check/mutate/save/stop, and fresh import into a separate native directory.
Verification reads native state and
re-exports the Java-changed import. Preserve the original 18 roundtrip checks,
Python TEST/environment/phase markers and real-Java assertions: marker blocks
in all dimensions, a named/moved entity, spawn, clock, gamerules, scoreboard
and offline player item/respawn custody, including vanilla-changed values.
This is a headless interoperability gate, not graphical-client evidence or
automatic EULA acceptance. `result.json` and complete v2/vanilla logs remain
under `.analysis/vanilla-world-roundtrip/`.

The existing Nether/End generators are active native chunk sources.
They remain single-biome terrain routers, not a claim of fortress/end-city or
multi-biome generation. Native schema 3 binds the closed dimension contract
(seed, generator/revision and geometry) inside the required DB records.
Unsupported historical native schemas fail closed; they are never relabelled.

The publication boundary performs explicit dimension-qualified command travel
by moving complete entity/player custody, preserving UUID and globally unique
wire IDs. Chunk caches, viewer entity sets, lighting and weather are dimension
qualified. Login announces all three worlds using the configured vanilla 26.1
dimension-type registry. Travel uses native Respawn retention bits 3; death uses
0, with a complete viewer reset. Sea levels are Overworld 63, Nether 32, End 0.
Beds and personal points retain their own dimension; invalid non-Overworld
points return to the Overworld, while unloaded respawn areas defer admission.

Nether portal rectangles are native committed block state, with optional corners,
2..21-block interiors and 3..21-block heights on either horizontal axis.
Ignition items use the existing success-only durability/count owner; ambient
fire also activates valid frames. Frame damage collapses the connected portal
in the same block/WAL batch. Destination links derive from current admitted
portal blocks (16-block Nether / 128-block Overworld search, 8:1 scaling), so
frame removal and restart cannot resurrect a stale link. If no link exists,
the destination creates a supported portal, including the forced obsidian
platform in unsuitable terrain. Missing search chunks postpone custody transfer;
portal-only load tickets are derived and never journaled as permanent tickets.
Players retain the creative/default portal-delay gamerules, four-tick timer
decay, and 10-tick cooldown; other entities retain a 300-tick cooldown. Standing
inside a portal refreshes cooldown rather than bouncing between worlds.
The timers use vanilla `PortalCooldown` plus a Solaris retained elapsed image
in player/entity NBT and absolute WAL records.
Riding and ridden entities are currently excluded from portal admission; atomic
passenger-graph transfer remains a gameplay gap.

Eyes of Ender debit through the native use-on path and require all twelve
inward-facing frames before creating the nine portal cells. End portal contact
uses the native Y=6/16..12/16 inside shape. End entry recreates
the 5x5 obsidian platform at Y=48 and clears Y=49..51; players arrive at Y=49,
other entities at Y=50. Exit portals use the personal bed/forced spawn validator
without copying player custody. Gateway pairs retain vanilla `Age`,
`ExactTeleport`, and **`exit_portal` as a three-int array**, plus an absolute
Solaris cooldown deadline in the existing opaque block-entity WAL/checkpoint
path. Gateway teleport locks for 40 ticks; resident age advances in memory and
is materialized at checkpoint, while links, cooldowns and beam transitions are
journaled. The native 2400-tick beam event remains tick-driven. Construction
spills container contents and records removal before final replacement storage
in the same WAL batch. Opaque owners supply their latest timer post-images;
chunk-cache presence fences removals but never replaces newer owner timers with
older checkpoint/wire images. Invalid loaded gateway fields refuse recovery;
later travel refuses malformed fields with dimension and exact block coordinates,
rather than inventing a destination.
These field names, platform offsets, post-increment timer threshold, and
10/300 cooldowns were read with `javap -c -p` from local 26.1.2
`NetherPortalBlock`, `PortalProcessor`, `Player`, `Entity`, `EndPortalBlock`,
`EndPlatformFeature`, `TheEndGatewayBlockEntity`, and `BlockPos` in
`.analysis/decompile/client.jar`. The fight owner invokes the shared idempotent
exit-fountain and generated-gateway block APIs.

`end_fight.rs` owns D9 authority in the End world: one retained dragon UUID,
eight derived multipart wire IDs, the vanilla air steering kernel and bent
flight-history hitboxes, circling/strafe/charge/landing/perch transitions, and
owned dragon-fireball/cloud attacks. Ten seeded obsidian spikes include the
two vanilla iron-bar cages; their crystals heal, explode and retain beam and
invulnerability state. The reliable pink boss bar plays boss music and fog
through the 200-tick death animation. First/later kills award 12000/500 XP,
activate the shared exit fountain, place the first egg and consume one of the
twenty shuffled gateway indices. Four cardinal crystals start the retained
start/preparation/pillar/dragon respawn stages; losing a ritual crystal aborts
and restores the exit portal. Phase targets, flight samples, damage thresholds,
crystal/perch-cloud membership and award/stage cursors ride End-enveloped WAL
images and the all-world native checkpoint. The converter emits the measured
26.1.2 `data/minecraft/ender_dragon_fight.dat` fields and vanilla-readable
namespaced extension on offline export. Native records preserve exact tick
authority; corrupt fields, contradictory projections, missing crystal/dragon
records and multipart ID collisions fail startup closed.

Player NBT records canonical `Dimension` and `respawn.dimension`; child captures
and dimension-enveloped journal entries share one tick and one all-world native
transaction. Global player UUID custody resolves across the whole WAL, not
independently per dimension. Retained entity/player corruption fails startup
closed with record diagnostics. Offline import assigns globally unique positive
wire IDs when absent, reserving multipart children and retaining the high-water
mark without rewriting source entity regions. Interrupted staging publishes no
destination and cannot resume as a runtime base. Any WAL without its required
native DB refuses recovery; complete native bases, including tick zero, require
positive Solaris wire IDs.
Registered type validation uses the complete entity-type contract, not the mob
AI blueprint subset; non-mob controllers restore canonical physical dimensions.
Scalar health is authoritative for restored wire appearance, including WAL-only
vitals updates.

Global player identity, roster, chat, scoreboards, gamerules, difficulty and
world border remain single-owner authorities. Unrestricted player names/UUIDs
and global selectors route command mutations to the owning world, applying a
selector's sort/limit once; spatial entity selectors remain dimension local.
Duplicate UUID login rejects the new connection before allocation, leaving the
existing session and its inventory unchanged. Weather has only an Overworld
authority, including commands issued in Nether/End; the shared day clock
advances once, not once per world.
Summon UUID admission covers every resident world, disconnected player custody,
pending joins and successful same-tick claims before assigning a new identity.
Shared difficulty is frozen into the checkpoint and folded from absolute WAL
post-images, so subsequent login/query responses use the recovered value.
Root and child native checkpoint images retain their durable API ticket
post-images; an empty WAL tail after prune cannot discard forced loading. Derived
`solaris:portal:` tickets remain restart-local and are rebuilt by portal owners.


Native spawning uses local geometry, biome tables, observer caps and despawn
state. The embedded bootstrap contains canonical spawners for all five Nether
and five End biomes, including striders, and the same charge/energy budgets as
full sidecars. Spawn potential is derived per world with the population census
and updated after each accepted pack. Nether monster light rules ignore sky
light, use the native 128-block spawning ceiling and retain Ghast/Magma Cube
exceptions; End monster block-light admission is at most 7.

Wrong-dimension beds and charged anchors remove their source blocks before a
power-5 environmental blast, without source-item loot. Anchor charging claims
one glowstone through native merge; admitted safe Nether respawn consumes one
charge only after the required area is loaded. Nether bucket water evaporates
without placing or waterlogging, except that cauldrons retain water. Nether lava
uses delay 10 and horizontal level drop 1 (Overworld/End: 30 and 2). Void damage
uses each world's minimum Y, with outside-height collision treated as known air
only for a loaded column.

The dimension, portal/gateway (D4/D5), and dragon-fight (D9) owners above cover
their native runtime rows. They do not certify every Nether/End gameplay system.

`conduit.rs` retains the vanilla `Target` UUID in chunk block-entity NBT;
40-tick scans require the water-filled 3×3×3 centre and 16–42 prismarine-family
frame cells, grant 260-tick Conduit Power over 32–96 blocks, and attack a nearby
wet hostile for four magic damage only at a full frame. Clients derive activation
from the same block frame; target changes use native type-26 BE updates. Only
durable target changes enter opaque WAL post-images, which checkpoint verbatim.
`dolphin.rs` grants 100-tick Dolphin's Grace to nearby swimming players via native
effect packets (0.96 water drag, fivefold baseline steady swim speed), follows
swimmers, surfaces for air, plays with existing floating items, and claims fish
atomically. Its `GotFish` state and air survive the entity WAL/native checkpoint;
treasure leading and structure generation are intentionally outside this slice.

`beacon.rs` scans one to four complete mixed mineral-block pyramid layers and
the beam's sky column every 80 ticks. Its one-slot payment menu admits iron,
gold, emerald, diamond or netherite; accepted level-gated selections consume one
payment and journal vanilla `primary_effect` / `secondary_effect` block-entity
fields through the conduit-style opaque post-image path. Nearby players receive
ambient status effects over a 20–50-block horizontal radius, extended upward,
for 220–340 ticks; level four enables regeneration or the primary's level II.
The vanilla client derives its beam from the world and beacon block entity.

Stored chunk admission fails closed at `ChunkSource`: only genuinely absent
native base/gameplay coverage reaches worldgen, including at sparse signed
coordinates; imported all-air coverage never regenerates. Decode and validation
failures identify DB/domain/key, chunk coordinates and reason. Recovery loads
WAL-targeted bases and fails if corrupt; runtime loads remain unavailable and
report the failure once. Checkpoint capture retains unapplied WAL-backed work
and refuses to overwrite a failed chunk or prune its journal when selected for
pending overlays, scheduled ticks or custody. An unrelated failed chunk does
not block a checkpoint that does not select it. Admission never substitutes
terrain for failed reads or scans unrelated stored chunks.

`copper.rs` shares the native 27/54-slot chest authority across all eight copper
chest variants, including comparator output, paired weathering, wax and axe
scraping without inventory replacement. Comparator updates reconcile cached
output with the powered property even when signal strength is unchanged.
`copper_golem.rs` consumes the vanilla
copper/pumpkin pattern into a matching chest plus one persistent golem, moves
up to sixteen items between copper and matching/empty ordinary chests, and
oxidizes into a facing/pose-preserving statue with native custom-name components;
the final axe scrape revives an unwaxed unaffected statue with its name/facing.
Carried items and the shearable poppy antenna use native equipment custody;
cargo retains the guaranteed main-hand drop chance through WAL/native restore
until fully deposited or returned, including deaths without player attribution.
Weathering saves the verified `weather_state` string and `next_weather_age` long.
Slot/entity/opaque absolute WAL images and native checkpoints retain these
states; chest metadata is preserved and supported imported loot materializes
once. Sorting movement/weather timing are approximate, not vanilla AI parity.

`trial_spawner.rs` owns UUID-based player registration and live-mob caps, normal/
ominous configurations, staged key/consumable ejection and absolute cooldowns;
`vault.rs` atomically consumes the appropriate key once per rewarded player and
retains its timed reward queue. Both use verified 26.1.2 block properties and
block-entity fields, native type-44/45 updates and the opaque WAL/native
post-image path; malformed UUIDs, timers, items and configs fail admission.
Invalid records are typed chunk-load errors, not hydration panics: the chunk
remains unavailable, checkpoints selecting its pending work are refused, and its
region bytes are left unchanged; unrelated empty checkpoints remain permitted.
Ominous trials create native item-spawner markers with item metadata and delayed
projectile payloads. `omens.rs` converts Bad Omen to 18,000 ticks of Trial Omen
per level and consumes it at reward ejection. Embedded trial loot retains potion,
bottle and enchantment components; enchant selection and mob steering are
approximate, not vanilla AI parity. Wind-charge bursts include their thrower and
retain the shared one-landing fall-protection latch. Structure generation is not
required: native placed blocks drive these owners.

### Storage blocks and golden dandelions (wave 3)

`crafter.rs` owns nine-slot disabled-slot crafting, four-tick rising-edge pulses,
six-tick animation, oriented ejection, comparator/hopper rules and the exact
`crafter_3x3` menu toggle packet. `shelf.rs` owns all twelve wooden three-slot
shelves and bounded powered hotbar-swap chains. Their verified vanilla NBT
post-images use the opaque WAL and native chunk checkpoint path. Golden
dandelions toggle the verified baby age lock (including tadpoles), reset the
species start age, consume one item and retain the forty-tick interaction fence;
the lock survives native entity checkpoints and absolute entity WAL images.
Stale crafter toggles cannot recreate removed custody, disabled slots reject
shift-click top-ups, and command-forced oversized shelf chains retain only the
last three cells, matching vanilla's guarded hotbar window.

`mc-data::block_entity_kinds` is the shared 26.1.2 kind-to-NBT/protocol identity
table; its complete embedded registry is checked exhaustively against the vanilla
report. Checkpoint projection derives container kind from the placed block, never
slot count: shelves and crafters retain their native opaque post-images instead
of becoming furnaces or chests. Shelf regression coverage includes inventory
debit, native `Items`/byte `Slot`, type-40 updates, real checkpoint projection,
native reload and retained alignment; crafter coverage uses the same real snapshot.

The same native identity table maps all 201 block aliases to their 49 owners;
`block_entity_defaults.rs` creates missing native records at the shared block
commit boundary for `/setblock`, `/fill` and ordinary placement. `shulker.rs`
owns all seventeen 27-slot boxes and native menu type 20. Filled boxes drop one
item with container/name components, never loose contents; accepted placement
restores pre-debit custody after old-owner tombstones, and menu slots reject
nested shulkers. Native byte `Slot` images, absolute WAL replay and checkpoint
hydration retain slot 26 and item components; malformed durable slots fail closed.
Accepted placement tombstones prior custody even when break/re-place folds to
the same state; in-place property/copper-family writes preserve the existing owner.
Hydration decodes a shulker only before live custody exists. Commands retain
custody only for the same block or the existing copper-family boundary, never
merely a shared BE type. Creative filled boxes drop once, and hopper/dropper
transfers obey the same no-nesting slot rule.

`bees.rs` owns retained nectar/flower/hive/anger/sting state, pollination and crop
boosts, three-bee hive custody, honey bottle/shears harvest and campfire calming.
Hive transfers happen after slot-based merge effects; opaque `bees` occupation
post-images and native entity images preserve one occupant across WAL and
checkpoint/restart. `allay.rs` owns held/fetched stacks, liked player/note-block
memories, jukebox dance and the javap-confirmed 6000-tick duplication fence. The
existing death commit drains fetched cargo and the non-vanishing held stack
exactly once, even when mob drops are disabled.
The small `jukebox.rs` owner persists `RecordItem` and the remaining song clock,
handles hopper custody and comparator output, and emits verified level events
1010/1011 with the configured song registry id. Vanilla 26.1.2 save keys and
creature metadata are verified in `.analysis/codex-logs/w3/bees-vanilla-javap.txt`;
flight/flower selection remains deliberately simple, not exact vanilla mob AI.
Hive occupation and jukebox song clocks advance in memory and materialize at
checkpoint; opaque WAL images record custody/content transitions, not idle ticks.
Allay collection sensors reuse a freshly rebuilt chunk census every twenty ticks
and revalidate candidate IDs; failed bee block searches back off two hundred ticks
with restart-local AI cooldowns.
Silk-touch hive breaks atomically transfer the authoritative occupant image into
the single dropped item's `minecraft:bees` component instead of releasing bees;
ordinary breaks remove the old owner and release occupants with nectar retained.
Accepted placement restores validated occupants and `minecraft:block_state`
honey level from the actual pre-debit stack, then journals the new block owner
after its block write. Rejected malformed item components cannot debit inventory
or create a hive owner.
Break custody rejects corrupt owner NBT through the corrupt-chunk path rather
than panicking. Multiple bee-bearing loot stacks are normalized to one carried
owner on the first nonempty stack; excess stacks retain no bees, while absent
carriers release occupants. Checkpoint materializes accepted pending hive
placement before encoding occupants, even if the bee tick has not run.

Attachment cascades remove native block-entity custody before publishing loot;
`block_drop_context.rs` captures live container slots and hive occupants before
accepted writes and rolls each break once, so later writes cannot cancel or
duplicate its drops. Vanilla wall aliases, `copy_components`, `copy_state` and
decorated-pot sherd branches preserve the selected durable item image.
All 71 vanilla copied block owners use the same compiled rules in the embedded
default registry and full sidecar registry.
Corrupt block-entity capture returns a storage error and quarantines the resident
chunk before break writes or debit. Explosions use the same captured loot and
custody admission, retaining copied contents while applying explosion decay.
`copied_components.rs` implements the javap-verified 26.1.2 streams for supported
copied component kinds plus existing typed components, using the pinned built-in
component registration index rather than dynamic Configuration registry IDs.
Its sibling tests list all 80 unsupported preserved registry kinds and require
explicit failure, never
silent omission. Every current-window click publishes one atomic authoritative
slots-and-carried image, including refusals; client hash predictions are ignored.

The four 26.1.2 template-bearing components share `item_stack_template.rs`:
`use_remainder` (25), `charged_projectiles` (49), `bundle_contents` (50), and
`container` (75). Their javap-verified template framing is raw item registry ID,
count, then component patch, unlike ordinary count-first inventory stacks;
container entries additionally carry an optional-presence boolean. Native NBT
and command admission retain remainder and bundle templates, including the
item-name shorthand, while malformed empty templates fail closed. Independent
raw-wire fixtures cover every component; decoded bow, crossbow (including
multishot), and trident ticks guard inventory and nested ammunition counts.

Accepted non-piercing arrow hits and bee stings increment the transient living
render counts, published as javap-verified INT metadata indices 12 and 13.
`projectiles/embedded_projectiles.rs` stages arrow impacts until damage admission
succeeds and reuses the existing row tick for the independent vanilla
`20 * (30 - count)` removal timers; unchanged timers do not clone appearances.
LivingEntity save/load omits these counters, so no WAL or native NBT fields are
added, and restart and player respawn clear them. Decoded bow-hit regressions
cover count-one publication, count-zero decay, refusals, and late-viewer sync.

Ordinary spawn eggs now share `spawn_egg.rs` admission instead of a copper-only
special case. The held item's default `minecraft:entity_data.id` comes from the
26.1.2 component reports (all 87 defaults are embedded); native entity-data
overrides/removals select or refuse the registered type. The javap-verified
collision-empty clicked-block versus adjacent-face rule and downward placement
offset use resident collision shapes, checking only the spawned body's footprint
so adjacent glass pen walls do not refuse a valid spawn. Normal `spawn_mob`
initialization, entity journaling and player inventory post-images remain the
owners; a successful spawn debits one egg in survival and none in creative.

Loom buttons index the exact `minecraft:banner_pattern` tag vector emitted in
`UpdateTags`, resolving its raw IDs against the registry order emitted in
`RegistryData`, as verified in 26.1.2 `LoomMenu.getSelectablePatterns` and
`clickMenuButton`. `TagsData` retains that registry binding for sidecar and
embedded startup; embedded native banner tags now contain their real members.
The independent station-catalogue pattern ordering is removed, and ordinary
stripe-bottom and reusable flower-pattern inputs have regression coverage.

Container clicks retain vanilla 26.1.2 `handleContainerClick` admission:
an old state ID does not reject an otherwise valid current-window click.
All menus execute against current custody and publish a full slots-and-cursor
image; merchant payment and result clicks use that same rule, with current
costs and offer uses still enforcing each transaction. Native decoded
back-to-back clicks cover all 22 menu layouts, and a stale 32-stick payment
placement produces the one-emerald merchant result preview.

Item admission rejects negative or unknown outer, template and merchant item
IDs before native custody, with an independent creative-slot guard; holder
offsets use checked arithmetic. Bundles have a Solaris admission bound of 64
entries (vanilla's list stream codec itself is unbounded). Actual main-hand
slot changes and hand swaps cancel matching projectile charges, and stale
charges do not swallow generic shield release. Dropped-item lava, fire and
cactus contact uses merge damage and journaled removal tombstones; vanilla
report-authored `damage_resistant/#is_fire` defaults control fire immunity
without item-name special cases; preserved `!minecraft:damage_resistant`
removal markers revoke that default immunity. Unsupported addition patches
remain rejected.

### Tick pacing and `/tick` (TickControl)

`tick_control.rs` owns session-only pacing state: target rate, freeze, queued
step ticks and sprint. The owner loop reads its period each tick (`1000/rate`
ms; sprint skips the wait). A frozen tick still runs the WAL/journal/broadcast
pipeline so the tick number keeps advancing, but `begin_tick` marks it
non-simulating: non-player entity, random/scheduled block, spawn, weather,
day-clock and pickup phases are gated, and `GameTime` (the `/time query
gametime` source) counts only simulated ticks. Absolute tick deadlines are
not paused (known limitation, documented in OPERATING.md). Rate/freeze/step
changes broadcast `TickingState`/`TickingStep` and are resent on join; the
native client does not yet act on ticking packets or the action bar.

## Migration — completed by ADR 0014

The model was built beside v1, reusing model-independent protocol, data, NBT,
worldgen, physics and Anvil codecs while replacing tick, native storage, entity
AI, inventory/containers, publication and the plugin boundary. ADR 0014 records
the actual port/delete decisions and cutover; Phase 5 removes the old server
crates and makes this the sole current model. The original Q10 load/client
targets above remain the owner decision record, not an assertion that every
target or historical receipt was exercised by the deletion slice.

## Rejected

- Spatial partitioning (regions or lanes as the unit of parallelism): one crowd
  collapses to one core (Q3).
- Sequential vanilla entity order: does not parallelize (Q4).
- Per-decision WAL with fsync before replies and separate durable streams:
  replaced by one tick journal (Q6).
- Per-tick plugin AI code: cannot reach hundreds of thousands of mobs (Q7).
- In-tick synchronous plugin calls: a slow plugin would stall the server (Q8).
- Incremental refactor of the current core: two models side by side for a long
  time is how the current contradictions accumulated (Q9).

## G6 — native population storage correctness (2026-10-16)

Entity checkpoint validity is bounded per record, not by the sum of every
entity in the world. The legacy entity-owner checkpoint path must not reject a
valid population merely because its aggregate exceeds 64 MiB; metadata and
individual entity decode/encode budgets, atomic publication, LSN/slot checks
and tombstone non-resurrection remain corruption fences. The runtime world
checkpoint already writes entity rows in one native transaction independently
of the bounded 256-participant WAL apply-batch API.

The first 500k correctness run reached eager owner capture, then hit its 6 GiB
memory cap while constructing a second complete NBT population in the writer.
The writer now encodes each frozen entity record directly into the same native
transaction, rather than retaining every expanded NBT record at once. A lazy
encoding error aborts all domains and indexes. The next run saved successfully
but hit the same cap while reopening the eagerly decoded table. Runtime
recovery now reads metadata first and visits entities once from the same pinned
transaction, hydrating one payload at a time with unchanged participant/slot
and reverse-slot corruption checks. Live restoration drains the replay map,
releasing each recovered row as its live owner is constructed instead of
retaining two complete populations through column growth. There is still one
frozen owner capture; no aggregate far-villager format or alternate authority
is added.

The imported Falcons case exposed a separate chunk-count memory bound: adding
5,000 villagers to a 144,753-chunk native world reached eager owner capture
but exhausted an 8 GiB slot during checkpoint preparation/commit. Preparing
owned baselines and composed deferred chunks for the entire captured set at
once is not permitted. Chunk captures retain frozen owner references and
metadata descriptors; the journal resolves and writes one baseline/effective
pair at a time, releasing that pair before preparing the next. All chunk,
entity, player and global writes still belong to one Immediate native
transaction. Late preparation/encoding corruption aborts that transaction;
dirty/revision ACKs remain strictly after its successful commit. Chunks omitted
from the captured set are neither materialized nor rewritten.

The same imported-copy fixture now completes under the enforced 6 GiB shared
slot: 145,842 captured chunks, 47,489 entities after SIGKILL/reopen (42,489
imported plus 5,000 added), and all 5,000 houses/beds/job POIs retained.
Child phase VmHWM was 2,157,334,528 bytes for capture and 2,143,686,656 bytes
for commit; this is correctness/memory evidence, not a performance window.
The observed 24,650,969,088-byte native file and 988-second commit are recorded
without claiming a disk-size or latency improvement. Receipt:
`.analysis/codex-logs/g6-storage/many-chunks-result.yaml`.

Recovery distinguishes persisted checkpoint bookkeeping from newer values:
restore compares inhabited/debt projections with their captured manifest
values, so even an identical repeated WAL post-image stays clean. A genuinely
new value, active clock advancement, or consumed-debt zero remains dirty until
the normal successful commit/ACK boundary. A checkpoint is not itself a
simulation mutation, and inactive outstanding debt does not repeatedly enlist
an otherwise unchanged chunk.

Native checkpoint writers retain byte-identical chunk/entity rows and their
per-row LSNs instead of restamping unchanged content. The admitted global
checkpoint tick fences older WAL participants, including absent keys; offline
edits advance from the maximum of that floor and the chunk's stored LSN.
Omitted live entities still become new durable tombstones. Identical same-tick
global/player records are also retained rather than reinserted.
The WAL gate accepts a save without new appends when its whole requested
interval was already validated at recovery; it never invents a missing tick.

Block/fluid schedulers track pending native projection separately from WAL
emission. Recovery compares folded per-chunk queues with their durable manifest
images; only changed queues, including empty clears, enlist chunk projections.
The complete queue images remain restart authority. A matching completed queue
image ACK releases projection custody; emitting its WAL image or acknowledging
an older, different queue never does. Coverage cannot retire an unacknowledged
native projection.

`NativeCheckpointIo` reports actual committed chunk/entity writes and their
encoded row payload bytes, excluding keys, indexes, dictionaries, globals and
redb copy-on-write overhead. These are separate from live logical user-table
bytes and the native file length: free pages and delayed reuse can enlarge a
file without duplicating live authority. The Linux no-op regression
`core::native_checkpoint_noop_tests::native_noop_save_twice_keeps_chunks_and_size`
opens a world once and saves twice without simulation; `SOLARIS_NOOP_ROOT`
selects an existing disposable copy, with its seed in `SOLARIS_NOOP_SEED`.
It records `noop-saves.json` and requires the second save to capture no chunks,
rewrite no chunks/entities, write no such payload bytes, and leave file length flat.
Correctness receipt: `.analysis/codex-logs/g6-storage/noop-result.yaml`.

`core::native_population_tests::native_offline_population_save_kill_reopen`
is the native offline fixture path. It seeds real villagers through the same
`spawn_mob` owner as the horde fixture, without commands, clients or ticks, and
adds one real house, bed and composter POI. It exercises ordinary eager
checkpoint capture/write, SIGKILLs the writer with owners still open, then
reopens through runtime recovery and checks count and sampled durable state.
The default 300-villager case runs in focused tests. For the large correctness
gate, set `SOLARIS_POPULATION_COUNT=500000` and an unused absolute
`SOLARIS_POPULATION_ROOT`, then run:

```sh
.analysis/tools/mem-run.sh -m 6G cargo test -p mc-core --lib \
  core::native_population_tests::native_offline_population_save_kill_reopen \
  -- --exact --nocapture
```

The retained native world includes `population.json`, recording native file
bytes/entity and whole-process live RSS/entity before checkpoint capture
(registry, chunks and allocator included; not a deep heap-size estimate).
This storage fixture does not claim terrain coverage, housing for the entire
population, live 500k-brain performance, or a performance-window measurement.
Evidence for the G6 storage acceptance belongs in
`.analysis/codex-logs/g6-storage/result.yaml`.

## Abstract resident catch-up — decision 001-f (2026-10-17)

Inactive intervals become per-chunk debt on activation, excluding that tick's
ordinary live step. The single tick merge owner rereads resident state and
applies one bounded time step to the whole chunk: item lifetime/despawn,
incremental furnace cooking, cereal crop growth, and villager work/restock
counters only. It never loads chunks for dormant simulation and never executes
unloaded villager movement, breeding, population growth, hoppers or redstone.

Each written cell has `block_expect`; furnace slots stay under the existing
container custody owner. Accepted ordinary block writes and container mutable
custody invalidate the chunk's remaining debt in the same tick WAL batch,
including writes made before the slice reaches that chunk. Hydration and
catch-up custody are separate from that external-write fence. Fingerprints
from amendments 001-a/c are superseded. Remaining debt and its zero image
retain the storage-only dirty/checkpoint-ACK eviction fence.
Accepted item births and chunk crossings share the same fence, including
admissions after the catch-up pass. They reuse the existing journal touched
set and previous-chunk baseline; replay hydration seeds that baseline and
therefore preserves owed age for pre-existing items. No new queue or
load-time fingerprint is introduced.
Accepted merchant trades likewise cancel old chunk debt before the new offer
uses can borrow historical restocks. Catch-up never rewinds a `last_work`
counter already advanced by a newer live pass.

Admission is bounded to two chunks, 200 elapsed ticks per whole-chunk step and
4096 target effects per tick. An interval with more than 4096 targets is
conservatively discarded rather than partially replayed by target. Missing
crop neighbours/light/hydration and unavailable villager job sites under-apply.
Evidence and exact covered checks:
`.analysis/codex-logs/abstract-sim/result.yaml`; these are correctness receipts,
not exclusive performance or real-client acceptance.

## Native session auth gate — opt-in plugin login (2026-10-19)

`[auth] require_plugin_login` (default `false`) wires `EdgeConfig.auth` into
`CoreConfig.require_plugin_login` at edge startup. Default behavior is
unchanged: native sessions play immediately when the flag is off.

When enabled, each new native play session starts locked and fails closed until
an admitted plugin `unlock_session(session)` command releases that exact session.
Only plugin-declared roots `register`, `login`, and `changepassword` may run while
locked; their arguments reach the owning plugin without native chat echo or
command logging. All other player gameplay input, including other plugin
commands and public chat, is refused before native or plugin menu dispatch.
The native lock also fences movement, damage, mob targeting and item pickup.
Locked movement is corrected at most once per teleport and only for a pose that
actually diverges from the server position: clients answer every teleport with
accept plus their pose, so correcting each input or the echoed pose would be an
unbounded teleport ping-pong that floods the connection.
Login prompts belong to the plugin, not core. The lock is connection-local:
disconnect preserves player custody and a replacement connection starts locked.
Unlock follows the existing plugin command journal/acknowledgement boundary;
configuration-off sessions retain their existing behavior.
Unlock admission requires the explicit `auth` manifest capability at both guest
batch conversion and sealed host admission; event subscriptions alone grant no
unlock authority. Only `solaris-auth` declares this grant in the standard pack.
The auth component registers no build hook: the native session gate above is
the sole enforcement path for login-locked gameplay.

Guest password work uses synchronous `password_hash(password)` and
`password_verify(password, phc)` host imports on the existing plugin worker.
The host produces Argon2id v19 PHC strings with OS-random salts (19 MiB, two
iterations, one lane); malformed PHC and wrong passwords verify false.
Verification rejects other algorithms, versions or cost parameters before
performing native work: guest fuel and store limits cannot bound an import.
Passwords are capped at 256 UTF-8 bytes before KDF work: oversize hashing
raises a host error, while verification returns false. These imports never
log arguments and are refused during precommit hooks, like other non-identity
imports. They do not run on the native tick owner.

## Settlement extraction workers — miner and lumberjack (2026-10-19)

Miner and lumberjack work is native tick authority like other villager work.
Their durable excavation and felling cursors (`mining_progress`,
`lumber_progress`) travel with the villager carry in the same entity/WAL
after-image; there is no second store. Block breaks and replanting go through
the journaled `world.set_block` + `ChangeSet` path, deposits through the
canonical settlement inventory fences. Each work attempt scans a bounded
number of cells, so per-tick cost does not grow with settlement radius.

Seed-87122 replay (2026-10-20) corrections: a miner walks to the cell it is
about to dig, because a one-cell route-arrival tolerance around the previous
open cell could strand it just beyond dig reach; a finished comb advances to
the next `level` (three blocks higher, two columns aside) instead of ending the
mine, and `level` is part of `mining_progress`. The lumberjack search no longer
has a height-band cursor (`vertical` is removed from `lumber_progress`): each
attempt reads a bounded number of simulated columns over the settlement
sphere's whole vertical chord, skipping inactive chunk strips in one step.
Worlds saved with the old worker-progress shapes fail closed. Route search
starts from the body's real position when its cell has no standable support (a
rim over water) and refuses drops whose landing column is covered. The
autonomous planner examines 64 terrain-following site candidates per pass
(solid ground within ±8 blocks of the bell) and a builder candidate must have a
walkable route to settlement storage.

Seed-87122 follow-up replay (2026-10-21): a jump step in `find_path_ctx`
(above `STEP_HEIGHT`) requires body clearance over the edge at the highest
rise the departure column permits (1.16, 1.10 or 1.05 blocks), because a
two-block roof over the ledge alone stops the body above the edge and the
resident jumps in place forever, while a staircase roofed over both steps caps
the rise inside the crossing window. A panicking resident whose flee route has
no step retreats to home, job site or meeting point. A miner waits (branch
intact) while a dig cell lies within a hostile's sensor reach (plus the dig
reach), and an
alternate lane starts at its column's ground (`surface_origin`), skipping
columns whose ground lies outside the zone.

Seed-87122 round 3 (2026-10-21): mining levels no longer shift sideways.
A higher level reuses the lane's lower-level staircase (cells inside it are
walked or skipped, and air beside it counts as the miner's own shaft, not a
cave) and runs its corridor one column beside the lane (`z_direction`), because
the same cells directly above a continuing staircase hang over its air and are
unwalkable. A finished lane set restarts at the next level, and a dig cell
within a hostile's reach is waited on for `HOSTILE_PATIENCE` (2,400 ticks,
`mining_progress.blocked_since`, saved) before the lane closes. A miner far
from `last_open` walks there first, since a route to the solid dig cell falls
back to the closest reachable point, which can be a dead-end pocket.

Seed-100 buildings replay (2026-10-21): after the L1 to L2 upgrade the mine
re-certified with new bounds, so `mining_progress` reset correctly, but every
lane then failed `safe()` because the air of the L1 tunnel (and the other
miner's) beside it counted as a cave, and the saved `lane` had run past the
zone width. `own_tunnel` floods the open cells beside a dig cell (cap 1,200,
inside the zone, no fluid, opening onto the entrance surface, at least six
cells) and accepts them as the mine's own; sealed pockets and dents still count
as caves. `blocked_since` is now `stalled_since`: a miner that walks without
reaching its dig cell for `STALL_PATIENCE` closes the lane exactly like a
hostile that outstays it.

Seed-100 buildings replay, round 5 (2026-10-21): the cobble in the chest stayed
flat while two miners dug for thousands of ticks. They carried 40-60 dirt plus
30-50 cobble (a mixed load never fills one 64 stack) and delivered only at day
end, which a stopped day clock never reaches. `heavy_load` also sends a miner
home when the whole load weighs 64 items, and a mine with nothing left to dig
delivers instead of idling with its cargo. The quarry itself was finite: a
flight reaches `min(run, depth)` blocks down and three levels cover it, so a
15-deep natural zone ran dry after ~12k ticks. Flights now chain
(`mining_progress.pass`, already saved): flight N starts at the open end of
flight N-1, two lead-in cells and a staircase two columns beside it, alternating
sides so it never undercuts the flight above and fits the footprint. A mine's
operational floor is 48 blocks below its certified floor (built mines were 12),
always clamped to the settlement sphere; X/Z stay the certified footprint.

Seed-87122 claim QA (2026-10-21): a storage ordered at the village anchor was sited 110 blocks away after 21 refusals, and its builder placed 4 blocks in ten minutes. Site search now scans square rings outward from the anchor (`construction_cursor` is the ring index; success resets it to 0), so the nearest free site wins; an owner order starts at the anchor and scans the whole sphere, the autonomous planner keeps 64 columns per pass. The slow builder stood in a crater, jumping at a two-block wall: a route search from a body hovering mid-jump started at the cell above it, so the wall top looked one block up and the same impossible route was replanned forever. `find_path_ctx` now starts a body that is not resting on a collision top from the floor it left (within a 1.3-block jump apex). Replay of the kept world: the builder escaped the crater and went from 4 to 83 placed blocks in 400 ticks.

Seed-87122 autonomy QA (2026-10-21): the planner did admit Storage L1 (nearest ring site), but the saved builder woke inside a house whose door step was roofed by an oak crown and stood in the doorway for 7000 daytime ticks (route search closed after 11 nodes); `assign_builder` returned as soon as any Builder existed, so nobody replaced it. `release_trapped_builder` now re-probes an on-shift builder's route to storage each planning pass and detaches it when unreachable; the usual candidate selection then picks a resident that can reach storage. Replay of the kept world: s20 released, s19 took over and the storage finished within ~1000 ticks of day break.

## A9 — native world size and offline pruning (2026-10-22)

Measured, not estimated. The ADR's 145,842-chunk fixture (24,650,969,088 bytes,
988 s commit) is about 169 KB per chunk. Row census of a fresh seed-424242 overworld
pregenerated by `world pregenerate --radius 512` (4,225 chunks): one baseline row
per chunk, 125,914 bytes of plain NBT each (light arrays, heightmaps, biomes,
block-state palettes), 255 KB per chunk of file because redb rounds a 126 KB value
up to a 256 KB block (1,077,940,224-byte file). Of the live-compat imported world
copy (2,174 chunks, 1,208,487,936-byte file) the rows were 102.5 KB per baseline
and 101.5 KB per chunk-metadata row: every checkpointed chunk rewrites a full second
copy of its side state (the metadata row always differs from the baseline by the
journal stamp). Section overlays were 33 rows and 2.5 KB in total, so palette and
bit-packing of overlays is not where the bytes are (zstd-3 on them measured no
better than the existing zlib: 2,545 vs 2,519 bytes) and is unchanged.

Decision: baseline and metadata rows are stored as `0xF0 | u32 BE raw length |
zstd-3 frame with content checksum` (`native/pack.rs`); `0xF0` is not an NBT tag
type, so the plain form (first byte `0x0A`) stays a valid stored representation
and old rows load unchanged. Rows under 256 bytes or that do not shrink stay plain.
Output is deterministic, so the checkpoint's byte-equality retention still works.
Decoding is bounded by the 16 MiB record budget and fails closed on a truncated
frame, bad checksum, length mismatch or trailing bytes. Level 9 was about 11%
smaller than level 3 (13.9 vs 15.5 MB on the 4,225 baselines) at several times
the encode cost, so level 3 is kept. Entity, global and player rows are small and
stay plain.

Result (same chunks, same code, plain vs packed rows): fresh world file
1,077,940,224 -> 23,334,912 bytes (255 KB -> 5.5 KB per chunk, 46x; rows 126 KB ->
3.7 KB, 34x). Live-compat copy 1,208,487,936 -> 40,550,400 bytes (30x) with
`world compact` (4,319 rows rewritten in 0.8 s, file compaction 0.4 s). Loading
all 2,174 chunks through `resolve_base`: 277 us/chunk plain, 283 us/chunk packed
(+2%; chunk assembly, not decompression, dominates). A per-chunk offline commit
(`apply_offline_snapshot`, fsync-bound) measured 1.88 ms plain and 1.96 ms packed
on NVMe with free pages available: the commit latency is unchanged at that scale;
what shrinks is the volume written per chunk (a metadata row 101 KB -> 1 KB, a
baseline 126 KB -> 3.7 KB), which is what bounded the 988 s commit and the backup
size. The 145,842-chunk commit time itself was not re-measured.

Not done, on purpose: skipping storage of untouched generated chunks and
regenerating them from (seed, worldgen revision). It is not exact: the generator
depends on the plugin-applied registry, `upgrade-worldgen` deliberately keeps
stored chunks across a revision bump (regenerating them would move the old border
terrain), and pregeneration exists to avoid generation at join time. Pruning below
is the exact form of the same idea: forget only chunks that still equal generation.

`world prune --keep-radius R [--center x,z] [--max-inhabited-ticks N] [--dry-run]`
and `world compact` (`mc-core/world_maintenance.rs`, `mc-world/native/maintenance.rs`)
run on a stopped world under the world lease. A chunk is pruned only if it is in the
overworld, outside the keep rectangle, **listed in `generated_baselines`** (written by
`fill_generated`, i.e. pregeneration/LOD fill, and by the checkpoint's lazy base
insert, both generator output; `import_chunk`/`import_staged_chunk`, the Anvil
import, never write it), has no overlay section, no persistent entity row (including
tombstones of consumed generated slots), a metadata row equal to its baseline apart
from the journal stamp and inhabited clock, and an effective inhabited clock at most
N ticks. Rationale: Anvil imports share the `imported_chunks` table, and a build with
`InhabitedTime = 0` equals its own baseline, so equality to the baseline alone would
delete work the generator cannot recreate. The provenance table is created with new
worlds and on first generated write in an older one; rows stored before it existed
are unlisted and never pruned (fail closed, no migration guess). Baseline, metadata,
watermark and provenance rows go in one transaction per 2,048 chunks; kept chunks
are never touched. The numbers below (seed 424242, 4,225 chunks, `--keep-radius 256`:
3,136 pruned, 1,089 kept, file 23,334,912 -> 6,189,056 bytes; a following
`world pregenerate --radius 512` generated exactly the 3,136 missing chunks) were
measured before provenance was recorded, when every baseline counted as regenerable;
on a world pregenerated by this build the provenance rule is expected to select the same chunks (not re-run). The
command opens/closes the core first, so a crash tail is folded in before judging.

Known remainder: a checkpointed chunk still stores its whole side state a second
time (about 1 KB packed). A field-level delta against the baseline would shrink it
further but changes the retention comparison; not needed at the measured size. redb
leaf pages are not densely filled after random-order rewrites (file 40.6 MB vs 9.5 MB
of live row bytes on the compacted live copy).
