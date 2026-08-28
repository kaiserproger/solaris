# Phase 4 20-minute fresh-world survival gate — 2026-08-26

## Scope

This checkpoint closes Phase 4 item 6 and the separate graphical natural-spawn acceptance gate in `docs/PUBLIC_ALPHA_PLAN.md`. It uses the repo-native graphical Minecraft Java 26.1.2 client, a fresh isolated world on seed `712816`, default natural-spawn configuration, no operator list and no debug-command setup.

Scenario: `playable-04-twenty-minute-survival-loop`, followed by runner-managed clean server restart and `playable-03-save-restart-after` against the same world.

Artifact root:

`.analysis/real-client-runs/20260825T235506Z-real-client-playable-loop-Bl9LAn`

Runtime provenance:

- `DISPLAY=:99`;
- `server_seed_override=712816`;
- `server_op_users=NONE`;
- real client adapter `:fabric-agent:runClientAgent`;
- compiled client runtime SHA-256 `934fbc6cf7f17a340ccbbe94b8062c5ace712fbf895f2e65054920014d74550d`;
- runtime validation `verified`.

Fail-closed artifact validation was rerun after completion:

```text
bash tools/run-real-client-regression.sh --validate-run \
  .analysis/real-client-runs/20260825T235506Z-real-client-playable-loop-Bl9LAn
validated .analysis/real-client-runs/20260825T235506Z-real-client-playable-loop-Bl9LAn
```

## Survival evidence

`observations.json` reports top-level `result = passed` and primary scenario `result = passed`.

Before the soak, the real client completed ordinary survival setup from generated terrain: natural acacia log break/drop/pickup, planks, crafting-table placement/opening, sticks, wooden pickaxe and wooden sword, with no debug setup.

The soak then completed the full requested duration:

- `duration_millis=1200000`;
- `ticks=24000`;
- `resource_runs=1`;
- `hostiles_neutralized=7`;
- `recovered_deaths=3`;
- final `wooden_pickaxe_count=1`.

Natural-spawn acceptance passed from actual spawned entities: a sheep was observed as the friendly/passive witness and a spider as the hostile witness. The final observation is `natural spawn acceptance: passed passive_observed=true hostile_observed=true`.

Local reachable logs were exhausted after the initial resource work. The gate recorded the bounded exhaustion and continued the survival soak rather than fabricating resources or using operator setup.

## Natural death / respawn / item recovery

The run exercised three natural deaths during the same 20-minute graphical session, at completed ticks `17120`, `21480`, and `23441`.

For every death:

- the real client reached the vanilla death screen;
- a new wooden-pickaxe item entity and wooden-sword item entity were identified by exact entity id + UUID;
- vanilla respawn passed;
- the wooden pickaxe was recovered from the attributable death drop;
- the wooden sword was recovered, including the valid automatic-pickup case;
- the scenario continued playing afterwards.

The first death, at tick `17120`, identified pickaxe entity `1000101` / UUID `5f1a0000-0000-0000-0000-0000000f42a5` and sword entity `1000102` / UUID `5f1a0000-0000-0000-0000-0000000f42a6`. The later deaths likewise produced distinct attributable identities and passed recovery.

This directly exercises the acceptance-harness race fixed before this run: death-drop identities are no longer folded into a post-death exclusion baseline when asynchronous damage lands between soak iterations.

## Clean restart / reconnect evidence

`automation-driver.txt` records:

- `client_agent_phase_exit_status_playable-04-twenty-minute-survival-loop=0`;
- clean pre-restart server stop with `status=0`;
- `server_restart_count=1`;
- restarted server `status=ready`;
- `client_agent_phase_exit_status_playable-03-save-restart-after=0`;
- `client_agent_driver_exit_status=0`.

After restart/rejoin, the real client reported:

- `restart marker persistence: passed target=4,71,5/up`;
- `inventory persistence: passed wooden_pickaxe_count=1`;
- `disconnect_reason=""`;
- `in_play=true`;
- `playable-03-save-restart-after` result `passed`.

The runner terminates the graphical client after the completed phases, so `client_exit_status=143` is the expected runner-owned process shutdown rather than a gameplay disconnect; the driver itself exited `0`.

## Log disposition

The server log contains no `ERROR`, panic, `DestinationUnloaded`, `degraded_delivery=true`, teleport-confirmation mismatch, reliable-outbound drop/backlog warning, or gameplay disconnect.

Performance warnings were not suppressed or hidden. The run contains bounded >50 ms tick-budget warnings, with a maximum observed tick of `82.781 ms` at tick `20906`, plus short lock-budget warnings around survival commits (maximum observed `commit player survival` hold about `35.9 ms`) and one `session registration epoch` wait of about `16.4 ms`. These did not produce degraded delivery, state loss, duplicate entities, disconnect, or validator failure. They remain performance evidence rather than a reason to weaken the frozen thresholds.

The client-side startup warnings are also fully dispositioned rather than treated as unexplained gameplay warnings:

- `Advanced terminal features are not available in this environment` is emitted before Minecraft startup by the non-interactive Gradle/dev-launch terminal environment. The identical warning is present in the earlier real-client artifact `.analysis/real-client-runs/20260706T115222Z-real-client-playable-loop/client.log`.
- Ten `minecraft/Commands` ambiguity warnings cover the vanilla/NeoForge `teleport`, `time`, and `waypoint` command-tree registrations. The same warning set is present in that 2026-07-06 real-client artifact, so it predates this checkpoint and is client command-registration noise rather than a Solaris gameplay failure. The no-operator scenario does not depend on these command branches.
- Two `mojang/GlProgram` warnings report that NeoForge `item_translucent_unlit` and `item_cutout_unlit` pipelines define an unused `Sampler2`. Both are likewise present in the earlier artifact. Rendering continued under llvmpipe, screenshots were captured, and neither client phase failed.
- OpenAL could not open an audio device under the X-display runner, so Minecraft disabled sound/music; ALSA also reports that the default device is unavailable. This is an environment-only audio failure. No gameplay disconnect or crash followed.

After startup, the client connected, completed the 20-minute scenario, reconnected after server restart, and remained `in_play=true` with empty `disconnect_reason`; the artifact validator passed. There are no later client WARN/ERROR entries indicating a gameplay, state, network, or rendering failure.

## Disposition

The fresh-world 20-minute no-operator survival + natural-spawn + death/respawn + clean restart/reconnect gate is **PASS**.

This closes Phase 4 item 6 and the graphical natural-spawn acceptance checkbox. It does not by itself satisfy the separate subjective owner terrain/playability disposition for seed `712816`; that boundary remains separate wherever `PUBLIC_ALPHA_PLAN.md` still requires it.
