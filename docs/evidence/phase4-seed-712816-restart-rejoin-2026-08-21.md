# Phase 4 seed 712816 traversal / restart / rejoin gate — 2026-08-21

## Scope

This checkpoint closes the automated traversal and restart/rejoin portion of the Phase-4 worldgen acceptance boundary on the exact public-test seed `712816`. It uses the repo-native graphical Minecraft Java 26.1.2 client agent, a fresh isolated `tellus_like` world, no operator list and no debug-command setup.

The composite real-client scenario is `playable-03-save-restart-rejoin` from `docs/playable/real-client-playable-loop.json`. The runner executes a natural-resource before phase, performs a clean Solaris stop/restart against the same world directory, reconnects the same real client and executes the after phase.

## Invocation

The run used the normal playable runner with:

- `DISPLAY=:1` backed by local Xvfb;
- `SOLARIS_REAL_CLIENT_MANIFEST=docs/playable/real-client-playable-loop.json`;
- `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-03-save-restart-rejoin`;
- `SOLARIS_REAL_CLIENT_SERVER_SEED=712816`;
- `SOLARIS_REAL_CLIENT_FRESH_WORLD=1`;
- no configured operators.

Artifact root:

`.analysis/real-client-runs/20260821T130644Z-real-client-playable-loop-yQG8Og`

Fail-closed artifact validation:

```text
bash tools/run-real-client-regression.sh --validate-run \
  .analysis/real-client-runs/20260821T130644Z-real-client-playable-loop-yQG8Og
validated .analysis/real-client-runs/20260821T130644Z-real-client-playable-loop-yQG8Og
```

## Before-restart evidence

`observations.json` reports `result = passed` for `playable-03-save-restart-before`.

The real client:

- joined the generated `minecraft:overworld` without debug setup;
- approached naturally generated acacia logs;
- broke and picked up three real log drops;
- converted the logs to twelve acacia planks through the inventory recipe;
- crafted and placed a crafting table and opened the vanilla crafting screen;
- crafted sticks and one wooden pickaxe;
- wrote the restart marker against the placed table;
- captured `screenshots/playable-03-save-restart-before.png`.

The effective runner config records `server_seed_override=712816`, an isolated per-run world directory, and `server_op_users=NONE`.

## Clean restart / after-restart evidence

`automation-driver.txt` records:

- `client_agent_phase_exit_status_playable-03-save-restart-before=0`;
- clean server `INT` stop with `status=0`;
- `server_restart_count=1`;
- restarted server `status=ready`;
- `client_agent_phase_exit_status_playable-03-save-restart-after=0`;
- `client_agent_driver_exit_status=0`.

The rejoined client observed:

- the same persisted player position (`x=5.043153530787848`, `y=71.0`, `z=3.8174087805812453`);
- `restart marker persistence: passed target=4,71,2/up`;
- `inventory persistence: passed wooden_pickaxe_count=1`;
- no disconnect reason;
- `screenshots/playable-03-save-restart-after.png`.

The top-level observations result is `passed`.

## Disposition

The automated exact-seed traversal/restart/rejoin gate is **PASS**. This is stronger than the earlier graphical spawn-only preflight because it exercises ordinary movement, natural block break/drop/pickup, recipes, table placement/menu opening, clean persistence, server restart and rejoin on the same generated world.

This does **not** substitute for the separate subjective owner terrain/playability disposition requested by `PUBLIC_ALPHA_PLAN.md`, and it does not replace the 20-minute no-operator natural-spawn survival soak. Those remain separate acceptance boundaries until explicitly closed.
