# Owner field-test handoff — 2026-09-10

## Contract and snapshot status

**Draft, deliberately unfinished as-is snapshot.** The owner explicitly stopped
implementation, requested one commit and a push to `main`, and will continue with
another agent. Do not interpret this commit as a release or a gameplay pass.
The owner explicitly chose `main`, not a separate handoff branch.

The owner was testing the locally installed debug build at
`~/.local/bin/solaris`, version `0.0.5`, protocol 775 / Minecraft 26.1.2.
Its installed SHA-256 is
`68e4377e9591a7f9995e4e9140bbaa150f624ea7b741d40c2142795ef1a9fdd8`.
That installation predates the fish/chest edits below. The owner process was not
restarted or replaced during this investigation. Do not overwrite its world or
configuration, invoke `malloc_trim` inside it, or attach an intrusive debugger.

This commit also captures the previously uncommitted work from the same session:
operator/control and world-lease fixes, indexed region reads, aquatic movement,
revision-20 river geometry, compact light/palettes, and resource profiling. Those
changes had their own earlier evidence; that evidence does not validate the new
field-test fixes. Existing first-party sibling repositories are not part of this
commit. Local runtime directories, logs, vanilla JARs and captures stay out of Git.

Worldgen revision is **20**. Do not increment it merely to hide these bugs. An
older revision-19 world still requires the previously documented fresh-world
migration; this field test started with a newly generated revision-20 world.

## What is already changed

### Fish cooking — focused regression passes

- `crates/mc-data/data/required_recipes.json`: added ordinary furnace, smoker and
  campfire recipes for cod and salmon, using normal vanilla recipe identifiers.
- Vanilla 26.1.2 recipe evidence: furnace 200 ticks, smoker 100 ticks, campfire
  600 ticks, one cooked fish, 0.35 XP. Values were read from the local client JAR,
  not recalled from memory. Original Mojang files remain ignored locally.
- `crates/mc-net/src/play/tests/furnace.rs`:
  `embedded_recipes_cook_fish_in_furnace_and_smoker` exercises real furnace
  progression with the embedded recipe set and coal. It failed before the data
  change (empty output) and passed afterwards for cod/salmon and both furnace
  kinds. Campfire and a real graphical client are not yet verified.
- Important next check: recipes are sorted into display IDs. Existing tests pin
  old IDs, including intentionally prefixed legacy recipe names. New real IDs
  can shift positions. Keep client/server advertisement and lookup consistent;
  do not add more fake `zz_...` IDs just to satisfy incidental numeric assertions.

### Double chests — partial implementation, not yet accepted

New files:

- `crates/mc-net/src/play/block_placement/chest.rs`
- `crates/mc-net/src/play/block_placement/chest_tests.rs`

Integration edits:

- `block_placement.rs` calls the new chest planner.
- `block_break.rs` resets a surviving partner to `single`.
- `play.rs::open_chest_container` and
  `scheduled_blocks.rs::cached_storage_chest_like_positions` use actual paired
  block states rather than any adjacent chest.
- The old adjacency-only helper/re-export was removed from `containers/chest.rs`
  and `containers.rs`.

The planner implements opposite-player facing, clockwise/counterclockwise
`left/right`, same-kind/same-facing single-neighbor selection, no third chest,
secondary-use behavior, and mutation preconditions for inspected neighbors.
These rules were inspected in 26.1.2 `ChestBlock` bytecode.

**Known verification state:** the first chest test command failed to compile.
A rust-analyzer `Remove all unused imports` quickfix incorrectly removed used
imports from `play.rs`; the four reported imports were restored manually. The
new test also used nonexistent `BlockRegistry::by_name` and `PlayerPose::default`;
these were corrected to `block` and `PlayerPose::new`. Do not confuse those
corrections with a passing chest test. Consult the final gate note below.

Remaining review points: neighbor fencing across chunk boundaries, trapped vs
normal chests, waterlogged states, secondary-use side placement, break/unpair,
old fixtures with propertyless fake chests, shared viewers and hopper storage
access. `ChestWindow` retains existing coordinate-sorted inventory ordering;
vanilla left/right slot ordering has not been independently established here.
The new helper also duplicates an `opposite` transform already available in
`mc_data::block_placement_26_1_2`; consolidate it rather than growing another
rotation convention.

## Complete issue list and concrete next work

### 1. Adjacent chests remain two single textures

**Owner evidence:** two adjacent chests, each with its own latch and single model.
This is separate from their darkness. Screenshots were around
`(-1303, 71, -1252)`, with chest/furnace/workbench nearby.

**Confirmed source defect:** ordinary placement had no chest pairing transition;
container opening nevertheless combined arbitrary adjacent chests. The partial
implementation above is intended to correct both authorities.

**Finish:** run the new planning tests; update legitimate paired-chest fixtures to
use real `facing/type/waterlogged` states; test both placement orders, Shift,
third-chest rejection, counterpart reset after breaking, and opening from either
half. Verify two players see both block-state updates and the same contents.
Use a real 26.1.2 graphical client for texture/opening acceptance.

### 2. Fish cannot be cooked in an ordinary furnace

**Confirmed cause:** the owner loaded the embedded 81-recipe set, which had no
cod or salmon cooking recipes. It was not a demonstrated furnace timing bug.

**Finish:** retain the passing regression described above; verify normal furnace,
smoker and campfire with real items, fuel, output and XP. Check recipe display-ID
consistency after additions. The installed owner binary still lacks these edits.

### 3. A flower floats after its supporting soil is broken

**Confirmed source gap:** `block_break.rs::append_vertical_support_cascade` only
recognizes sugar cane, cactus and bamboo through
`is_vertical_support_cascade_block`. Ordinary flowers are omitted.

**Finish:** extend the actual support-removal policy using vanilla plant survival
rules, not a client-only air packet. Include dependent removals and their read
preconditions in the same authoritative block edit transaction. Preserve normal
loot semantics and do not duplicate drops for upper/lower halves. Check both
players, saved/reloaded chunks and a chunk-boundary support break.
Relevant reusable domain: `mc-world/src/plant_rules_26_1_2.rs`; existing regressions
are under `mc-net/src/play/tests/plants.rs` and block-break tests.

### 4. Grass also floats after its supporting block is broken

A separately reported acceptance case, sharing the likely support-cascade gap.

**Finish:** test short grass and tall grass/upper-lower halves, not only poppies.
Remove server-side state and send all resulting updates. Preserve grass seed /
shears behavior rather than granting a grass item for every support break.
Check the same support policy for relevant placement, explosion and fluid
mutation entrypoints; do not fix only one packet path while leaving the
underlying neighbor-survival rule absent.

### 5. Moving any inventory item turns a water bucket back into an empty bucket

**Strongly localized mechanism, not yet fixed:** the server rejected the owner's
empty-bucket `UseItem` actions as `unsupported_item`. The embedded registry maps
item 1013 to `minecraft:bucket`, 1014 to `minecraft:water_bucket`. The failed
owner actions still held 1013 on the server. A client-predicted fill followed by
an authoritative inventory resync is consistent with the visible water loss;
this is not proof that arbitrary inventory moves themselves consume water.

`play.rs::handle_use_item` handles shield/bow/food, but not bucket pickup.
`bucket_interactions.rs::handle_bucket_use_on` handles explicit block interaction
and can already commit block+inventory changes together.

**Finish:** implement authoritative bucket use for the actual `ServerboundUseItem`
packet. Raycast from the validated player eye/pose with vanilla range, block
occlusion and appropriate fluid mode; empty buckets target source fluid. Reuse
`BucketUsePlan` and the existing simulation transaction, not a client claim of a
filled item and not a special-case inventory resync. Then pick up water, move an
unrelated stack, swap the bucket, close/reopen inventory, reconnect, and verify
that both world fluid and bucket contents remain authoritative. Test both hands.

### 6. Clicking a block with a bucket causes a visible failure

The owner's wording did not specify a crash versus failed/desynchronized use.
**Observed:** PID 2175746 remained alive; inspected logs showed continued ticks,
not a demonstrated server crash. At 09:32:59–09:33:28 UTC, `UseItemOn` with held
item 1013 was rejected as `TargetBlockedOrUnplaceable`, followed by `UseItem`
`unsupported_item`. Do not relabel the report as a confirmed process crash.

**Finish:** exercise pickup and placement through actual vanilla packet ordering,
including source fluid, solid clicked faces, replaceable vegetation, invalid
reach, a full inventory and both hands. The current filled-bucket adapter only
accepts literal air at the adjacent target. Reuse the existing transactional
replacement/drop policy; reject failed uses with authoritative resync and the
correct acknowledgement. If the reported visual failure is still unexplained,
ask for the exact remaining symptom after reproducing the known rejected path.

### 7. Chest lighting intermittently becomes almost black

**Owner timeline:** first dark, then temporarily correct without a fix, then dark
again. Do not close this because it once recovered. It is not the double-chest
texture issue.

**Observed data problem:** startup logged
`block-light table loaded version=blocks-report-conservative ...`.
`mc-data/src/block_light.rs::conservative_opacity` treats ordinary chests as
opacity 15 / no skylight because they are absent from the transparent cases.
**Causality is not fully established:** this is a suspect, not a demonstrated
explanation of the intermittent behavior. No lighting fix has been applied.

**Finish:** inspect exact 26.1.2 chest light/shape behavior; capture light values at
the chest before and after placement, delayed recomputation, streaming and save
reload. Trace full versus incremental light publication, unknown versus known
zero, and block-entity model sampling. Reproduce using the embedded runtime data,
not an oracle sidecar that hides the owner configuration. Correct source metadata
or publication ordering as demonstrated; do not force a constant brightness or
add periodic resends as a symptom workaround. Keep a graphical acceptance gate.

### 8. `time set night` visibly sets daytime

**Owner observation is the acceptance failure.** No fix applied.

**Ruled out by source inspection:** console aliases already map night to 13000,
noon to 6000, midnight to 18000 and day to 1000. The local 26.1.2 `timeline/day.json`
contains exactly those markers. Changing constants arbitrarily is not justified.

**Trace next:** `mc-server/src/console/commands.rs::parse_time` →
`OperatorControlHandle::set_world_time` →
`simulation.rs::set_world_time_response` →
`session/sleep.rs::set_world_time_core` / `broadcast_world_time` →
`command_execution.rs::clientbound_world_time` →
`mc-protocol/src/packets/play.rs::ClientboundSetTime`.
The 26.1.2 packet uses a registry-keyed clock map, VarLong total ticks, partial
fraction and rate, not the old two-long day-time layout. Check actual embedded
world-clock registry IDs, timeline/dimension bindings and the packet observed by
the client. Also rule out a subsequent tick or a real sleep transition overriding
the command. Demonstrate night/day/noon/midnight visually and agreement between
server hostile-spawn time and client sky; keep simulation age separate from day
clock changes.

### 9. RSS stays near a gigabyte despite roughly 235–240 MiB Rust live

**Frozen owner capture, 280 seconds uptime:** RSS 1018.496 MiB, anonymous RSS
954.301 MiB, file RSS 64.195 MiB, requested Rust live 235.054 MiB. Across 200.027
seconds RSS grew 525.875 MiB but requested Rust live grew only 1.759 MiB. Allocated
52.13 GiB **and** freed 52.13 GiB during that interval. Saving used 190.098 of
333.033 process CPU seconds (57.08%). Last save flushed 558 chunks in 10.492 s.

**Newer profile, 1406 seconds uptime:** RSS 955.387 MiB, Rust live 240.186 MiB;
across the next 1125.979 seconds, allocation volume was another 276.193 GiB and
saving used 1037.5 CPU seconds. The owner later reported about 971 MiB RSS after
logging out. No fresh post-logout Rust-live value was supplied; do not invent one.

**Later read-only mapping observation:** 41 anonymous writable mappings with
64-MiB-aligned starts occupied 863.285 MiB RSS; the main `[heap]` occupied 80.762
MiB. These resemble glibc arena heaps, but are not 41 proven independent arenas
or a measurement of free/reclaimable bytes. This later smaps capture totaled
1036.383 MiB RSS and must not be mixed as simultaneous with the frozen profile.
An earlier isolated load/drop probe left 244 requested-live bytes while RSS stayed
245144 KiB; diagnostic trimming reduced it to 49808 KiB. That proves the retention
mechanism on the isolated workload, not how much of the owner RSS can be reclaimed.
Native allocations, allocator overhead and stack residency are not all accounted
for by Rust allocation counters. The first profile also had 166.52 MiB of live
Rust outside estimated owners, and session capture failed `try_lock`.

**Confirmed waste path, not yet fixed:**
`mc-world/src/storage/dirty_flush.rs::DirtyFlushPlan::write` calls `read_region`
for every existing touched region, retaining uncompressed payloads for all its
slots. It replaces dirty slots, then calls the region writer, which zlib-compresses
**all** payloads again, including unchanged slots. The writer additionally builds
a body buffer and then a complete region-image buffer. This is a concrete source
of temporary-buffer pressure and redundant CPU; the profile does not attribute
all allocation bytes specifically to this function.

**Fix next:** preserve unchanged validated compressed chunk records when rewriting
an Anvil region; encode/compress only changed chunks. Keep location/timestamp
validation, compression type, size limits, unique-temp-file writes, stale-version
fences, journal barrier, fsync/rename/parent-sync and dirty-generation commit
semantics. Do not introduce in-place writes or bypass durability. Test unchanged
slot preservation, replaced/deleted slots, corrupt input handling, stale plans,
recovery and readers captured before replacement. Benchmark the same copied
region/chunk workload before/after with requested allocation churn, live peak,
RSS, CPU and save duration. Do not use `malloc_trim` or arena tuning to conceal
the allocation pattern. Do not claim the earlier 57.5% compact-storage probe was
a whole-server RSS reduction.

### 10. Mobs jerk on hit but lack the red hurt flash / hurt animation

Owner reports cows and other mobs. No implementation or vanilla verification yet.

**Next:** inspect 26.1.2 client handling and packet codecs with local `javap` /
wire-probe, distinguishing living-entity damage/hurt events from knockback,
health metadata, death events and player-only hurt packets. Trace
`play/session/entity_combat.rs`, `player_combat.rs`, entity event dispatch and
outbound encoding. After an accepted nonlethal hit, send the vanilla-visible hurt
signal to all tracking viewers; do not broadcast a fake damage event on rejected
attacks or duplicate it for repeated snapshots. Verify attacker, observer,
invulnerability/rejected hit and death transition in the graphical client.

### 11. Skeletons shoot arrows with empty hands and no bow-drawing pose

No fix yet. Relevant owner: `play/session/hostile_authority.rs`, entity spawn /
equipment publication, living-entity metadata and subsequent combat-state deltas.

**Next:** inspect vanilla skeleton equipment initialization and ranged attack
states. Publish a real bow through existing equipment authority for initial spawn
and late tracking; publish the verified item-use/hand/aggression metadata through
the draw/release/cancel lifecycle. Do not fake a permanently drawn bow or render
arrows without their attack state. Test losing the target, death, re-tracking and
two observers. Use local vanilla packet/metadata evidence, not remembered IDs.

### 12. Skeleton arrows never exhibit vanilla inaccuracy

The owner called this a random miss chance. **Do not implement an arbitrary
percentage of intentionally missed shots.** No fix yet.

**Next:** inspect 26.1.2 skeleton ranged attack and projectile `shoot` behavior,
including target aim compensation, velocity perturbation/distribution and
server difficulty. `play/session/hostile_authority.rs` owns current skeleton
shot planning (`SKELETON_ARROW_SPEED`, shot interval and target range).
Reuse the existing deterministic random-state ownership; avoid a second global
RNG. Test seeded nonzero directional variance, difficulty behavior and preserved
speed/aim invariants, then compare a graphical varied-shot scenario against
vanilla. A deterministic center-aim unit test alone is not acceptance.

## Validation, publication and continuation

Earlier complete workspace evidence before these field fixes was **4500 passed /
194 ignored**, with formatter, strict workspace Clippy and code-health passing.
That is historical evidence only. Fish's focused before/after result is described
above. The initial chest command failed compilation; fixes to those compile errors
were made before the final snapshot gate. The final snapshot gate result is
recorded at the end of this document, not inferred from old green runs.

The owner requested the repository as-is. Do not mark remaining issues complete,
claim graphical acceptance, or publish a release/tag based on this snapshot.
Continue with the gameplay defects before optional tuning; run the canonical
`python3 -m tools.harness run correctness` once for the completed implementation,
plus focused reproductions and approved graphical client scenarios. Each known
failed owner scenario stays unresolved until the same or a stronger scenario
passes. Independently review the completed changes, not just this handoff.

Local-only evidence (not included in Git):

- `.analysis/codex-logs/owner-rss-1018/receipt.json`, `owner-profile.json`,
  `process-smaps.txt`, `mapping-summary.json`; independent RSS review:
  `OwnerRssReview`, pass with the timestamp/arena/churn caveats above.
- `.analysis/codex-logs/owner-four-fixes/owner-profile-latest.json`,
  `owner-bucket-log-slice.txt`, `fish-vanilla-26.1.2.json`,
  `chest-vanilla-26.1.2-javap.txt`, `time-command-vanilla-26.1.2-javap.txt`,
  `timelines-vanilla-26.1.2.json`.
- `.analysis/codex-logs/compact-profile/receipt.json` and
  `installation/receipt.json` for the prior storage/profile implementation and
  the installed binary backup.

A remote agent will not have these ignored captures. The observations above are
included to make the handoff usable remotely; obtain fresh vanilla evidence and
fresh graphical reproductions rather than assuming a referenced local file exists.

## Final as-is gate result

Command: `cargo fmt --all && python3 -m tools.harness run correctness`.
Formatting completed and the formatter check passed. **The correctness profile
failed at strict workspace Clippy**, before the full test suite ran:

```text
crates/mc-net/src/play/tests/furnace.rs:1195
Identifier::parse(&format!("minecraft:{name}"))
clippy::needless_borrows_for_generic_args
```

The immediate mechanical correction is to remove `&` from that `format!` argument.
It is intentionally recorded rather than presenting this requested as-is snapshot
as green. Rerun the failed Clippy gate after correcting it, then run the chest
regressions and remaining test gates. Recipe display-ID tests may expose the
separate issue described above. No successful post-change chest run or graphical
acceptance is claimed.

Local receipt:
`.analysis/validation/20260910T095908-correctness-0t0ey1bm/result.json`.
