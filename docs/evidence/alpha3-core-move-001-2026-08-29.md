# Alpha-3 CORE-MOVE-001 — ordinary movement correction evidence

Date: 2026-08-29
Status: **complete — verifier fix validated; independent reviewer blocker fixed; post-review graphical self-validation PASS**

## Field defect

The standardized graphical Luna QA run `.ai-bridge/pi-agents/20260829015734-1ec74354` reached Play but reported 14 authoritative `SweptCollision` corrections during ordinary sprint+jump movement. The old h5i box's raw graphical run directories were not retained in the parent repository, so its exact packet coordinates/on-ground sequence could not be promoted into a captured replay fixture. The terminal report is retained and the missing raw trace is stated explicitly rather than reconstructed as if it were captured evidence.

## Vanilla 26.1.2 oracle

Local transformed/decompiled 26.1.2 sources under the Gradle cache show the server movement path in `ServerGamePacketListenerImpl`:

- `player.move(MoverType.PLAYER, reportedDelta)` resolves the requested packet movement through the entity collision path;
- the server then compares reported target X/Z against the collision-resolved position and uses horizontal residual squared `> 0.0625` as the Survival/Adventure `moved wrongly` condition;
- Creative bypasses that residual rejection and Spectator/no-physics is handled separately;
- `Entity.collideWithShapes` resolves Y first, then the horizontal axis with the larger absolute requested component, then the other horizontal axis;
- step-up is considered only after horizontal clipping while grounded/falling onto support, uses candidate collider Y heights, and is retained only when horizontal progress improves.

The relevant local oracle was inspected in transformed `net/minecraft/server/network/ServerGamePacketListenerImpl.java` around the player movement block and transformed `net/minecraft/world/entity/Entity.java` around `move`, `collide`, `collectCandidateStepUpHeights`, and `collideWithShapes`.

## Deterministic red case

Before the implementation change, a temporary external probe was compiled against the current public `mc_physics::resolve_collision_displacement` without modifying the repository:

- one full-block collision obstacle at `(1, 64, 1)`;
- player center `(0.6, 64.0, 0.6)`;
- player AABB `half_width = 0.3`, `height = 1.8`;
- desired displacement `(+0.3, 0.0, +0.3)`;
- `was_on_ground = true`, normal player step height.

The old verifier returned approximately:

```text
delta=(0.10000000000000009, 0.0, 0.10000000000000009)
collided=true
stepped=false
horizontal residual squared=0.07999999999999992
```

`0.08 > 0.0625`, so the old continuous diagonal first-impact verifier deterministically entered the same `SweptCollision` rejection class. With vanilla axis-separated horizontal resolution the first horizontal axis can clear the corner before the second is tested, avoiding the artificial two-axis residual.

This is an oracle-backed deterministic reproduction of the semantic defect. It is **not** presented as the exact packet coordinates from the lost graphical trace.

## Root cause and blast radius

`resolve_collision_displacement` was originally reusing the same continuous `resolve_movement` / `resolve_horizontal_sweep` primitive used by ordinary server-side entity integration. That is not vanilla player packet verification semantics at corners.

The general entity solver must not be globally changed for this checkpoint: `mc-physics` already has explicit entity-simulation contracts such as `diagonal_sweep_stops_at_an_isolated_positive_corner` and the corresponding negative-corner case, which intentionally require continuous corner-stop behavior.

The correction therefore specializes only the external-authority verification path:

- general `resolve_movement` keeps `resolve_horizontal_sweep`;
- player verification uses `resolve_vanilla_collision_movement`;
- that path resolves Y first and then uses `resolve_horizontal_axes`, choosing the larger requested horizontal component first;
- shared step selection remains, but receives the verifier-specific horizontal resolver;
- straight tunnelling, embedded escape, Creative/Spectator policy, loaded-destination and residency authority remain separate safety fences.

## Added regressions

`crates/mc-physics/src/lib.rs`:

- `vanilla_axis_order_allows_face_flush_sprint_jump_around_corner`;
- `vanilla_axis_order_keeps_straight_tunnelling_blocked`;
- `vanilla_axis_order_steps_onto_half_block_terrace`.

`crates/mc-net/src/play/movement_tests.rs`:

- `authority_accepts_face_flush_sprint_jump_with_vanilla_corner_axis_order`.

The authority test explicitly documents that its coordinates are oracle-backed synthetic regression data because the original graphical trace did not retain packet coordinates.

## Parent validation

On the parent working tree after applying only the Sol agent patch:

```text
cargo test -p mc-physics --lib -- --nocapture
79 passed; 0 failed

cargo test -p mc-net movement --lib -- --nocapture
57 passed; 0 failed

cargo test -p mc-net --lib --quiet
1984 passed; 0 failed; 5 ignored

cargo run -p xtask -- code-health
0 fail / KEEP

cargo clippy -p mc-physics -p mc-net --all-targets -- -D warnings
PASS

cargo fmt --all -- --check
PASS
```

The first formatter run exposed only a mechanical formatting diff in the already-dirty F3 brand assertion in `crates/mc-net/src/configuration.rs`; that expression was reformatted without behavior change and the exact formatter gate then passed.

## Implementation agent

Detached Pi/Sol implementation run:

```text
run_id: 20260829034741-d263e879
model: openai-codex/gpt-5.6-sol
h5i box: cp-core-move-001-d263e879
agent exit: 0
h5i export exit: 0
agent-only patch: .ai-bridge/pi-agents/20260829034741-d263e879/agent.patch
```

Only the agent-only patch relative to the seeded dirty snapshot was applied. The larger h5i export patch contains the complete seeded parent dirty tree and was deliberately not applied.

## Independent graphical QA and blocker disposition

Exactly one independent Pi/Luna graphical reviewer was run:

```text
run_id: 20260829041526-c5a8e165
model: openai-codex/gpt-5.6-luna
role: independent graphical QA/reviewer
agent exit: 0
h5i export exit: 0
verdict: QA_RESULT=BLOCKED
```

The reviewer did not report a movement failure. Its fresh Xvfb client launched, but the Solaris server could not start inside the h5i QA worktree because ignored runtime input `data/vanilla/version.json` was absent. The blocked artifact is `.analysis/qa-core-move-001/20260829T041822Z-real-client-playable-loop-oUZ6ah/` inside that h5i run. Per repository policy, the environment finding was fixed without spawning a second reviewer.

CodexPro's QA lifecycle now materializes parent `data/vanilla` into QA/reviewer h5i worktrees as a bounded runtime-only input: symlinks are refused, total bytes are capped at 64 MiB, a deterministic tree SHA-256/file-count/byte-count is recorded in run metadata, and the prompt explicitly forbids staging or treating those Mojang-derived bytes as source.

## Post-review graphical self-validation

The same acceptance boundary was then exercised on the parent tree, where the required vanilla sidecar is present. These are post-review self-validation runs, not additional independent reviewers.

### 90-second ordinary movement soak

Artifact: `.analysis/core-move-001-self-qa/20260829T044133Z`.

A real Minecraft Java 26.1.2 client ran under fresh Xvfb and completed exactly 18 ordinary `[forward, sprint, jump]` pulses of 100 client ticks each: **1800 client ticks / 90 seconds**. The route traversed twelve distinct chunk coordinates and naturally crossed terrain elevations. It also opened/closed inventory and completed disconnect/reconnect back into Play.

Server-log classification for the complete ordinary soak:

```text
SweptCollision:       0
WorldUnavailable:     0
runtime unavailable:  0
server panic:          0
```

The structured run has `passed=false` only because its helper omitted the required `face` argument when attempting the targeted natural-log obstacle subroute. That harness argument defect did not invalidate the already-completed 1800-tick ordinary movement, chunk-seam, inventory, reconnect, or server-log evidence.

### Natural full-block face/corner routes

The targeted obstacle portion was rerun as a short graphical continuation against the persisted test world after correcting only the evidence helper's `minecraft_look_at_block` arguments.

Artifact: `.analysis/core-move-001-obstacle-qa/20260829T044614Z`.

The real client used only client-visible scans and ordinary navigation/input to reach two natural `minecraft:spruce_log` obstacles. Both routes completed:

1. natural log `(60, 74, 84)`, west face — navigate, 120 ticks face-pressure sprint+jump, then 120 ticks diagonal release at +45°;
2. natural log `(67, 74, 82)`, west face — navigate, 120 ticks face-pressure sprint+jump, then 120 ticks diagonal release at -45°.

Structured result: `passed=true`, `successful_routes=2`. Server classification remained:

```text
SweptCollision:       0
WorldUnavailable:     0
runtime unavailable:  0
server panic:          0
```

Screenshots and client/server logs are retained under the artifact directory. The client remained in Play throughout both obstacle routes.

## Invalid-movement safety boundary

The graphical MCP intentionally exposes ordinary Minecraft input rather than arbitrary impossible position-packet injection, so the adversarial tunnelling check is covered at the authoritative verifier boundary instead of being misrepresented as a graphical client action. The focused suite includes both `vanilla_axis_order_keeps_straight_tunnelling_blocked` and `authority_movement_sweep_rejects_tunneling_between_clear_endpoints`, together with existing embedded-escape, loaded-destination, world-residency, Creative and Spectator boundary tests. All are included in the green `mc-physics` 79/79 and `mc-net movement` 57/57 results above.

## Closeout

CORE-MOVE-001 closes because the deterministic old-verifier corner failure is green under the verifier-specific vanilla axis order, general entity continuous-sweep contracts remain intact, the complete parent movement suites are green, a real 90-second client route produced zero ordinary corrections across multiple chunk seams plus reconnect, and two targeted natural full-block face/corner routes also produced zero corrections. No tolerance was widened and the deliberate tunnelling fence remains red-to-reject.
