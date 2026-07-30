# Active Playable Task

This file contains only the current playable queue and recent evidence. The
previous detailed log is preserved in
[`../archive/status/2026-07-19-playable-active.md`](../archive/status/2026-07-19-playable-active.md)
for targeted lookup.

## Target

Keep the normal 26.1.2 client stable through a useful survival session, then
broaden the loop beyond wood -> tools -> restart. Optimize for common gameplay,
multiplayer correctness, and visible failures before rare parity edges.

The baseline loop remains:

```text
join -> move -> gather -> craft -> build -> fight/farm -> save/rejoin
```

This is Playable Spike Mode. Do not turn focused playable evidence into M100
replacement-readiness claims.

## Active Checkpoint

Next autonomous goal checkpoint: route `playable`; mechanically move the full
three-test debug-command parsing, water-corridor fixture, and zero-count give
execution class beginning with
`debug_commands_parse_survival_mutations_and_give` and ending with
`debug_give_zero_count_clears_hotbar_slot_before_item_lookup` out of aggregate
`crates/mc-net/src/play/tests.rs` into a focused sibling module. Preserve every
test and production behavior, leave the preceding
`recoverable_death_xp_uses_level_cap` test in the aggregate file, and use
explicit imports rather than a new `use super::*`. The complete six-test chest
quick-move, stack-limit, menu/crafting revision, and persistent-container
claim class has already moved to
`crates/mc-net/src/play/tests/container_inventory.rs`; its concentration and
validation are recorded in
[`../evidence/mc-net-container-inventory-test-extraction.md`](../evidence/mc-net-container-inventory-test-extraction.md).
The owner-run subjective fresh-world seed-`712816` playtest and
release-candidate performance gates remain queued at their declared
boundaries.

The remaining Phase 1 flaky/self-skip inventory is closed. Two `mc-server`
structure tests are now explicit fail-closed local-sidecar gates, and the
test-world generator accepts only a non-empty region-plus-`level.dat` cache
shape while rejecting partial output. Together with the preceding worldgen and
block-drop closures, the bounded Rust and 72-file non-Rust scan found no
remaining unexplained retry, quarantine, serial-only, disabled, or
environment-sensitive success class. Scope, reproduction, and the explicit
release/manual boundary are in
[`../evidence/phase1-flaky-test-inventory.md`](../evidence/phase1-flaky-test-inventory.md).

The `mc-worldgen::structures` local-artifact class is closed. Three tests that
previously returned success when the local blocks report or plains-fountain NBT
was absent are explicit ignored gates and fail closed when selected. The three
selected gates pass against the current local 26.1.2 sidecars; scope, ownership,
reproduction, and limits are in
[`../evidence/mc-worldgen-structure-local-artifact-tests.md`](../evidence/mc-worldgen-structure-local-artifact-tests.md).
This does not claim complete vanilla village parity.

The first workspace closeout run exposed and closed a real `mc-net` block-drop
test-isolation race. Its process-global journal await probe could be reached by
another concurrently running block-drop owner despite the three probe tests'
serial mutex, producing a closed-receiver panic. The probe is now scoped to the
exact Tokio owner task and consumed once; the focused eight-test slice and
workspace gates pass. Evidence and limits are in
[`../evidence/mc-net-block-drop-await-probe.md`](../evidence/mc-net-block-drop-await-probe.md).

The Cargo feature-gated test class is closed. The workspace declares only
`mc-script/lua-runtime` and `mc-net/load-bench`: the former adds the already
classified 80 in-process Luau tests, while the latter exposes benchmark support
without changing the 1,857-unit/3-doctest `mc-net` list. No other local feature
or `required-features` test gate exists. Current lists, owner boundaries, and
limits are in
[`../evidence/cargo-feature-gated-test-inventory.md`](../evidence/cargo-feature-gated-test-inventory.md).
This does not claim that either explicit feature suite was executed on this
documentation-only checkpoint.

The Phase 1 progress-wait class is closed. First-party tests contain no
remaining wall-clock sleep or scheduler-yield call; candidate loops wait on
exact packet, channel, watch, notification, process, or filesystem events, use
timeouts only as failure watchdogs, or perform finite iteration. The bounded
inventory, classifications, reproduction, and limits are in
[`../evidence/phase1-progress-wait-inventory.md`](../evidence/phase1-progress-wait-inventory.md).
This does not close the remaining ignored, feature-gated, structural, or
manual test inventories.

The automatable graphical preflight is closed. A real Minecraft Java 26.1.2
client joined a fresh no-operator `tellus_like` seed-`712816` world at
`(0.5, 71.0, 0.5)`, rendered terrain and HUD, received all `81` visible
view-distance-4 chunks without absence or degraded delivery, wrote a valid
`854x480` screenshot, and passed the fail-closed artifact validator. Exact
config, observations, checksums, logs, screenshot path, and limits are in
[`../evidence/worldgen-seed-712816-preflight.md`](../evidence/worldgen-seed-712816-preflight.md).
This is agent-run graphical startup evidence, not the owner disposition.

The current tree now includes a deterministic production-sampler renderer and
checked `2048x2048`-block height, biome, and vegetation artifacts for seed
`712816`. The exact extent, resolution, fixed palettes, reproduction command,
SHA-256 checksums, and evidence boundary are recorded in
[`../evidence/worldgen-mosaics.md`](../evidence/worldgen-mosaics.md).

The downhill-drainage vertical is closed. Revision-10 cells now select only
strictly lower seeded hydraulic elevations, depth-three accumulation stays
bounded, and deterministic seed/order/border regressions protect the network.
The mapped seed-42 debug probes completed all 25 chunks in 526 ms
(`47.5 chunks/s`); this is not the open release-host comparison.
Evidence is in
[`../evidence/worldgen-downhill-drainage.md`](../evidence/worldgen-downhill-drainage.md).
No graphical or gameplay-readiness claim is implied.

## Recent Checkpoints — through 2026-07-30

| Slice | Result | Current-tree evidence |
| --- | --- | --- |
| Periodic natural spawning runtime | Chunk preparation now registers templates without production one-shot materialization. A four-chunk rotating scheduler runs independent friendly/hostile cadences over simulation-loaded chunks, rechecks distance, active-chunk, cap, collision, support/fluid, night and block-light fences, refills after movement/despawn, and reports bounded rejection metrics. Spawn ownership is split into scheduler, planning, periodic authority, commit/publication and test-only legacy modules; the server owns only a narrow ticker. Repo-owned 26.1.2 supported-entity tables now cover eight common overworld biomes. The 20-minute graphical survival and restart identity gates remain unrun. | Periodic focused slice `12/12`; bounded chunk-candidate projection `1/1`; chunk registration/no-one-shot gate `1/1`; common-biome rules `2/2`; `mc-entity` `568/568` executable and `mc-net` `1849/1849` executable PASS; mob-presence wire gate `6/6`; workspace Clippy `-D warnings`, formatter, diff-check and code-health PASS. Independent review found the unbounded pre-selection snapshot; selected-chunk-only snapshots fixed it. The following test-gate review passed after exact body-push and crossing-prewarm fixes. |
| Respawn bundle | Respawn republishes health, abilities, default spawn, chunk view, position and the complete inventory snapshot. A disk-world restart/rejoin restores an alive, damageable player rather than stale dead state. | `mc-net` respawn `6/6`; exact wire lifecycle `1/1`; save/restart/rejoin `1/1`; focused Java route test passed. |
| Attachment sturdy faces | Removed the block-name fallback. Torch/attachment support now requires an exact embedded 26.1.2 state fingerprint and complete collision-face coverage; partial faces, fences and mismatches reject through resync-before-ack/no-debit. | `mc-data block_facts` `6/6`; placement support `7/7`; accepted and rejected TCP paths `1/1` each. |
| Stair neighbour recomputation | Placement publishes root plus corner before acknowledgement and exactly one later debit, survives save/restart, then removal after restart publishes target air plus the straightened neighbour and survives a second reopen. Existing stale placement/break paths retain atomic conservation. | stair unit slice `25/25`; raw-TCP place/restart/remove/restart `1/1`; scoped read-only review passed. |
| Out-of-reach break rejection | The stale TCP gate now matches the accepted vanilla oracle: an out-of-reach `START_DESTROY_BLOCK` is acknowledgement-only in survival and creative and does not invent a target-cell resync. | exact TCP test passed; full `mc-test-harness` package passed. |
| Two-client door/trapdoor convergence | Existing mutation-token authority already commits both door halves atomically and a trapdoor as one edit. The actor receives exact decoded `SectionBlocksUpdate`/`BlockUpdate` packets, the loaded observer receives every accepted delta, exact `facing`/`half`/`open` properties remain intact, and replaying the original plan changes neither half and emits no block or inventory publication on the rejection tick or the following owner tick. | exact concurrency/wire test `1/1`; complete `mc-net toggle` slice `6/6`; focused Java playable-route test passed; full package-split L2, Clippy and code-health passed; independent review PASS. |
| Scheduled fluid restart continuity | Runtime/persistence PASS: a TCP client places a water source, clean save/shutdown preserves either settled water or pending air backed by a concrete persisted source-water tick, restarted simulation reaches the adjacent spread cell, a second reopen keeps source/spread water, and duplicate scheduled requests/sequences are rejected. Real-client rerun remains BLOCKED by the graphical host, not by Solaris. | exact restart gate `1/1`; water-bucket slice `3/3`; lava-water scheduled path `1/1`; M94 Java route PASS; full package-split L2, workspace Clippy, formatter and code-health PASS; reviewer blocker fixed. Approved runner artifact `.analysis/real-client-runs/20260725T123715Z-m94-regression-pack-fPX76A` fails validation because observations stayed `prepared-owner-run/not-run`; `client.log` records `ERROR DISPLAY` and repeated `glfwInit failed`. |
| Representative movement boundaries | Existing authority is consolidated without production changes: `step_entity` proves farmland step-up, exact terminal-velocity clamp stability, and pre-sampling non-finite rejection; player geometry proves standing/crouching/swimming body and eye dimensions at one collision edge, powder-snow sink/boots/Shift behavior, and the exact long-fall `0.9F` boundary. Crouch coverage is server body-height collision authority, not invented sneak-edge anti-cheat. | `mc-physics` `60/60`; `mc-net movement` `39/39`, including packet-side `collision_correction_applies_powder_snow_movement_context`; full package-split L2, workspace Clippy, formatter and code-health PASS; reviewer terminal-clamp blocker fixed. |
| Chunk prepare disconnect/rejoin | A client moves from center `(0,0)` to `(3,0)`, disconnects immediately after the new center while batch-1 preparation is active, waits for authoritative unregister, then rejoins with the same offline UUID and restored position. The fresh session receives exactly the required 25-chunk ring with no duplicate/out-of-view chunks, inherited unloads or stale block deltas. Stream drop now releases claims already buffered in `result_rx`; unregister also releases only prepared claims owned by that session, while generic/prewarm claims remain independent. The test uses capacity `121` to exclude unrelated dirty-cache pressure that made a minimal-capacity fixture abandon three chunks. Entity visibility is not claimed by this wire test. | exact reconnect gate PASS in ~7s; `chunk_stream` harness `2/2` executable tests PASS (`1` sidecar gate ignored); buffered-result and unregister claim regressions PASS; M94 save/restart visibility Java test PASS; full package-split L2, workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. The actual graphical real-client leg remains host-BLOCKED by the already recorded `ERROR DISPLAY`/`glfwInit failed` environment. |
| Window-0 rejection/resync recovery | The existing authoritative rejection path already conserves inventory. A malformed pickup with an impossible carried item leaves slot 36 and the empty cursor unchanged, moves no dirt elsewhere, and resends the exact unchanged state id. The next valid pickup must advance the state id by exactly one and move all ten items to the cursor; the following valid place must advance exactly once again and move the complete stack to slot 37 with an empty cursor. | exact malformed/recovery TCP gate PASS; ordinary Window-0 cursor flow PASS; `mc-net inventory` `59/59`; full package-split L2, workspace Clippy, formatter, diff-check and code-health PASS; reviewer stale-packet blocker fixed by exact `wrapping_add(1)` assertions. |
| Dropped-item merge conservation | Compatible item entities merge through one atomic regional-owner CAS that replaces the deterministic survivor and removes the consumed identity across one or multiple lanes. Larger count survives, then lower entity ID breaks ties; exact item identity and max-stack limits reject incompatible, full and unknown stacks. The survivor keeps the younger vanilla age (`max(spawn_tick)`), later pickup-ready tick and compatible active owner block; expired different-owner blocks normalize away. Merge runs both at readiness and when bounded item-physics steps later bring entities within the `0.5` radius. Publication updates survivor metadata before despawning the consumed identity, without a full-store scan. | owner CAS/stale/cross-region journal rollback `3/3`; item-drop matrix `16/16` plus expired-owner and late-convergence regressions; real TCP player-drop merge `1/1`; `mc-net pickup` `52/52`; full package-split L2 (`mc-entity` `522/522`, `mc-net` `1739/1739` executable), full harness (`block_edit` `98/98` executable), workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Dropped-item despawn restart deadline | Production save writes the item snapshot and authoritative lifecycle clock. The restart gate retains the server-authored identity/stack/retained state, shifts only the clock-derived age and pickup-delay to `deadline-200`, then binds through the normal loader. The item is visible on wire before the reconstructed deadline, emits `RemoveEntities` at `deadline..deadline+2`, and the second production save contains no entity to resurrect. Rescheduling now removes the old deadline-bucket membership before inserting the new one, so one item has exactly one queue entry and one by-ID deadline even after repeated merge-age updates. | exact restart/wire/persistence gate `1/1` (~11s); complete `persistence_inventory` `2/2`; `mc-net item_despawn` `3/3`; full package-split L2 (`mc-net` `1740/1740` executable), full harness (`block_edit` `98/98` executable and persistence `2/2`), workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. Test uses exported runtime `ITEM_DESPAWN_AGE_TICKS` and atomically rewrites its accelerated checkpoint via temp+sync+rename. |
| Common damage-source matrix | Living damage now has typed lava, suffocation and starvation sources with exact embedded 26.1.2 flags. Player damage derives armor, protection, shield and armor-durability policy from one typed source mapping rather than separate matches; `generic_kill` bypasses armor/invulnerability/resistance but correctly retains enchantment protection. Real contact scanning classifies solid overlap as suffocation, lava as 4 damage, fire/soul fire as fire, lit campfire as campfire and air as no damage. NaN, infinity and unsupported test sources reject without health, inventory or wire mutation. | `mc-entity living_26_1_2` `32/32`; `mc-net player_damage` `4/4`; contact/world paths `4/4`; full package-split L2 (`mc-entity` `523/523`, `mc-net` `1743/1743` executable), full harness (`block_edit` `98/98` executable), workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Player death inventory/XP conservation | One owner-locked alive-to-dead commit now applies the complete death policy exactly once. Survival and Adventure with `keep_inventory=false` atomically clear inventory and cursor, reset XP, spawn exact item stacks and one recoverable XP orb; `keep_inventory=true`, Creative and Spectator preserve inventory/cursor/XP and create no item or XP entities. A stale plan whose gamerule snapshot no longer matches is rejected without mutation, and dead-to-dead replay produces no second drop or event. The operator-only `keep_inventory` gamerule is exposed through the command tree, persisted in `world.dat`, defaults false for legacy metadata and is restored before serving. | policy/idempotency matrix `1/1`; stale gamerule-plan rejection `1/1`; ordinary raw-TCP death item+XP drop/reset `1/1`; keepInventory death→respawn→save→restart/rejoin retention `1/1`; metadata `5/5`; full package-split L2 (`mc-net` `1746/1746` executable), full harness (`block_edit` `98/98` executable), workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Partial pickup and overflow conservation | Existing owner authority already commits inventory credit and item-entity remainder under one session/entity lock. A raw-TCP collector fills all 36 admissible inventory slots through ordinary Window-0 actions, leaving exactly one compatible cobblestone capacity. A second client drops stack `3`; the collector receives exactly one item into slot 36 (`63 → 64`) while the same entity ID remains visible with count `2`, without a take animation or removal. Full-inventory rejection, stale entity CAS, stale session/disconnect, concurrent claimant uniqueness and requester loss after owner apply remain covered by the production matrix; no runtime change was required. | exact near-full wire gate `1/1` (~3.75s); `mc-net pickup` `53/53`; full package-split L2 (`mc-net` `1746/1746` executable); `block_edit` `99/99` executable PASS serially and all remaining harness targets PASS; workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Crafting-table max-craft and cursor conservation | Existing crafting authority already clones the destination inventory before every result batch and consumes ingredients only after a complete output batch fits. A capacity matrix proves that five logs with room for exactly eight planks crafts two batches and leaves three logs, while room for only two planks is a total no-op. A raw-TCP client places five logs into one crafting-table input and one result `QuickMove` yields exactly 20 planks with empty input, result and cursor. Normal pickup, cursor merge, stale owner projection rebuild, disconnect recovery, transactional partial-room rejection and one-time publication remain covered by the current production matrix; no runtime change was required. | `mc-net crafting` `30/30`; capacity/no-op matrix `1/1`; raw-TCP max-craft `1/1`; crafting-table harness `3/3`; full package-split L2 (`mc-net` `1747/1747` executable); `block_edit` `100/100` executable PASS serially and all remaining harness targets PASS; workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Chest max-stack and malformed replay conservation | Existing chest mutation planning already uses canonical item facts for pickup, quick-move and QuickCraft. Valid-source matrices prove that stack-1 buckets never merge to count `2`, stack-16 snowballs cap at `16`, and greedy drag preserves the exact cursor remainder. Raw TCP sends impossible bucket `2` and snowball `17` carried predictions and receives the unchanged authoritative chest/cursor at the unchanged state id; a valid bucket pickup then advances exactly two revisions, a later impossible prediction again resyncs unchanged, and a valid placement advances exactly two more. Existing two-client stale-state and unsupported-mode gates remain green; no runtime change was required. | `mc-net chest` `25/25`; item-limit matrix `1/1`; chest raw-TCP/replay `6/6`; full package-split L2 (`mc-net` `1748/1748` executable); `block_edit` `101/101` executable PASS serially and all remaining harness targets PASS; workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Shield angle/timing and axe-disable | Existing shield authority retains the exact five-tick activation delay, front-hemisphere check and one-time durability CAS. Local 26.1.2 item-component and packet oracles add exact axe `disable_blocking_for_seconds=5.0`, shield cooldown scale `1.0`, and `ClientboundCooldown` play ID `0x16` (`Identifier` + `VarInt`). A blocked axe hit damages the shield exactly once, atomically removes active use, stores a simulation-tick deadline of `+100`, publishes `minecraft:shield/100`, rejects stale held-stack or selected-slot plans, suppresses shield reactivation before expiry, and permits it exactly at the deadline. Disconnect clears the session-owned deadline. | local oracle facts and packet codec `1/1`; shield unit/owner/stale-CAS slice `27/27`; two-client raw-TCP block→cooldown→suppressed use→damage→expiry→re-block `1/1`; full package-split L2 (`mc-net` `1751/1751` executable); `block_edit` `102/102` executable PASS serially and all remaining harness targets PASS after one transient persistence fixture rerun; workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Unified worktrees, inbound DoS fence and tag releases | Every registered worktree HEAD is now an ancestor of `main`; stale uncommitted campaign fragments were discarded only after their correct committed replacement was verified in main. Serverbound reads retain at most 1 MiB for one incomplete frame, while existing framing still caps wire frames and decompression independently. The accept loop owns a full-lifetime semaphore sized `clamp(max_players*2+16, 32..512)`, preventing unbounded ten-second pre-login task/buffer amplification. Tags matching `v*` run fmt, Clippy, code-health, the full Rust test/build gates, all Loader modules and installer tests before native x86_64/AArch64 release jobs publish checksum-paired assets. The installer verifies SHA-256 before extraction and rejects traversal, unexpected/duplicate entries and links before atomically installing the binary. | ancestry check PASS for all 10 worktrees; inbound ceiling and task-limit unit tests PASS; independent security/release review PASS; combined `mc-net` `1794/1794` executable; `block_edit` `102/102` executable PASS plus one documented ignore across four exact shards; all remaining harness binaries PASS; Java M94 and Fabric/NeoForge/Forge Loader tests PASS; native release package/install/`--help`, checksum rejection and symlink rejection PASS; workspace Clippy, formatter, diff-check and code-health PASS. |
| Data-driven mob profiles and villager village brain | Every canonical non-player living 26.1.2 type now has one validated common behavior profile. Ground, flying, aquatic, amphibious, hostile pursuit, immobile and villager-schedule movement are table-driven; creeper fuse, skeleton-family arrow and generic melee are explicit, while unimplemented species-specific attacks remain `UnsupportedSpecial` and never fall through to melee. Villagers persist adult/baby schedule, activity, home/job/meeting POIs and bounded plugin overrides through the regional owner and ECS/entity save path. Ordinary brain work is staggered across 20 ticks, custom schedule boundaries wake the full active population, override expiry bypasses the general physics shard, and stale members recursively split CAS batches instead of starving current neighbours. Generated village markers create one toolsmith with exact wire metadata; save/restart restores the same UUID without duplicate marker spawn. `EntityBehaviorHandle` atomically swaps only validated mob and villager profiles without exposing entity storage. | `mc-data` `220/220`; `mc-entity` `528/528` executable plus `5` documented ignores; `mc-world` `228/228`; `mc-net` `1798/1798` executable plus `3` documented ignores; villager brain cadence/override/stale-CAS `4/4`; hostile slice `48/48`; generated-village raw-TCP save/restart `1/1`; full worldgen harness `5/5`; `block_edit` `102/102` executable plus one documented ignore; all remaining harness binaries PASS after one transient persistence fixture rerun; workspace Clippy, formatter, diff-check and code-health PASS; independent review PASS. |
| Pillager crossbow combat | Pillagers now use a dedicated crossbow policy instead of `UnsupportedSpecial`. The regional entity owner retains aim, charging and charged deadlines; the exact local 26.1.2 defaults are five visible aim ticks, 25 charge ticks, eight-block attack range and a deterministic 20–39 tick charged delay. A supported pillager projects its canonical crossbow to the client, toggles exact metadata index 17 while charging, cancels without a shot when the target becomes non-targetable, and fires one owner-attributed arrow without the skeleton arm-swing packet. Mutable mob equipment, patrol/raid composition, difficulty inaccuracy and broader raider AI remain open. | mob profile oracle `2/2`; ECS retained-component round trip `1/1`; pillager owner/wire slice `4/4`; neighboring hostile authority `18/18`; skeleton regression/owner-request gate `2/2`; formatter PASS. No raw-TCP or manual-client gate was run. |
| Iron-golem panic defence | Adult villagers now persist a bounded `LAST_SLEPT` projection and inclusive `GOLEM_DETECTED_RECENTLY` deadline. Every 100 ticks, three recently rested villagers inside the exact 10-block agreement box and near a whitelisted 26.1.2 villager threat may create one deterministic, block- and entity-collision-validated persisted iron golem; the 200-tick sensor refreshes the exact 599-tick memory and save/restart preserves both villagers and golem without duplication. The golem selects the nearest active hostile within follow range except creepers, commits `FollowTarget`, applies the vanilla-form damage distribution through ordinary entity damage/death/loot authority, vertically launches surviving targets and publishes tracking-scoped entity event `4`. Actual bed pose, the five-villager gossip path, Java RNG stream identity, player-created anger/reputation, flowers, repair/crackiness and raids remain open. | pure villager boundaries `11/11`; ECS retained-state round trip `1/1`; village-defence owner/spawn/wire/restart `6/6`; neighbouring villager brain `12/12`; hostile authority `18/18`; skeleton `2/2`; melee velocity regression `1/1`; `cargo check -p mc-net --features load-bench` PASS; formatter PASS. No raw-TCP or manual-client gate was run. |
| Guardian beam combat | Guardian and elder guardian now use a dedicated `GuardianBeam` policy instead of `UnsupportedSpecial`. Canonical attributes are `30/80` health, `0.5/0.3000000119` movement, `35` follow range and `6/8` attack damage. The runtime-only owner state retains warmup/beam deadlines and target session/entity identity; start publishes metadata index `17` plus entity event `21`, completion reserves one publication epoch and applies ordered `indirect_magic` then mob-attack damage, and cancellation resets target metadata to `0`. The normal guardian refuses a new target at the exact three-block boundary; elder may continue an already acquired close target. The current server advertises EASY, so magic damage is `1` or `3`. Player targets only, tracking visibility instead of full LOS, squid/axolotl selection, thorns, mining fatigue and runtime difficulty selection remain open. | guardian owner/wire `6/6`; `mc-data` `222/222`; living damage `32/32`; runtime retained-state round trip `1/1`; hostile/pillager/skeleton/village regressions green; production check and affected-crate Clippy PASS. No raw-TCP or manual-client gate was run. |
| Toolsmith merchant vertical | Generated toolsmiths now expose the exact 26.1.2 merchant screen, offer and select-trade packets with a data-driven five-offer novice catalog. Selection moves bounded compatible payment stacks into persisted merchant inputs; result pickup and `QuickMove` atomically commit the player inventory/cursor, remaining inputs, offer use, villager XP/level and entity snapshot through one stale-fenced session/entity CAS. Repeated trades conserve payment remainder; incompatible cursor, full inventory, exhausted offer and stale replay paths reject without partial mutation. Used offers restock only while the villager is in `Work` activity within two blocks of its claimed job site, at most twice per 24,000-tick day with a 1,200-tick cooldown. | Full `mc-data`, `mc-protocol` and `mc-entity` packages PASS; `mc-net` `1807/1807`; merchant authority/container slice `5/5`; generated-village raw-TCP open→select→trade→save/restart `1/1`; full worldgen harness `5/5`; workspace Clippy `-D warnings`, formatter and code-health `0 fail / KEEP` PASS; independent read-only review PASS. No manual-client gate was run. |
| Toolsmith job-site assignment | An unemployed level-one adult now resolves its claimed POI against the loaded world snapshot and accepts only the exact supported `minecraft:smithing_table` mapping. The villager profession and canonical five-offer merchant state commit in the same owner CAS as its brain transition; stale batch members remain recursively isolated. Visible observers receive exactly one villager metadata update, while unsupported or unloaded blocks fail closed and unchanged later ticks publish nothing. | exact mapping `2/2`; villager assignment/unsupported/one-time metadata and existing cadence/restock suite `7/7`; entity checkpoint save/load preserves assigned profession, brain POIs and merchant catalog `1/1`; full `mc-net` executable suite `1807/1807`; workspace Clippy and formatter PASS. This is focused owner/wire evidence, not a raw-TCP or manual-client assignment gate. |
| Debug/release benchmark matrix refresh | The load matrix now uses durable worlds for regional transaction scenarios, waits for real game-mode transitions before Survival conservation races, performs final disk saves for disk-backed servers and reports inhabited-time bookkeeping separately from off-tick checkpoint I/O. The focused 20-client same-spawn VD8 route streams `289/289` chunks to every client, drains CPU/IO workers and finishes with zero dirty chunks in debug and release. | debug 20-client gate PASS: first-chunk p99 `510 ms`, full-window p99 `34.681 s`, tick p99/max `14.521/21.962 ms`; release PASS: first p99 `69 ms`, full p99 `3.409 s`, tick p99/max `11.069/15.499 ms`; checked replay, short 400-tick soak, bounded reconnect/slow-reader replay and slow-reader gates PASS in both builds. O3 combat and journal fsync PASS. O3 explosion authority remains FAIL at p99 `55.941 ms` against the frozen `50 ms` budget. Exact low/high cgroups and long soaks remain open. |
| Villager trading and hurt gossip authority | Gossip now lives in one persisted retained state shared by employed and unemployed villagers. A successful trade records exact 26.1.2 `TRADING +2` (max `25`, daily decay `2`); accepted player melee records `MINOR_NEGATIVE +25` (max `200`, daily decay `20`) in the same regional damage CAS. Values below `2` disappear. Merchant offers and debit use the matching session UUID's total weighted reputation, while stale/replayed trade and invulnerability-window attacks cannot duplicate gossip. The generated-village raw-TCP route proves trade→close→attack→reopen publishes `special_price=+2`, selects an exact `17`-coal payment, and restores the same surcharge after save/restart. The ledger is bounded to 64 player UUIDs; a full ledger never rejects the underlying trade or damage. | local decompiled 26.1.2 gossip/event/price oracle inspected; gossip add/cap/decay/ledger matrix `3/3`; merchant trade+hurt authority `4/4`; villager brain cadence/restock/decay `7/7`; entity checkpoint restart `1/1`; generated-village raw-TCP hurt-price/restart `1/1`; full `mc-entity`, full `mc-net`, and full worldgen harness PASS. Workspace Clippy `-D warnings`, formatter, diff-check and code-health PASS; the independent review's legacy-save and decay-timestamp findings were fixed and covered by migration/parity tests. No manual-client gate was run. Transfer, `VILLAGER_KILLED`, and production zombie-villager curing were closed by following checkpoints; Hero of the Village remains open. |
| Bounded villager gossip transfer | An `Idle` or `Meet` initiator retains its valid runtime interaction target or selects the nearest eligible villager from a bounded active-population spatial index. At exact `distance² <= 5`, the target transfers into the initiator through at most ten weighted draws using Java's 48-bit `nextInt(bound)` algorithm; duplicate `(player,type)` selections collapse, each type applies its exact transfer decay (`10`, `20`, `5`, `20`, or `20`), results below `2` disappear, and existing values merge through `max`. The seed is Solaris-deterministic from both UUIDs and the tick, so the complete vanilla per-entity `RandomSource` stream and bit-for-bit selected-entry identity are not claimed. Both participants receive the same 1,200-tick runtime cooldown and may enter only one disjoint pair per tick. Gossip and both cooldowns commit in one two-snapshot CAS, so a stale participant leaves both unchanged. Save/restart keeps the transferred gossip but clears interaction target/cooldown like vanilla. | checked-in oracle fact sheet `docs/evidence/villager-gossip-transfer-26.1.2.md` records the local source paths, SHA-256 fingerprints, confirmed facts and RNG boundary; gossip weighted-transfer/Java-`nextInt` matrix `6/6`; brain/owner/production/cooldown/stale/disjoint-pair matrix `12/12`; transfer checkpoint restart `1/1`; full `mc-entity`, full `mc-net`, and worldgen harness `5/5` PASS. Workspace Clippy, formatter, diff-check and code-health PASS; the independent review's inaccessible-oracle and false-RNG-parity findings were fixed with the checked-in fact sheet, Java legacy `nextInt` port, explicit deterministic-seed boundary and focused regressions. The current tree extends the same transfer authority to all five vanilla gossip types and closes `VILLAGER_KILLED`; production curing was closed by the following checkpoint, while Hero pricing and population/defence remain open. |
| Villager kill witness gossip | A lethal accepted player attack on a villager records the direct `MINOR_NEGATIVE` hurt event on the victim and exact `MAJOR_NEGATIVE +25` for living indexed villager witnesses inside the victim's bounded follow-range cube. Witness reputation immediately affects merchant pricing. Stale witnesses split away without starving current neighbours, and replaying the already-lethal victim cannot duplicate gossip. | focused lethal-witness and stale-neighbour tests `2/2`; current-tree `mc-entity` `545/545` and `mc-net` `1822/1822` PASS; strict affected-crate Clippy, formatter and code-health PASS. No raw-TCP or manual-client kill-witness gate was run. |
| Positive villager gossip state | The retained ledger now carries exact `MINOR_POSITIVE` and `MAJOR_POSITIVE` fields. `ZombieVillagerCured` atomically adds `+25` and `+20`; weighted reputation uses `+1` and `+5`; daily decay is `1` and `0`; transfer decay is `5` and `20`. All five vanilla types share the existing bounded transfer selector. `MINOR_POSITIVE 25` transfers as `20`, while capped everlasting `MAJOR_POSITIVE 20` is discarded on transfer. Positive values survive checkpoint/restart and immediately feed merchant pricing. The new alpha fields are required on persisted gossip entries; no old-schema fallback was added. | positive gossip focused matrix `3/3` within the full gossip suite `8/8`; toolsmith checkpoint/restart `1/1`; current-tree `mc-entity` `545/545` and `mc-net` `1822/1822` PASS; strict affected-crate Clippy, formatter, code-health and independent read-only review PASS. Production zombie-villager curing was closed by the following checkpoint; real-client evidence and Hero pricing remain open. |
| Zombie-villager curing and conversion | A real `ServerboundInteract` with `minecraft:golden_apple` reaches one session/simulation owner turn that rechecks the connected player, hand slot and exact stack, reach, entity identity, Weakness and conversion state before debit. Cure-created Strength remains a normal active effect through `ConversionType::SINGLE`; pre-existing Strength remains independently owned, and Solaris creates no permanent cure-specific base attack modifier. Completion drains at most four deadlines per tick through an explicit type-changing regional CAS, keeps entity ID/UUID, resets canonical villager facts, removes hostile indexes, publishes ordered despawn→spawn, adds Nausea 200 and records `ZOMBIE_VILLAGER_CURED`. Checkpoint restore is fail-closed for wrong type/lifecycle, missing active Strength or resurrected Weakness. Bed/iron-bar acceleration is intentionally not implemented. | domain matrix `4/4`; explicit conversion-CAS and rollback `2/2`; owner/lifecycle/persistence matrix `5/5`; raw-TCP golden-apple/event-16/debit gate `1/1`; full `mc-entity` `551/551`; all `1833` `mc-net` lib tests covered across exact shards with executable tests green and documented ignores unchanged; `block_edit` `103/103` executable plus one documented ignore; workspace Clippy `-D warnings`, formatter and code-health `0 fail / KEEP` PASS. No manual-client or accelerated-conversion gate was run. |

Strict formatter, Clippy `-D warnings`, diff checks, and the affected regression
slices passed before closeout. These are playable-route claims only, not M100 or
replacement-readiness claims.

`wait_for_health_below` also now uses strict numeric ordering: the real
client accepts exact zero below a `0.001` threshold without changing its
push-driven wait. Player collision now consumes the complete embedded vanilla
shape table before custom-registry fallbacks and supplies vanilla movement
context for leather boots, Shift descent, and long falls through powder snow.
Entity physics now carries the matching dynamic context: the exact 26.1.2
powder-snow-walkable mob tag stands only from above, short-falling blocks use
the full base shape, and ordinary entities sink. ECS motion retains accumulated
fall distance, so every entity switches to the earlier vanilla 0.9F shape after
a fall beyond 2.5 blocks and resets on landing. Both player and entity dynamic
branches remain behind the exact embedded state-fingerprint check.

## Current Queue

This queue is binding across context compaction: common vanilla gameplay first,
then production plugin API, then measured optimization, and only then rare
hardening. An already-open lower-priority diff does not override this order.

1. [x] Run the requested 20-minute MCP survival session on the current build, with
   one fast subagent making the decisions and no deterministic scenario runner
   or operator setup. Record concrete client-visible failures; an owner-played
   subjective-feel gate remains separately pending. The client advanced
   authoritative `game_time` by 24,071 ticks, reached a stone pickaxe through
   ordinary survival actions, and resumed after one deliberate
   disconnect/reconnect without a crash or reproducible blocker.
2. Treat failures from that session as the playable queue. Fix the first common
   player-visible blocker, then rerun the shortest real-client path that
   reproduces it.
   The exact dense-world failure is closed on an O3 server with 5,132 injected
   cows and 5,227 total entities. Solaris keeps one outstanding keepalive,
   treats other valid inbound packets as connection-liveness evidence, uses
   vanilla's three-tick default movement interval, and rotates a bounded
   movement-publication shard under extreme entity counts. A real 26.1.2 MCP
   client remained in play for 975 client ticks with no keepalive mismatch,
   timeout, reliable drop, or retry. The reported water gap now has
   server-owned air/drowning, vanilla swimming metadata, and aquatic
   physics that no longer pushes fish to the surface. The client now receives
   the vanilla enabled-feature packet and can cross kelp/seagrass instead of
   being corrected onto their former full-cube fallback. Chunk sections now
   publish the exact vanilla `fluid_count`, so the real client no longer skips
   entity/fluid overlap: entering source water reports `in_water=true` and a
   fluid height of `0.8888889`. The O3 deep-water client gate now covers
   ascent, diving, swimming pose, air depletion and drowning damage without a
   disconnect.
3. The agent-run 26.1.2 hostile-combat gate is closed. Ordinary client action
   packets killed a zombie with an iron sword and collected its rotten-flesh
   drop; a skeleton published a visible arrow and damaged the player; a
   creeper damaged the player and was removed, consistent with its explosion
   path already proved by the TCP regression. The retained harness shows that
   local operator commands only created the deterministic night-time fixture
   and summoned the three mobs. This proves the functional client/server
   paths, not subjective combat feel or natural survival progression.
4. Basic economy and whole-chunk land claims now run on the production Lua API.
   Close the remaining claim surfaces (containers, fluids, explosions and
   entity interaction) through a first-class zone protection policy after the
   ordinary break/place slice is client-verified.
5. [x] Close this owner batch with a 20-minute MCP-driven survival session whose
   decisions are made by one fast subagent. Do not use the deterministic
   scenario runner or operator setup. The run completed; its server profile
   exposed and closed the ordinary break spike described below.
6. Keep the rare multi-region save recovery `fsync`/metrics issue documented
   but deferred. Do not resume it unless it becomes ordinary save corruption or
   blocks the playable or plugin path.

## Recent Evidence

- The Loader client gate now has an isolated MCP launcher for Fabric, NeoForge,
  and Forge. Forge attaches the existing fat Java agent; Fabric loads that
  artifact from its development runtime classpath after Knot startup, while
  NeoForge compiles the same endpoint sources into the Loader game classloader
  and starts them from client lifecycle events. Each
  profile accepts a per-run bearer token, port, game directory, and username;
  its default game directory stays under ignored `.analysis/` and does not
  mutate the owner's PrismLauncher instance. All three profiles passed Java 25
  input validation, agent-jar construction, and Gradle launch-graph dry runs.
  The shared MCP HTTP transport regression and all affected Loader/agent package
  tests are green.
  The gate now also has one reproducible two-owner fixture under
  `examples/loader-live-gate/`. Production plugin discovery starts both Lua
  owners and exposes their separate commands; each command grants its exact
  block carrier and opens its exact screen, while each screen button routes
  only to its owner. The combined client archive activation proves two blocks,
  two items, two screens, four exact assets, and two interactions through the
  shared client contract. Its dedicated config isolates server port, world, and
  plugin paths. The first real Fabric launch exposed an over-broad mixin package
  that made Fabric reject the ordinary Loader entrypoint; the accessor now lives
  in a dedicated `dev.solaris.loader.fabric.mixin` package and the same client
  reaches its render loop. The Fabric MCP profile no longer attaches that agent
  before Knot owns the Minecraft classes. It keeps the existing agent artifact
  on the development runtime classpath, starts it on the first Fabric client
  tick, and publishes Fabric tick/state plus join/disconnect lifecycle events
  through the existing MCP event bridge. The MCP surface now also has one exact-title,
  exact-label confirmation-button action for the ordinary client confirmation
  UI. A fresh 26.1.2 Fabric process exposed all 43 tools, opened
  `Allow Solaris content from 127.0.0.1:25567?`, accepted `Allow`, and reported
  pushed `in_play=true`. Its isolated cache contains both exact bundle
  identities with matching SHA-256:
  `ruby-live:rich-content/1/70dd527ac0c5075faf1dff65e8e426f657746d42215e4fc4fd18244ac5b9d765`
  and
  `sapphire-live:rich-content/1/6c16425b2bf9c5415184345c4cb6bc10e98bf41a3e73dc27b3915aa7962418a5`.
  That handshake gate did not invoke either owner's content. A follow-up fresh
  Fabric process exposed all 44 tools and ran both fixture commands after the
  world render became ready. `loader_ruby` opened `Ruby Loader Fixture`, showed
  the verified cyan item plus red block display, granted one named
  `Ruby Fixture Block`, and its exact `Confirm Ruby` button returned the
  ruby-owned Lua chat message. `loader_sapphire` independently opened
  `Sapphire Loader Fixture`, showed the verified green item plus blue block
  display, granted one named `Sapphire Fixture Block`, and its exact
  `Confirm Sapphire` button returned only the sapphire-owned message. The
  client remained in Play with both distinct items. A later fresh Fabric world
  run granted and consumed each exact item separately, placed Ruby as
  `solaris_loader:loader_block` and Sapphire as
  `solaris_loader:loader_block_1`, and visibly rendered the red and blue world
  blocks together. Survival breaking both blocks observed air, the world drop,
  and inventory pickup, returning one exact named `Ruby Fixture Block` and one
  exact named `Sapphire Fixture Block`. A fresh NeoForge 26.1.2 run then passed
  the same gate: its Configuration prompt used the connected remote address
  while `currentServer` was not yet available, entered Play, placed the same two
  distinct projected block ids, rendered the red and blue blocks together, and
  confirmed air/drop/pickup plus both exact named items after survival breaks.
  Forge remains not run.

- One real 26.1.2 client completed an unscripted, agent-directed survival run
  from `game_time=1581` through `25652` without operator commands or a scenario
  runner. It gathered wood, used the 2x2 and crafting-table interfaces, made
  wooden and stone pickaxes, mined and explored, respawned after six natural
  deaths, then deliberately disconnected/reconnected and continued the same
  world. There was no crash, connection timeout, reliable-command drop, or
  reproducible gameplay blocker. One narrow-shaft entrapment death did not
  reproduce and remains a movement watch item; this run does not replace the
  separate owner-played feel gate.

- The run's server profile caught two normal block breaks at `117-121 ms`
  post-admission. Every regional break was cloning an 8x8-chunk ownership
  snapshot for a falling-block check that only reads the edited vertical
  columns. The regional path now snapshots only chunks containing applied
  edits, matching the established non-regional footprint. Eleven focused
  survival-break regressions remain green, including falling sand and relight.
  A repeated real-client grass break completed with `9.1 ms` total
  simulation-command work and no slow-command attribution warning.

- The profile's apparent `49.5 ms` breeding spike was sheep grazing: the
  combined timer included both passes, and grazing performed one synchronous
  regional-owner timer mutation per active sheep. Grazing now reuses the
  selected sheep snapshots and applies all timer changes through one
  conditional batch. Runtime telemetry reports `sheep_grazing_us` separately
  from `animal_breeding_us`. Focused tests prove two sheep use one selected read
  plus one batch mutation. A fresh real-client run kept natural passive load
  visible for 1,020 client ticks, grew from 11 to 16 visible entities, and
  emitted no over-budget tick warning.

- The deep-water backlog came from fluid planning preserving every intermediate
  transition for the same position and repeatedly traversing the same
  unsupported-flow graph. Planning now visits each candidate position once,
  commits only the final state per block, and schedules follow-up ticks once per
  chunk. Ordinary resident fluid batches use the normal dirty-chunk save cadence
  instead of waiting for a journal `fsync` on the tick thread. The same loaded
  ocean that previously applied 94-105 edits indefinitely at 68-76 ms settled
  after nine O3 batches with at most 28 final edits: `fluid_tick` measured
  1,751 us p50 and 3,321 us max, and the real 26.1.2 client stayed connected
  through 600 client ticks. Evidence is
  `.analysis/codex-logs/scheduled-fluid-coalesced-o3-summary-20260723.json`.

- Repeated `minecraft_connect` calls no longer start competing login attempts.
  Calling it for the active address is an idempotent no-op; a different address
  is rejected until the caller explicitly disconnects. The original autonomous
  attempt had mistaken this duplicate-login lifecycle for respawn/navigation
  failure. On a fresh server, three same-address calls retained one session with
  no rejection or disconnect. A fast decision-agent rerun then found and mined
  a natural jungle log, crafted planks and a crafting table with normal
  container clicks, placed it on observed clear ground, and opened the crafting
  screen with health `20.0`.

- The autonomous survival run's apparent natural-zombie combat failure was in
  the MCP observation boundary, not server combat. The server log had committed
  health `20 -> 19`, but `minecraft_attack_entity_once` returned before the
  client applied the entity update. The tool now sends the player's exact
  rotation before the attack and waits on applied-state notifications for
  target damage or removal, with UUID/type and client-level fences. A fresh
  26.1.2 client attacked the same naturally spawned zombie without operator
  setup and returned `confirmed=true`, health `19 -> 18`. The dead-player path
  timed out as an error instead of producing a false success.

- Worldgen revision 8 passed an agent-run MCP route with a fresh 26.1.2 client,
  seed `918273645`, and `tellus_like` mode over forest, coast, ocean, and the
  representative high-relief range around `(-78080, -28928)`. The first route
  exposed low ridge masks becoming enormous
  flat gravel shelves. The corrected router keeps low shelves out of mountain
  biomes, strengthens the existing 720x280 rolling relief, adds anisotropic
  520x210 mountain detail, uses elevation-aware grass/gravel/snow surfaces, and
  explicitly keeps spawn dry. Numeric checks require visible local mountain
  relief without a five-block adjacent wall. A second agent-run MCP pass used
  the exact shipped `playable.toml` profile (`seed=0`, `tellus_like`) and found
  a dry, solid, moderately decorated forest spawn with raised tree crowns. Its
  focused harness then exposed leaf canopies being accepted as spawn support;
  spawn selection now rejects leaves, matching vanilla's no-leaves heightmap
  intent. The first run exposed a pending
  31-second far-travel chunk stream under dirty-cache flush pressure; that is a
  runtime latency item, not accepted worldgen performance evidence. The
  follow-up removed per-request full dirty flushes from production chunk
  preparation. Pressure now goes through the server-owned push worker with an
  exact completion ticket, while a changed stream generation cancels stale
  waiters. Three fresh O3 289-chunk windows completed in 3.195, 2.726, and
  2.608 seconds with 42-74 ms first-chunk latency and no client disconnect;
  direct shutdown also drained without timeout. This closes the 31-second
  far-travel regression, not the broader worldgen-quality or weak-machine
  targets.

- Fresh and legacy Solaris chunks now serialize the mandatory vanilla Anvil
  metadata at the codec boundary rather than in one generator. `DataVersion`
  defaults to the pinned 26.1.2 value, nonzero production ticks become
  `LastUpdate`, imported `InhabitedTime` is preserved, and each field is emitted
  exactly once through a real region write/read. Runtime now uses vanilla's
  strict 128-block
  chunk-center range around non-spectator players, counts each spawning chunk
  once per game tick, applies those counts in 20-tick mutation batches, and
  flushes chunks as they leave that range. Missing residents retain their delta
  for retry and shutdown loads them without generation. The value survives a
  real Anvil flush/reopen without adding per-tick chunk publication pressure.

- Every exact vanilla state now reaches its embedded collision shape in player
  movement instead of being bypassed by the old campfire/passable-name lists.
  Focused tests prove empty torch collision, the campfire's 7/16-block body,
  leather-boots support on powder snow, Shift descent, the long-fall 0.9F shape,
  authoritative teleport correction, and conservative fallback after a state
  fingerprint mismatch. Independent review found the fingerprint and direct
  correction-path gaps; both were fixed before the full gates.

- Shift-click batching is client-verified. One real 26.1.2 inventory-menu
  quick-move consumed four logs and produced 16 planks, and one crafting-table
  quick-move did the same, raising the existing plank stack from 16 to 32. Both
  crafting grids were empty after their transactions, and the crafting table
  remained empty after reopening. A chest quick-move transferred one 61-stone
  stack from player slot 54 to storage slot 0 and back to player slot 27;
  reopening the chest preserved that complete stack and empty storage slot.
  Every quick-move was confirmed and the client remained connected.

- Placement is client-verified through both hands and a side face. A real
  26.1.2 client placed stone upward from the main hand (`64 -> 63`), used the
  ordinary vanilla input path to place upward from the offhand (`63 -> 62`),
  then placed to the east side (`x+1`) from the main hand (`62 -> 61`). Focused
  adapter regressions route ordinary blocks through all six clicked faces and
  independently cover an offhand packet against an east face. The run remained
  connected. It also exposed the now-closed direct-use limitation below:
  `minecraft_use_item_on` returned `ok` for the offhand-only stack without
  changing its count or the target, while ordinary `use` input placed it.

- The direct client-MCP offhand gap is closed. `minecraft_use_item_on` accepts
  `main_hand` or `off_hand`, defaults to main hand, forwards the exact vanilla
  interaction hand, and returns the local interaction result. In the focused
  26.1.2 gate, stone was present only in offhand slot `40`, the selected main
  hand was empty, and `hand=off_hand` returned vanilla `Success`, placed stone,
  consumed the stack from `1` to `0`, and left the client in play.

- A real 26.1.2 MCP client closed the intermittent survival-break gate. It mined
  eight prepared stone blocks at `x=12..19`, crossing both sides of the chunk
  boundary at `15/16`. Every first
  ordinary break became air and exposed a visible item entity; the final
  inventory contained exactly eight cobblestone, health stayed at 20, and the
  client remained connected. The run also exposed a client-MCP defect:
  `minecraft_break_block` missed pickup into a non-selected slot despite the
  authoritative inventory update; that tooling issue is closed below.

- The client-MCP pickup defect from that run is closed. `minecraft_break_block`
  snapshots the total expected-item count before mining and, after the block
  becomes air, reacts to applied client state events instead of polling ticks
  or the selected stack; only an observed inventory-count increase completes
  pickup. In the focused 26.1.2 client gate, a stone world drop was
  observed and one cobblestone entered non-selected slot `1` while the diamond
  pickaxe remained selected in slot `0`; the command returned
  `pickup_confirmed=true` with `initial_count=0`, `inventory_count=1`, and the
  client remained in play.

- Rapid sequential mining no longer strands the second valid early `STOP`
  behind an existing delayed break. The stop is retained as queued work, and
  completion or cancellation of the older delayed break promotes it into the
  single delayed slot for event-driven completion. Focused coverage proves
  both transitions alongside the existing chunk-edge precondition regression;
  owner/client confirmation remains pending.

- Survival block loot remains a server-owned world item before pickup rather
  than a direct inventory credit. The focused TCP gate observed the committed
  block update/ack and tool durability update before `AddEntity` plus item
  metadata, kept the drop visible for at least 100 ms, and only then accepted
  one pickup command and emitted take/remove plus inventory slot updates. The
  current run processed one block edit and one item pickup with queue depth
  returning to zero.

- The optional `examples/plugins/geological-mines` plugin declares the
  `geological_deposits` startup ore profile. Prepared plugin discovery runs once
  before world validation and is reused to start Lua later. The profile removes
  the vanilla ore rules and generates deterministic elongated deposits larger
  than 512 connected blocks across chunk boundaries. World contract schema 2
  persists the profile under the current worldgen revision and rejects a later
  profile change; no declaration remains the vanilla default.

- The optional `examples/plugins/settlement-prototype` plugin declares the
  independent `plains_village_prototype` startup profile. The server loads an
  extracted vanilla plains fountain, small house, and toolsmith NBT, combines
  them at stable offsets, and consumes the extracted vanilla village
  spacing/separation/salt. Seed zero fixes the composite near spawn; other
  seeds use deterministic grassland placement. The settlement choice joins the
  persisted plugin worldgen profile fence, missing sidecar/templates fail
  startup, and Lua receives no generator or mutable-world handle. Local
  extracted-data tests prove all three templates resolve through the production
  embedded block registry and that two independent generators reproduce every
  block while differing from the no-structure baseline by more than 200 blocks.
  The bounded startup plan also selects per-building parts and roles, named
  inhabitants and jobs, and owner-prefixed extension records. Generation
  preserves the templates' vanilla villager jigsaw slots as chunk markers.
  Installing one of those chunks submits a dedicated system-owned simulation
  command, which idempotently materializes villagers with persisted
  plains/profession/level metadata and a durable claim distinct from ambient
  herd admission.

- Solaris Loader now has one protocol-1 manifest/ack contract across Fabric,
  NeoForge, and Forge. Plugin discovery validates bounded bundle descriptors
  for blocks, items, screens, assets, and interactions with exact permissions,
  SHA-256, size, and cache identity. The server sends a Configuration custom
  payload only when bundles exist and refuses Play until a compatible loader
  acknowledges every permission and cached bundle; ordinary servers retain the
  vanilla handshake. The shared Java validator now runs behind native
  Configuration payload registration for Fabric, NeoForge, and Forge, with
  platform codec tests covering all four Loader channels. Missing exact cache
  identities now use bounded request/artifact transfer; plugin startup verifies
  the source artifact, and the client stages, size/SHA-verifies, and atomically
  publishes it before acknowledgement. First contact with an exact server
  address and permission set now opens a Minecraft confirmation screen on all
  three platforms. The shared decision store keeps allow/deny choices
  server-scoped; denial produces no request or staging file. Actual content
  registration now begins with a closed archive index: verified and allowed
  bundles can atomically publish immutable owned screen, item, and first-slice
  block definitions, exact asset bytes, and screen-bound interactions on all
  three platforms before acknowledgement. Unknown fields or entries, asset
  mismatches, unverified cache files, and denied permissions fail before
  activation, and logout clears the
  registry before another connection can reuse the process. Host-attested Lua
  plugins can now open an activated owner-namespaced title/body screen through
  one ordered Play payload, but only for the exact player connection that
  acknowledged the Loader manifest. All three adapters fence queued UI work to
  that originating connection. Verified asset paths now mount as one transient
  required Minecraft client resource pack on all three platforms; ACK waits for
  exact-byte visibility after reload, and exact connection close removes the
  pack without allowing a stale close to clear a newer mount. Declared actions
  now render as bounded screen buttons on all three adapters and send one
  serverbound Play payload only while their exact definition and connection are
  current. Solaris requires the exact acknowledged session plus the owner's
  interaction permission and delivers a targeted `loader.interaction` event
  only to that Lua owner. Items now use verified owner item definitions through
  26.1.2 `ITEM_MODEL`. Up to eight owner blocks use a bounded carrier set
  registered before registry freeze on every platform; the mounted pack maps
  sorted owner identities to distinct blockstate/item models without vanilla
  substitution. The client reports the exact owner-id-to-runtime-state map in
  its ACK. The server cross-checks that map against the hash-verified artifacts,
  rejects missing, extra, invalid, or duplicate carrier states, and retains it
  only in the exact acknowledged Play session. Every canonical server-owned
  Loader block state projects to its own session carrier id in block updates and
  chunk palettes without sharing projected
  chunk frames between clients. That full opaque owner state is now registered
  in the server block and light tables before world open, so normal world
  storage persists it by name rather than by any client runtime id. The exact
  host-attested owner can place it through
  `solaris.place_loader_block`; the existing server-owned block transaction
  publishes the edit through each loaded session's projection. That exact owner
  can now grant the verified block presentation to an exact
  Loader-acknowledged player with `solaris.grant_loader_block_item`. The
  session owner merges the named `minecraft:paper` plus the owner's deterministic
  carrier `ITEM_MODEL` stack under the canonical inventory gate, persists before
  publication, and leaves a full inventory unchanged. `UseItemOn` recognizes
  that exact model only in the live session that acknowledged the block,
  resolves its canonical owner state, and
  reuses the ordinary survival placement transaction to conditionally commit
  the world edit and persist one-item consumption before publication.
  Wrong-model and unacknowledged stacks remain no-ops. Survival breaking that
  canonical state now emits the same named paper/`ITEM_MODEL` stack through the
  ordinary authoritative world-item path. Wire publication, entity persistence,
  partial claims, and simulation-owner pickup preserve those presentation
  components; a missing ACK or different state cannot select the Loader drop.
  This interaction slice has automated all-platform coverage. The 2026-07-24
  live-gate preflight originally found no isolated all-loader MCP launch path
  and no runnable two-owner fixture. The production-discovered
  `examples/loader-live-gate/` fixture is now runnable, and the Fabric MCP
  profile completed its real two-owner Configuration handshake through the
  ordinary permission screen before entering Play. Its two owner screens,
  display assets, grants, interaction returns, distinct world projections,
  placement consumption, survival drops, and exact named pickup returns are now
  client-verified. NeoForge now has the same client-verified Configuration,
  grant, placement, distinct projection, visual, survival-drop, and exact-pickup
  evidence. The Forge runtime profile and PrismLauncher gate remain not run, so
  the three-platform visual matrix is still incomplete.

- Default ore generation now embeds 18 separate vanilla 26.1.2 placement passes
  instead of nine merged family approximations. It preserves raw height anchors
  before world clipping, so diamond and lower-redstone trapezoids peak at
  `Y=-64`; rarity filters and uniform `0..1` counts retain fractional attempt
  density. Emerald and extra gold use the exact vanilla biome lists rather than
  broad terrain groups. The local extracted oracle matches all embedded
  anchors, placement kinds, counts/rarities, sizes, discard chances, targets,
  and scoped biome lists. Generated 9x9 chunk evidence keeps each family inside
  its bands, makes diamond/redstone bottom-heavy, and retains iron at ordinary
  branch-mining heights. Vein shape remains Solaris-owned deterministic
  connected geometry, not vanilla RNG.

- Worldgen rolling relief now uses a rotated 720x280-block field with weaker
  190-block detail, while continent, erosion, mountain and river authorities
  remain at 610-3,600-block scales. A behavior gate requires 128-block regional
  height change to dominate eight-block change. Actual generated grassland,
  forest and jungle vegetation remains present but below 12.5% of eligible
  columns, with separate tree/grass/flower density per biome. The embedded
  collision oracle verifies every state of generated and growable plants is not
  a full cube, and the runtime sampler reproduces an exact partial pitcher-crop
  shape. This is measurable coherence/collision evidence; the bounded
  Tellus/Tectonic visual gate is now closed by the revision-9 route below.

- A normal 26.1.2 client is now fenced from combat between respawn and its
  `ServerboundPlayerLoaded` acknowledgement, matching vanilla's load gate.
  While unloaded it remains simulated but cannot be selected or damaged; the
  acknowledgement republishes it as a combat target and reconciles hostile
  goals. Focused coverage proves both sides of that transition. The embedded
  MCP can also combine respawn with immediate movement and perform an exact
  ordinary block break with drop/pickup confirmation. On a fresh non-operator
  world the real client navigated to a jungle tree, broke and collected four
  logs, then used ordinary inventory clicks to craft planks and a crafting
  table; it remained alive at full health. This closes the observed idle-agent
  tooling blocker and the first gather/craft survival slice, not the pending
  20-minute autonomous session.

- Embedded MCP launches now prefer the current run's environment token and
  port over stale JVM properties and check for an existing listener before
  starting another client. The earlier 401 was not reproduced under a fixed
  token, so this removes two launch ambiguities rather than claiming a proven
  server-side auth race. A patched real 26.1.2 client completed 200 fresh MCP
  sessions, 180 observations, and 20 one-tick forward inputs with zero failures
  or 401s, moved from z=0.598 to z=9.956, and remained in play at full health.
  The current-build 20-minute autonomous survival session remains pending.

- Worldgen revision 10 removes the production-only spawn fixtures inherited by
  revision 9: no fixed tree, stone, surface iron, dry-origin blend, mountain
  suppression, or river suppression remains. A deterministic bounded locator
  searches the actual seeded terrain for dry low-relief land; schema-3
  `solaris/world.json` persists that block position, startup generation and light
  center on its chunk, and fresh-player support/body-space search consumes the
  same `WorldSpawn`. Vegetation now combines routed moisture with one seed-driven
  192-block density field; exact column hashes place trees/plants only inside those
  coherent patches. Savannas receive sparse acacia, deserts/snowy plains/ice spikes
  remain treeless, and taiga/grove retain spruce. A 32-seed Tellus regression
  includes `0`, `712816`, and `-1`, samples 8192x8192 around each selected spawn,
  requires unique biome/feature fingerprints and bounds repeated >90% single-biome
  land dominance. `example.toml` and the config default are `tellus_like`;
  `playable.toml` uses `.analysis/test-world-v10`. `mc-worldgen` is 110/110 and the
  external worldgen harness is 5/5. A fresh graphical revision-10 inspection,
  2048x2048 height/biome/vegetation mosaics, coarse drainage, seed-`712816` owner
  playtest, restart, and release-host throughput comparison remain pending.

- Hostile melee now keeps a zero-speed target-facing goal while in reach, so a
  stationary zombie stops without freezing its body/head rotation and publishes
  the corrected facing to observers. Hostile ticks now read a dedicated active-
  hostile publication plus stable per-session pose/visibility snapshots and
  perform creeper, skeleton, and melee owner work on regional lanes without a
  global registry read. Final melee publication reads per-session
  immutable combat-target and visibility snapshots, rechecks target life, pose,
  vertical reach, and range, then reserves ordered output only while a shared
  target/visibility epoch remains unchanged and even. It never reacquires the
  global session registry. Focused tests cover an unmoving player, facing,
  attacker/target death, movement out of range, Spectator transition,
  unregister-after-snapshot, and completion while another thread deliberately
  holds the session registry; a whole ordinary melee tick is covered by the same
  lock exclusion. Regional selection publishes current loaded hostiles before
  attacks on each goal turn; unload and last-player disconnect clear that input
  without a later owner read. The existing TCP survival zombie damage/kill/drop
  test also remains green. A real-client feel check remains pending.

- A fresh isolated O3 server and real 26.1.2 client completed the hostile-mob
  functional gate through embedded MCP. The client selected an iron sword,
  approached zombie `1000079`, killed it, saw the rotten-flesh drop, and
  restored the pickup. Skeleton `1000082` published arrow `1000083` and reduced
  player health from `13.833334` to `10.333335`. Creeper `1000084` reduced
  health from `10.333335` to `9.333335` and disappeared, consistent with the
  explosion path already proved by the TCP regression; the client remained
  `in_play=true`. The server recorded one `57.474 ms` over-budget tick after
  processing 62 simulation commands with 10 still queued, with zero reliable
  drops, retries, or disconnect warnings. Evidence harness and results:
  `.analysis/mcp-combat-check.py`,
  `.analysis/codex-logs/mcp-hostile-combat-result-v2.json` and
  `.analysis/codex-logs/mcp-hostile-combat-server-v2.log`. This was an
  agent-run deterministic functional gate; subjective animation/feel and a
  natural no-operator survival run remain unproven.

- Water diagnosis now uses structured MCP state rather than screenshots. The
  26.1.2 client exposes fluid tags/type/height/collision, player water flags,
  pose bounds and loaded-chunk state. Solaris sends configuration packet `0x0c`
  with `minecraft:vanilla` before known-pack negotiation, matching the local
  26.1.2 protocol registration and server ordering. Kelp, kelp plants,
  seagrass, tall seagrass and bubble columns are passable instead of falling
  through the unknown-shape full-cube fallback. Swimming/crouching/standing
  body and eye heights now share one pose contract, so one-block water can
  submerge a swimming player's eyes. Focused Rust and client Gradle gates pass,
  and an O3 real client entered the ocean without the prior kelp correction.
  The zero-contact cause was the second section counter: Solaris encoded
  `fluid_count=0`, so 26.1.2 `LevelChunkSection.hasFluid()` prevented
  `EntityFluidInteraction` from scanning otherwise-correct water states. The
  encoder now counts water, lava, water plants and `waterlogged=true` states.
  In the O3 real-client rerun, entering a source block produced
  `in_water=true`, `water_fluid_height=0.8888889`, and no disconnect while 81
  chunks streamed with `chunk_data_ms=0`. Evidence is in
  `.analysis/codex-logs/fluid-count-real-client.json`. A second O3 real-client
  run observed ascent, diving, horizontal movement with the swimming pose, air
  depletion and drowning damage while remaining connected. Evidence is in
  `.analysis/codex-logs/deep-water-real-client-final.json`. Client-local fluid
  contact, movement and breathing are green for this representative survival
  path; broader aquatic mechanics remain normal parity work.

- The exact dense-world rerun reproduced the final disconnect as an unanswered
  keepalive challenge while the client was still sending valid movement. The
  tracker no longer replaces an unanswered challenge and only closes when both
  the challenge and all inbound client activity exceed the deadline. Ordinary
  entities use vanilla's default three-tick tracking interval; above 512
  candidates a rotating shard bounds each tracking turn, while arrows, items,
  and experience orbs remain latency-sensitive. On the O3 5,132-cow fixture
  (5,227 total entities), a real 26.1.2 client completed 720 movement ticks and
  255 additional ticks, remained `in_play=true`, and recorded zero keepalive
  mismatches, timeout closes, reliable drops, or retries. Evidence is
  `.analysis/codex-logs/dense-5132-spawn.json`,
  `.analysis/codex-logs/dense-5132-release-build-v5.log`,
  `.analysis/codex-logs/dense-5132-keepalive-fixed-v5.json`, and
  `.analysis/codex-logs/dense-5132-fixed-v5-server.log`.
- A current-head O3 rerun addressed the dense world's remaining latency rather
  than only its keepalive symptom. With 5,208 active cows, AI/physics now uses
  autoscaler-sized deterministic cohorts, natural movement publication rotates
  a bounded entity-id cohort, sheep grazing reads an exact sheep index, and
  breeding reads only indexed babies/animals in love while retaining the full
  active population. Ordinary populations below the limits retain per-tick
  simulation. The same 975-client-tick gate reduced over-budget warnings from
  223 to 8; conditional warning p50 changed from 93.123 ms to 56.775 ms,
  entity-goal p50 from 78.237 ms to 17.473 ms, and grazing p50 from 8.389 ms to
  0.234 ms. The 26.1.2 client stayed in play with no disconnect, reliable drop,
  retry, or runtime work-budget info spam; the only CPU-admission info
  transition was the requested shutdown drain. Evidence:
  `.analysis/codex-logs/natural-load-current-o3-server.log`,
  `.analysis/codex-logs/natural-load-indexed-breeding-o3-server.log`, and
  `.analysis/codex-logs/natural-load-indexed-breeding-o3-client-gate.json`.
  This remains an artificial overcrowding overload gate. The complementary
  representative natural-load interaction gate is now also green: a fresh
  agent-run 26.1.2 `playable-12` gate on the optimized dev profile broke three
  natural logs with visible drops, used maximum-count inventory crafting,
  placed/opened a crafting table, killed and collected the drop from a natural
  pig, placed/opened a chest, and transferred the food through the normal
  container path in 22 seconds. Nine natural entities were present and new
  chunk rings streamed during the run. The client stayed in play and the server
  emitted no tick-budget, packet-dispatch, reliable-command, or disconnect
  warning. This is a representative ordinary-play responsiveness smoke gate,
  not a per-action latency SLO or broad overload soak. Evidence is
  `.analysis/real-client-runs/responsiveness-o3/20260723T103459Z-real-client-playable-loop-4vVxYV`.

- The shipped `basic-economy` plugin now uses one configured physical item as
  currency and opens its server-owned inventory shop on zone entry or
  `/economy`. Purchase and refund each mutate currency, product inventory, and
  the durable refund ledger in one transaction. The old virtual wallet and
  duplicate `currency-catalog` fixture were removed. The production TCP/Lua
  gate customizes gold ingots and a stone axe, then proves zone opening,
  purchase, insufficient-funds rejection, refund, and refreshed menu state.
  Stable product ids retain the original refund terms across catalog changes;
  changed terms block new purchases until old purchases are refunded.
  The shipped `land-claims` plugin remains a bounded durable whole-chunk index.
  Direct break/place, right-click block actions, containers, buckets,
  living-entity interaction, explosion block damage, and bounded random-fire
  burns into common fuel are protected. Direct lever/button power now extends
  and retracts one normal piston with one common full block; its atomic
  base/head/destination mutation group consumes the immutable protection
  snapshot in direct and scheduled-button paths. No manual-client gate was run.

- Land claims now deny foreign right-click block actions, cross-boundary filled
  bucket placement, double-container access, living-entity interaction at the
  authoritative target position, and explosion block damage. Every
  chest/furnace click rechecks its backing positions. Explosion planning uses
  one immutable generic protection snapshot only after an explosion becomes due and before
  the world lock, not one zone lock per candidate or one clone per idle tick.
  Random fire planning consumes the same immutable snapshot and rejects a
  protected adjacent burn without freezing the source fire lifecycle.
  The production wire gate proves foreign break, placement, and filled-bucket
  placement leave the claimed world unchanged. The focused fire regressions
  prove one bounded common-fuel spread and protected-target rejection. Focused
  piston regressions prove direct one-block extension/retraction and protected
  rejection through the scheduled owner path. This is a claims baseline, not
  sticky-piston, multi-block, slime/honey, or moving-animation parity.

- Land-claim semantics are no longer embedded in Rust. The generic
  `solaris.upsert_protected_zone` API carries an actor-or-operator policy in the
  typed zone DTO; the zone adapter does not match plugin ids or parse zone ids.
  `examples/plugins/land-claims/main.lua` owns claim identity, persistence,
  registration, removal, and rollback.

- Production Lua mutation DTOs expose no locks, region keys, leases, epochs, or
  worker handles. Entity spawn and villager commands route through simulation
  or regional owners; menu, teleport, and standalone player-inventory commands
  route through the target session's ordered lane. Player-inventory routing now
  waits for that exact owner to plan against live state and update its durable
  mirror instead of mutating persistence from the script router. A dropped
  owner command returns `player_unavailable` without mutation. The typed
  inventory/storage purchase coordinator shares the internal session gate with
  standalone owner commands, so planning cannot overtake an earlier owner
  mutation and its durable ledger and inventory must commit together.

- Mob death completion no longer scans every server entity every tick. Lethal
  melee, projectile/effect damage, test ingress, and persisted restore enqueue
  exact retained deadlines; the tick path drains four due deaths per tick so a
  mass-death spike cannot monopolize one tick. An explicit `-O3` benchmark with
  4,096 cows measured idle death ticks at 7 us p50 / 11 us p99, sustained
  four-kill batches at 11,664 us p50 / 13,668 us p99, and four-removal batches
  at 10,147 us p50 / 24,367 us p99. Focused tests cover an empty index,
  multi-tick backlog, lethal effects, arrows, normal death timing, and restart
  reconstruction. This is in-process owner/publication evidence, not real
  socket throughput or manual combat feel.

- Primed TNT and creeper fuses now enter an exact deadline index instead of
  scanning every server entity each tick. Spawn, cancellation, rescheduling,
  persisted restore, and entity removal maintain that index without stale queue
  entries. The explosion owner claims at most one due explosion per world tick,
  including repeated owner calls, so ray planning, world edits, drops, entity
  impacts, and ordered publication cannot combine an arbitrary simultaneous
  batch under one world lock. The explicit O3 full-path benchmark used 4,096
  background cows, 64 due explosions, a fresh 27-block solid dirt volume per
  explosion, and one loaded observer. Idle fuse checks measured 0 us p50/p99;
  the 64 bounded explosion ticks measured 23,812 us p50, 37,943 us p95, and
  46,463 us p99/max. This is an in-process release-build authority/world/entity
  envelope, not publication or real socket throughput.

- Reach now uses the 26.1.2 server contracts instead of one shared
  center-distance rule. Block checks measure eye-to-block AABB at 5.5 survival
  or 6 creative with a strict boundary; entity use measures eye-to-entity AABB
  at 6 or 8 with a strict boundary; default main-hand melee uses the same 6 or
  8 packet envelope with the inclusive `AttackRange` boundary. Authoritative
  held spears use their 4.5/6.5 reach and 0.125 hitbox margin; crouching and
  swimming use their pose-specific eye height and target box. Non-finite
  coordinates fail closed. Focused tests cover exact boundaries, the
  previously rejected 5-to-6-block melee buffer, far rejection, direct mob
  damage/death, death timing, and skeleton owner requests; the full `mc-net`
  suite passes. A manual client gate remains pending.

- Creepers no longer enter generic melee authority. A visible survival target
  within three blocks starts one retained 30-tick fuse; leaving seven blocks
  away reverses its progress, and expiry removes the creeper and uses the shared ordered
  explosion path at power 3. Swelling stops navigation, dying creepers cannot
  reach fuse expiry, and natural swell is not persisted across restart.
  Source-specific explosion centers/power preserve
  TNT power 4, and chained TNT now always uses the canonical TNT entity type
  instead of inheriting the source type. Focused unit tests cover start,
  no-restart, cancellation, exclusive trigger boundary, retained air state,
  terminal removal, and packet
  planning. Real TCP tests prove creeper spawn -> removal -> radius-3 explosion
  -> player damage and preserve the existing skeleton-arrow and TNT paths.
  Client swell/ignited wire indexes and line-of-sight cancellation remain
  pending exact integration evidence; no manual-client gate was run for this
  slice.

- Fresh-player spawn now scans the already-resident 11x11 spawn window and
  chooses the nearest non-hazardous collidable support with collision-free,
  non-fluid body space. Missing world data still uses the previous origin
  fallback. Focused regressions cover origin water, transparent collidable body
  space, and magma support; all `mc-net` tests, fmt, and code-health pass. On
  the same generated seed `20260721` that previously reported
  `block_below_player=minecraft:water`, a new 26.1.2 client reached play with
  `block_below_player=minecraft:air` and 56 visible entities. That observation
  proves only that the initial sampled cell was no longer water; the focused
  server tests prove the selected support. The final tested O3 binary SHA-256
  is `6be274ad51f43129e4949ad2a5eea39444d50d580bd694f5340e300b59b105d9`.
- The current O3 binary (`f299a01c1dd281cf6cb82b587b40390be2a35a8f294de32f199f45048d0fb60f`)
  passed a short embedded-MCP real-client
  gate on an isolated fresh world: the 26.1.2 client joined, reached play,
  loaded blocks, observed 53 entities, and accepted a forward-input request.
  Server logs contained no slow-tick, autoscale, reliable-drop, or disconnect
  warning. This proves the ordinary small-world path only; it does not replace
  the pending 5,132-entity owner-world rerun and does not establish binary
  provenance from a clean tracked tree. The same gate found the player
  standing over `minecraft:water` on fresh seed `20260721`, directly confirming
  dry-land spawn selection as the next reproducible playable checkpoint.
- This checkpoint fixes two concrete hot-path faults from the owner's dense
  5,132-entity O3 run. Autoscale recovery now requires 20% tick headroom, so
  50-57 ms boundary jitter cannot alternate `ScaleDown` and `ScaleUp`.
  Per-tick `Hold` and capacity-capped actions no longer synchronously
  reconfigure regional owner lanes or invalidate their read routes. Continuous
  slow-tick warnings emit on episode entry and then every 100 ticks, not every
  tick. Direct tests preserve memory shedding and drain-to-one behavior; all
  1,600 `mc-net` tests and all workspace L2 gates pass. The independent review found the
  missing application-path coverage, which was added. This removes observed
  autoscaler churn; the later exact dense-world gate above closes the separate
  periodic disconnect.
- This checkpoint adds the ordinary water survival path. Player eye immersion
  now consumes the vanilla 300-tick air supply, publishes metadata index 1,
  deals two drowning damage at the vanilla `-20` boundary, and recovers four
  air per tick outside water or in invulnerable modes. Swimming publishes the
  shared entity flag `0x10`. Aquatic entity queries use fish water drag without
  generic buoyancy, so canonical 26.1.2 aquatic and amphibious mobs are no
  longer driven to the surface by the shared body kernel. Focused breathing,
  metadata, classification, and sampled-water
  physics regressions and full workspace tests, strict Clippy, fmt, and
  code-health pass. The independent reviewer found incomplete aquatic class
  coverage, Adventure immunity, respawn air carryover, and a rejected-commit
  damage loss; all four were fixed. Owner-client verification is pending.
- The owner-run O3 build exposed a server-triggered disconnect while loading a
  dense world: 5,132 entities produced 5,702 per-entity spawn dispatches and
  overflowed the bounded reliable queue (`reliable_command_drops=963`). Chunk
  visibility now publishes one ordered spawn batch per loaded chunk, pauses
  further chunk emission at outbound pressure, and writes at most 16 entity
  spawns per play-loop turn so keepalive and timeout boundaries keep making
  progress. The 17-entity/channel-capacity-1 regression passes with zero drops
  and exact entity accounting; all 1,589 `mc-net` tests and three doc tests
  pass. A `sol high` reviewer confirmed ordering and state-loss behavior and
  requested the bounded write turns, which were added. Owner-client rerun on
  that earlier binary was superseded by the movement-backlog checkpoint above.
- Checkpoint `7cdd917` fixes a normal active-game save conflict found by the
  natural furnace scenario. The first artifact
  `.analysis/real-client-runs/20260721T110747Z-real-client-playable-loop-yFIIqx`
  completed birch -> table -> wooden pickaxe -> cobblestone -> furnace ->
  charcoal, but the runner exited 1 after repeated false `region changed before
  replace` dirty-flush warnings. The resident chunk had changed while its whole
  Anvil region encoded outside the world lock; this is not a filesystem CAS
  failure. The normal flush now skips that region before disk installation,
  keeps it dirty for bounded replanning, and continues stable regions. The
  second artifact
  `.analysis/real-client-runs/20260721T112014Z-real-client-playable-loop-pURskM`
  passed the same no-debug natural loop with runner exit 0, no dirty-flush or
  pressure-flush warning, and periodic saves draining to zero dirty chunks.
  The observed warned tick peak was about 55 ms. Full workspace tests, strict
  workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass.
  The rare partial-install `fsync`/counting debt remains deferred. This does
  not replace the pending owner-played 20-minute session or a vanilla oracle.
- Checkpoint `5e0d93b` adds bounded host-local Lua timers driven by pushed
  monotonic simulation ticks. Queue pressure coalesces the newest tick instead
  of blocking the simulation thread or requiring plugin polling. Timer
  callbacks are ordered, capped at eight per pushed tick, and share one command
  and instruction budget with `on_server_tick`. Focused tests cover replacement,
  cancellation, invalid input, exact capacity, stale/coalesced ticks, handler
  rollback, same-tick cancellation, close/drain, and shared-budget failure. A
  real TCP/Lua gate proves a player command schedules a timer and receives its
  targeted result without subscribing to `server.tick`. Full workspace tests,
  strict workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check
  pass. A `sol high` re-review found no blocker/high/medium issue. No
  manual-client or vanilla-oracle gate was run for this plugin-only slice.
- Checkpoint `d59bd57` adds optional bounded `config.toml` snapshots to Lua API
  `0.6.0`. Configuration is validated before command ownership, read once, and
  returned as a fresh recursive Lua table. The then-separate currency catalog
  took currency, zone, and products from operator configuration. Its real
  TCP/Lua gate overrides all three and proves exact menu content, purchase,
  stale rejection without mutation, and refund. Full workspace tests, strict
  workspace Clippy, fmt, code-health `0 fail / KEEP`, and diff-check pass. A
  `sol high` re-review found no blocker/high/medium issue. No manual-client or
  vanilla-oracle gate was run for this plugin-only slice.
- P02 real-client artifact
  `.analysis/real-client-runs/20260721T095305Z-real-client-playable-loop-hXlAv8`
  passes natural birch breaking with visible progress, drop and pickup, then
  crafts twelve planks, a table, sticks, and a wooden pickaxe and opens/closes
  the table. It used the Gradle client adapter without debug grants. The server
  emitted sub-500 ms tick-budget warnings in `animal_breeding`, peaking at 133
  ms, but the scenario had no client-visible failure. This does not replace the
  pending owner-played 20-minute session or broad performance evidence.
- Checkpoint `9aee245` adds production Lua player-inventory transactions for
  atomic grants and exchanges over the connected player's main inventory and
  hotbar. Planning precedes canonical state replacement, so unknown resources,
  insufficient input, full output, stale/disconnected sessions, and worldless
  runtimes do not partially mutate inventory. Results are correlated and
  targeted to the issuing plugin. The real TCP/Lua gate proves grant, exchange,
  two rejected mixed transactions, unchanged state after each rejection,
  targeted non-leak, and a worldless `runtime_unavailable` rejection without an
  inventory packet. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found
  no remaining blocker/high/medium issue. No manual-client or vanilla-oracle
  gate was run for this plugin-only slice.
- Checkpoint `c82c344` adds production Lua same-dimension player teleports
  through the exact reliable session and authoritative simulation owner. The
  result distinguishes unavailable/stale players, an outstanding vanilla
  teleport confirmation, and runtime failure; success cannot become failure if
  the session waiter is cancelled after owner commit. The real TCP/Lua gate
  proves pending rejection, exact cross-chunk position and center publication,
  zone observation, targeted non-leak, repeated pending rejection, and the
  authoritative follow-up pose. A direct queue test proves the post-commit
  cancellation case. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found
  no blocker. No manual-client or vanilla-oracle gate was run for this
  plugin-only slice.
- Checkpoint `0969620` completes the production Lua zone membership lifecycle
  with owner-targeted `player.zone_exited` snapshots. Accepted absolute movement
  publishes deterministic exits before entries with the authoritative new pose;
  stale, rejected, no-op, disconnect, and zone-removal paths publish nothing.
  The real TCP/Lua gate proves pre-teleport movement rejection, entry, exit,
  outside no-op, re-entry, and isolation from another subscribed plugin through
  exact pushed chat fences. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` review found no
  blocker; its rejected-movement finding was added. No manual-client or vanilla
  oracle gate was run for this plugin-only slice.
- Checkpoint `d9c0804` replaces the narrow furnace-fuel matcher with the exact
  default-feature 26.1.2 builder order over resolved item tags plus a complete
  repo-owned 280-item fallback. Sidecar startup rejects membership or duration
  drift instead of silently accepting a partial fuel graph. Furnace, smoker,
  blast-furnace, menu, quick-move/swap/pickup, and hopper paths share the same
  immutable snapshot; smoker and blast-furnace durations are halved, while
  crimson/warped wood is removed after all additions. The local decompiled
  `FuelValues` oracle and full sidecar match the fallback for all 280 ids and
  durations. The real TCP container gate smelts iron with oak stairs; focused
  tests also prove accepted hopper transfer and mutation-free rejected menu and
  hopper transfers. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. A `sol high` review found a
  partial-sidecar acceptance gap and missing transaction sad paths; both were
  fixed and the re-review found no blocker. No manual Prism-client gate was run.
- Checkpoint `99b9879` adds production `player.entity_interacted` Lua events for
  authoritative reachable right-click gestures against alive server-owned
  living entities. The exact event carries actor identity/pose/mode, target
  id/type, hand, and secondary-action state; it does not claim a vanilla side
  effect. Missing, far, nonliving, dying, Spectator, and dead-actor paths emit
  nothing. Vanilla feed/shear handling and client writes complete before
  required Lua admission can wait, and write errors retain immediate cleanup.
  The production TCP/Lua gate proves rejected far/missing attempts followed by
  exact off-hand/secondary and main-hand events without a quiet-window success
  condition. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. A `sol high` review found and verified
  the delivery-order fixes. No manual-client or vanilla-oracle gate was run for
  this plugin-only slice.
- Checkpoint `aabea52` adds the production `player.entity_killed` Lua event for
  exact direct player-melee kills. It publishes only after target lethality and
  attacker costs commit, captures the transaction pose, and does not attribute
  nonlethal, unreachable, stale-cost, repeated-dying, projectile, explosion,
  environmental, or non-player damage. The real TCP/Lua gate observes two
  distinct kills from the same committed-event FIFO and proves no delayed
  duplicate; direct tests also cover a moved session snapshot and closed
  outbox. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. One overloaded workspace attempt exposed
  unrelated existing probe/TNT timing failures; each focused rerun passed and a
  clean single workspace run passed. A `sol high` review found no blocker; the
  stale-position finding was fixed, while global cross-producer script ordering
  remains explicitly outside the outbox FIFO contract. No manual/client or
  vanilla-oracle gate was run for this plugin-only slice.
- Checkpoint `e09c6ec` replaces smoke-only confidence in the shipped Lua
  examples with production wire evidence. The then-shipped currency catalog
  files passed zone activation, rendered menu contents, an atomic three-emerald/two-
  apple purchase, insufficient-funds rejection with unchanged ledger, and a
  refund. The exact colony scaffold files now pass `/colony recruit worker`,
  durable activation, initial `home`, a later owner-accepted `hold`, and status
  reload. The gate then removes the bound villager, proves cached-token owner
  rejection, one fresh binding attempt, and the explicit no-villager result.
  Plugin-emitted readiness messages and exact combat-cooldown tick events are
  push barriers; no elapsed-time success condition is used. Full workspace
  tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`, diff-check,
  focused plugin `2/2`, colony router `17/17`, and exact loader `1/1` pass. A
  final `sol high` review found no blocker, high, or medium issue. This is
  server/wire plugin evidence, not a manual-client or vanilla-oracle gate.
- Checkpoint `9330336` adds required `player.died` events at the authoritative
  live-to-dead survival commit. Operator, fall, starvation, contact, hostile,
  PvP, and projectile paths converge there; nonlethal, shield-blocked, stale,
  unsupported-mode, already-dead, and respawn paths publish nothing. The owner
  snapshots the event before fallible client writes, shutdown drains all
  producers and this outbox before required `server.stopping`, and Lua admission
  remains bounded and push-driven. The owner outbox is intentionally unbounded
  because its synchronous commit cannot await capacity while holding state
  locks; revisit that debt only if measured workloads make it material. A real
  packet/Lua gate observes health zero, one death event, respawn, and no
  duplicate. Its prior pickup timeout was a genuine two-client race in the test:
  both clients shared spawn and could claim the same dirt. Causally fenced peer
  movement now isolates the collector without longer timeouts. Full workspace
  tests, strict workspace Clippy, fmt, code-health `0 fail / KEEP`, and
  diff-check pass; `commands` passes `14/14` and `mc-server --test play` passes
  `16/16`. A final `sol high` review found no blocker, high, or medium issue. No
  manual/client or vanilla-oracle gate was run for this plugin-only event slice.
- Checkpoint `51b2659` adds required post-commit `player.item_picked_up`
  events for exact item-entity and grounded-arrow inventory credits. Partial
  pickup reports only the merged count. Stationary drops now wake nearby
  sessions from an exact simulation-tick readiness index instead of depending
  on entity movement or polling. A regression found by the full workspace gate
  removes duplicate candidate publication when physics and readiness meet on
  the transition tick. `sol high` review found that deferred campfire outputs
  entered the index before journal acknowledgement; the index now activates
  only after entity publication, and a focused regression proves hidden
  outputs cannot be picked up. Full workspace tests, strict workspace Clippy,
  fmt, code-health `0 fail / KEEP`, and diff-check pass. The command wire gate
  passes `14/14`; no manual/client or vanilla-oracle gate was run for this
  plugin event slice.
- The current plugin slice adds post-commit `player.item_crafted` events for
  2x2 inventory crafting, 3x3 crafting-table result clicks, and recipe-book
  crafting. Max crafting reports one aggregate output/count pair. Direct tests
  prove stale owner state, mismatched/no-op clicks, missing ingredients, and
  unsupported game modes publish nothing before pushed FIFO fences. The real
  packet/Lua gate observes an inventory commit of two oak logs into eight
  planks and one exact `craft_count = 2` event, then proves a missing-input
  retry emits no event. Focused `mc-script`, `mc-net`, and wire tests pass.
  Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check pass. A `sol high` re-review found no
  remaining blocker, high, or medium issue. No manual/client or vanilla-oracle
  gate was run for this plugin-only event slice.
- The current plugin slice adds required post-commit `player.block_placed`
  events for the actual registry-backed root state. The shared real packet/Lua
  gate observes creative and survival commits and FIFO-fences blocked and
  empty-hand retries. A direct owner-stale stair dependency test proves no
  placement event before a pushed `server.tick` fence. Focused contract,
  adapter, and wire tests pass. Full workspace tests, strict workspace Clippy,
  fmt, code-health `0 fail / KEEP`, and diff-check pass. A `sol high` re-review
  found no remaining blocker, high, or medium issue. No manual/client or
  vanilla-oracle gate was run for this plugin-only event slice.
- The current production plugin slice publishes `player.block_broken` only
  after an authoritative root block transition. A real packet/Lua wire gate
  observes exact creative and survival events. FIFO command fences prove abort,
  repeated-air attempts after both modes, and a two-client owner-stale survival
  completion publish nothing. Focused `mc-script`, `mc-net`, and wire tests
  pass. Full workspace tests, strict workspace Clippy, fmt, code-health
  `0 fail / KEEP`, and diff-check also pass.
- Checkpoint `5ea197b` makes the active save install its exact simulation
  barrier snapshot and makes accepted PvP attacks observable from the
  simulation owner. Full workspace tests, strict workspace Clippy, fmt,
  code-health `0 fail / KEEP`, and diff-check pass. The broad `block_edit` gate
  passes `94/94`.
- Real-client artifact
  `.analysis/real-client-runs/20260720T234001Z-real-client-playable-loop-QHfB9Z`
  passed natural log breaking with visible progress, drop pickup, and log-to-
  planks crafting through the real Gradle client. With no new common blocker in
  that focused loop, work moved to the production plugin API as required by the
  queue above.
- Checkpoint `feba79a` passes full workspace tests, workspace all-target strict
  Clippy, fmt, code-health `0 fail / KEEP`, and diff-check. The `block_edit`
  target also passes both parallel and sequential runs with 94/94 tests.
- Ordinary wall torches have registry-backed tests for four horizontal facings,
  standing `UP`, rejected `DOWN`, and partial support. Raw TCP proves one debit
  after accepted update/ack and unchanged held-stack resync before rejected ack.
- Stair facing/half, slab top/bottom, matching-slab merge, waterlogging, and
  stair neighbour-shape recomputation use the inspected local 26.1.2 rule.
  Selector and real placement/break adapter tests cover every corner shape and
  stale dependency rollback; a dedicated raw-TCP corner assertion remains open.
- The regional mutation extraction is architecture-only and makes no gameplay
  or performance claim.
- Latest P44 artifact is
  `.analysis/real-client-runs/20260720T120018Z-real-client-playable-loop-UJtsgc`.
  Sheep and chicken passed, including chicken yaw. The selected cow moved 2.69
  blocks on a flat Y=78 surface but never encountered a rise, so the scenario
  failed its 0.8-block climb condition. The prior artifact
  `.analysis/real-client-runs/20260718T111008Z-m94-regression-pack-4X1iF7`
  already observed a real-client cow climb of 1.0 block. P44 therefore needs a
  deterministic climb candidate before another run can be strong evidence.
- The unrestricted P44 run exposed a 3.47-second entity-journal checkpoint
  stall. Entity checkpoint acknowledgement is now memory-only after the durable
  world checkpoint: older append-only WAL records are filtered by the saved
  lifecycle/sequence watermark, and physical compaction runs on normal journal
  shutdown. A production-journal regression proves the next mutation queues an
  `Append`, not a checkpoint `Replace`; crash-before-compaction replay and
  shutdown compaction both have focused coverage. This is not a broad
  performance claim.
- The PvP wire oracle now waits on an accepted attack event published by the
  simulation owner instead of assuming TCP ingress lands on a chosen tick. The
  event separates cooldown and hurt-resistance clocks and carries owner-order
  sequence and attacker identity. Focused wire runs, the authority-clock unit,
  the reciprocal concurrent-owner test, full `mc-net`, and `sol high` review
  pass. The first broad `block_edit` run found the separate persistence failure
  below, so this is focused PvP evidence rather than a broad gate claim.
- The persistence barrier defect found by broad validation is fixed locally.
  Background flush now validates the resident region before install and
  replans stale work. Active save installs its exact owner-barrier snapshot,
  leaves post-barrier mutations dirty, and waits for the exact journal-fence
  release before recapturing an incomplete snapshot. Final post-drain save
  rejects an orphaned fence instead of retrying a fixed number of times or
  acknowledging it. Focused `mc-world` dirty-flush tests pass `20/20`, active
  save tests pass `4/4`, the orphan-fence sad path passes, and the parallel
  `mc-net` passes `1534/1534` runnable tests, and a fresh parallel `block_edit`
  gate passes `94/94`, including restart persistence and in-flight campfire
  state. Full workspace passes. A second independent `sol high` review found no
  blocker or high-severity issue. It found only a rare multi-region recovery
  path that can `fsync` an installed prefix under the world mutex and omit that
  prefix from aggregate metrics; this is explicitly deferred behind common
  gameplay and production plugin work.
- P47 artifact
  `.analysis/real-client-runs/20260720T122329Z-real-client-playable-loop-Dbzfoj`
  passed stonecutter placement, menu open, normal take (1 input to 2 slabs),
  close/reopen conservation, and shift-click (3 inputs to 6 slabs). The outer
  runner returned degraded because startup emitted 350 ms and 52 ms slow-tick
  warnings; the client scenario itself exited 0. Its setup used three
  `giveAndSelect` debug commands, so this is real-client wire/menu evidence,
  not earned-survival gameplay evidence. Earned setup and rejected invalid
  input remain open.
- P48 artifact
  `.analysis/real-client-runs/20260720T124754Z-real-client-playable-loop-l8eWbc`
  passed the no-debug, no-op real-client building scenario. The client earned
  wood, stone, a furnace, charcoal, torches, and matching planks; crafted
  stairs and slabs; then proved wall-torch facing, stair facing/half, bottom
  and top slabs, matching-slab merge, exact inventory debits, and rejected
  bottom-slab wall-torch support without a debit. The scenario and driver
  exited successfully and produced a valid screenshot. The outer gate remains
  degraded because `server.log` contains slow-tick warnings, including one
  342 ms entity-physics tick and one 271 ms entity-goals tick, so this is
  gameplay evidence rather than a clean combined gameplay/performance gate.
- P04 artifact
  `.analysis/real-client-runs/20260720T143912Z-real-client-playable-loop-rjWZVp`
  passed the full no-debug `24,000`-tick continuity gate. The real Gradle
  client gathered natural birch, observed break progress and drops, crafted a
  table, sticks, wooden pickaxe, and wooden sword, completed 27 later resource
  cycles, then survived a clean server stop/restart and proved the table and
  pickaxe persisted after rejoin. The continuity profile disables only natural
  hostile spawning; manual play and combat scenarios keep it enabled. Earlier
  real-client evidence separately proved wooden-sword zombie and skeleton
  combat. The run emitted 44 tick-budget warnings with a maximum of 412.302 ms,
  so this is functional/playable evidence, not a clean performance result.
- P11 artifact
  `.analysis/real-client-runs/20260720T155306Z-real-client-playable-loop-YAMHzs`
  passed the no-debug food loop. The real client killed a natural chicken,
  collected its drop, sprinted until food fell from 20 to 19, then consumed the
  earned chicken and observed food return to 20 while the stack fell from one
  to zero. The failed predecessor exposed that fractional movement exhaustion
  was discarded before reaching the 4.0 threshold. Accepted movement now adds
  every positive exhaustion increment in the same owner turn that commits the
  pose, while health packets remain limited to visible food or saturation
  changes. The repeat after review emitted no tick-budget or slow-tick
  warnings. This is focused gameplay evidence, not a broad performance result.
- An interactive embedded-MCP run on the isolated world under
  `.analysis/mcp-smoke-ZZm7qf` passed
  `playable-05-stone-tool-progression` in 25.7 seconds after connection. The
  real 26.1.2 client mined three natural birch logs, crafted planks, a table,
  sticks, and a wooden pickaxe, mined three natural stone blocks into collected
  cobblestone, reopened the earned table, and crafted a stone pickaxe without
  debug commands. The structured MCP response proved exact inventory and world
  transitions. Server output had two boundary tick warnings at 56.150 and
  54.209 ms, but no multi-second journal stall. This is focused gameplay
  evidence, not a broad performance result.
- The first live Lua gameplay adapter now connects admitted `upsert_zone` and
  `remove_zone` commands to initial/accepted player poses and disconnect
  cleanup. A wire client waits for a plugin readiness message, enters the zone
  through a normal movement packet, and receives the owning plugin's targeted
  `player.zone_entered` reply. Changed bounds do not repeat entry while the
  player remains inside. Workspace tests, strict Clippy, fmt, code-health and
  the 94-test `block_edit` target pass.
- Lua inventory menus now have an end-to-end wire gate in an embedded playable
  world. The test proves admitted Lua open, the client `OpenScreen` and fixed
  content, stale-state rejection, a normal predicted primary click, and the
  plugin's response. A second subscribed plugin plus a later targeted command
  fence proves `inventory.menu.clicked` did not leak beyond the menu owner.
  Atomic inventory/storage transactions now route through the storage actor.
  A disk-backed wire test gives a player currency, commits a purchase and one
  ledger CAS, observes the authoritative inventory, then proves a stale CAS
  rejects without another inventory mutation or leaking its targeted result.
  Storage unit coverage proves multi-key restart replay, one batch revision,
  stale/quota rejection, definite write failure, and unknown-sync replay. The
  runtime transaction excludes concurrent player inventory mutation, but the
  plugin WAL and vanilla playerdata are not yet one crash-recovery log.
- The embedded 26.1.2 client MCP now exposes ordinary primary and secondary
  container-slot clicks and waits for an applied server update instead of a
  guessed delay. An agent-run client on a fresh local world entered the catalog
  zone, received the inventory menu, bought two apples for three emeralds, and
  refunded them. Structured observations proved exact `64 -> 61 -> 64`
  emerald and `0 -> 2 -> 0` apple counts, menu reopen IDs `1 -> 2 -> 3`, ledger
  labels `owned 0 -> 1 -> 0`, and both plugin messages. A stale slot click after
  closing the menu was rejected before packet dispatch. This is focused plugin
  gameplay evidence, not a broad survival or readiness gate.
- Colony identity, homes, roles, orders, limits, and durable member intent now
  live entirely in the shipped strict Luau plugin and its `config.toml`/plugin
  storage. The former Rust colony registry, colony DTOs, `upsert_colony`, and the
  hard-coded `home`/`hold` order vocabulary are deleted. Rust exposes only a
  generic `villagers` capability: an owner-scoped 600-tick opaque binding to the
  nearest live villager plus bounded `idle` and `follow_position` goals through
  the journaled regional entity owner. Typed targeted failures distinguish
  `not_found`, transient `busy`, and `binding_unavailable`; no entity id, region,
  ECS reference, or pathing handle crosses the plugin boundary. The colony
  plugin maps `home` to a configured movement target and `hold` to idle, owns
  retries and disconnect cleanup, and persists only its domain state. Focused
  `mc-script`, villager-router, raw TCP goal, and shipped-plugin disk-backed wire
  gates pass. General villager memory/inventory access and a complete colony
  game remain intentionally outside API `0.6.0`.
- Restart evidence now requires the stopped server process to exit with status
  0. A recorded interrupt without a clean exit can no longer pass validation.
- Multi-entity physics dispatch no longer sends one cached owner mutation per
  entity, each of which ran the complete ECS `PhysicsApply` schedule. The actor
  groups cached same-lane updates by region without serializing unrelated
  lanes; multi-lane work uses the coordinator's equivalent grouping.
  Deterministic tests prove 76 same-region entities run one schedule, same-lane
  and multi-lane regions run one each, stale input runs none, and journal
  failure rolls the whole batch back. The existing 512-entity debug benchmark
  reported actor `p50 5.107 ms` and `p99 6.400 ms`. A real 26.1.2 client
  observed all 31 persisted passive entities
  through 255 client ticks; warned dispatch samples were `4.814`, `10.640`, and
  `2.913 ms`, rather than the earlier repeated `300+ ms` stalls. Canonical
  pathing and collision tables now prewarm before the entity ticker starts. A
  fresh real-client rerun built 5,436 pathing facts before listening, kept all
  32 client-visible entities across 255 ticks, and no longer reproduced the
  earlier `282.512 ms` physics or `316.242 ms` goals first-use stalls. Its only
  warned tick was `56.709 ms`, with goals at `9.575 ms`, physics at `1.264 ms`,
  and dispatch at `11.671 ms`. This closes the catastrophic cold-table stall;
  it is not a replacement for a longer performance soak.
- Furnace cooking now changes the authoritative block state and block entity in
  one resident commit. Both resident and locked fallback paths retain the old
  baked light only as the base for immediate incremental relighting, while
  advancing the light-source token; this removes the 123-127 ms full chunk
  relight seen when a furnace toggled. The embedded 26.1.2 client opened a fresh
  furnace in 75 ms, observed `lit=true` and block light 13, then received
  `minecraft:cooked_porkchop` in output slot 2 through the new event-driven
  `minecraft_wait_for_container_slot` tool. No tick-budget warning occurred in
  the corrected rerun. This is focused furnace evidence, not a broad soak.
- Player melee reach and bare-hand damage are client-verified. A focused
  geometry regression accepts an ordinary survival attack against a sheep
  exactly two blocks away while the existing boundary regression rejects a far
  target. An embedded 26.1.2 client selected an empty hotbar slot, dispatched a
  full-strength attack, received the sheep's authoritative motion update, and
  observed health change from `8` to `7`; the sheep remained alive. The client
  stayed connected. This proves the requested common melee path, not broad
  combat balance across every item, enchantment, effect, or mob.
- Hostile melee now has a facing fence in both planning and final publication.
  A zombie cannot deal damage while its current head direction points away from
  the target, and a stale plan is rejected if either facing, range, visibility,
  attacker life, or player targetability changes before commit. Existing
  push-published target state handles stationary players and immediately fences
  dead players. Focused tests cover those ordinary and race paths. In an
  embedded 26.1.2 client run, the controller issued no movement input: a zombie
  spawned 1.5 blocks away at yaw `0`; the post-damage observation had yaw
  `-180`, player health `17` instead of `20`, and an unchanged player position.
  This is the requested zombie behavior gate, not broad hostile parity.
- Natural passive and hostile spawning now has a fresh-world client gate. An
  embedded 26.1.2 client saw seven naturally spawned pigs/sheep and consumed
  five pushed motion events from one sheep: horizontal deltas stayed smooth,
  yaw changed across events, vertical rise stayed zero, and the sheep travelled
  about `0.85` blocks. Changing only server-console time to night then exposed a
  naturally spawned moving zombie `20.3` blocks away; no summon command was
  used. Focused physics already proves full-block climbing for cows, sheep, and
  chickens, and session tests prove every-tick publication for bounded natural
  passive and hostile movement. This closes basic natural spawn, publication,
  and one-block stepping.
- Ground-mob visual movement now retains a deterministic 3-7-block wander
  destination until arrival, pauses for a per-entity interval, and selects the
  next destination independently. Moving goals bound body and head turning,
  while physics commits preserve the authoritative goal rotation when
  collision clips velocity. Courting animals follow a nearby compatible mate
  and return to wandering after breeding. Stationary melee keeps immediate
  facing so the existing attack fence does not gain a dead tick. Unit coverage
  proves independent destinations, retained paths, pause without position or
  rotation drift, bounded turning, explicit hostile facing, courtship,
  exhausted-path retargeting, path detours, full-block livestock climbs, and
  loading saved wander state from before the pause fields existed. The cow and
  chicken wire breeding regressions also pass. An embedded 26.1.2 client
  separately identified a natural sheep, pig, and cow and received pushed
  samples travelling `0.34-0.39` blocks with non-zero yaw changes and zero
  vertical rise. That client sample confirms the wire path; deterministic tests
  establish independent targets, pauses, and turn limits. The server emitted
  no tick-budget or disconnect warning during this focused gate. This closes
  the common ground-mob visual-quality item, not specialized vanilla goal
  parity.
- The representative player water path is client-verified. The retained O3
  26.1.2 run proves ascent, diving, a `3.43`-block swimming pass, air depletion,
  `20 -> 18` drowning damage, and connection continuity. The missing aquatic
  client path was command spawning: `/summon` left water mobs in `Idle` even
  though natural spawns already received `AquaticWander`. Command-spawned water
  mobs now share one class-wide three-dimensional default goal with natural
  aquatic spawns and start off-ground; exact policy tests cover every supported
  aquatic and amphibious class, including hostile members. A corrected
  representative debug client gate summoned one previously absent tropical
  fish into an inspected deep source-water column, consumed eight pushed motion events,
  measured `0.36` blocks of horizontal travel, and observed it remain underwater
  at `y=62.50..62.57`; the client stayed connected. The fixture also exposed a
  separate scheduled-fluid backlog, recorded in the owner performance queue.
- Scheduled-block snapshot planning runs on an autoscaler-admitted blocking
  worker. The scheduled-block phase services pushed simulation commands while
  that bounded job is active, but does not advance into fluid or later phases
  before the job commits. A shared admission fence rejects overlapping entry
  points. A deterministic 256-button regression held the only CPU permit,
  completed an owner command before release, rejected duplicate admission, and
  then committed all ticks. The optimized `-O3` batch took `1,666 us`. This
  closes scheduled-block owner starvation. The separate interactive
  natural-load client gate is recorded with the dense-load evidence above.

## Manual And Agent Gates

Default playable server:

```sh
cargo run --bin mc-server -- --config playable.toml
```

Use the embedded client MCP for reproducible agent-run observations when the
scenario exists. Record whether a result is owner-run, agent-run, prepared
only, or not run. Screenshots may support a visual finding, but world/protocol
state should come from structured client observations when available.

- The graphical 26.1.2 world-clock gate is agent-run and closed on tree
  `1e2fcc62101c65a5d06dcfa7431dd962f0f62022`. A real client reached pushed
  `in_play=true`, proved a 766-tick first interval, and advanced matching
  `game_time` and overworld-clock deltas by 24,003 ticks while valid captures
  showed day, sunset, natural night, dawn, and the next day. `/time set day`
  and `/time set night` produced the expected rendered sky. A clean stop/save
  followed by restart loaded world time `25,357`; the rejoined client observed
  day-one time `25,540` and the expected sun. Exact observations, artifact
  paths, and the discarded pause-screen capture are recorded in
  `docs/evidence/world-clock-26.1.2.md`. This is agent-run clock evidence, not
  an owner-played subjective gate or no-operator survival evidence.

## Stop Conditions

- Do not update readiness or validation-ledger rows in Playable Spike Mode
  unless the owner explicitly requests readiness work.
- Do not call parity from unit or Solaris-only wire evidence.
- Stop hardening a rare edge once dominant risk is proved and the next common
  gameplay blocker is more valuable.
