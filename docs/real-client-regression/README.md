# Real-Client Regression Pack

This directory tracks reproducible real-client scenarios for M94+ evidence.
These manifests are not protocol harnesses and do not count as client evidence
until a normal vanilla 26.1.2 client or PrismLauncher-launched client executes
them and the required artifacts are recorded.

Artifacts from a run stay local under `.analysis/real-client-runs/<run_id>/`:

- `manifest.json` copied from the tracked scenario manifest.
- `client.log` from the real client.
- `server.log` from `cargo run --bin mc-server -- --config <config>`; the
  runner defaults to `example.toml`.
- `observations.json` with structured pass/fail notes for each step.
- `screenshots/` containing at least one screenshot for every scenario whose
  manifest entry has `screenshots_required: true`.
- `git.txt` with commit id and `git status --short --branch`.
- `toolchain.txt` with the autonomous preflight output.
- `automation-driver.txt` with the approved client kind, launch command, and
  forbidden-client guard outcome.

Protocol bots, `wire-probe`, and `mc_test_harness::client::Client` runs may be
linked as supporting harness evidence, but they must not be recorded as the
`client_gate` for these manifests.

## Runner

The approved local entrypoint is `tools/run-real-client-regression.sh`:

```sh
SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_COMMAND='<real PrismLauncher or vanilla client command>' \
  bash tools/run-real-client-regression.sh --check

bash tools/run-real-client-regression.sh --prepare

SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_COMMAND='<real PrismLauncher or vanilla client command>' \
  bash tools/run-real-client-regression.sh --run
```

Set `SOLARIS_REAL_CLIENT_SERVER_CONFIG=<path>` to use an explicit local config,
for example a copy of `example.toml` with `data.vanilla_data_dir` enabled.

`--run` starts Solaris, executes the configured real-client command, and writes
the local artifact directory. It does not mark scenarios passed. After a real
client executes the pack, fill `observations.json` with `client_gate` set to
`agent-run real-client`, attach screenshots/logs, and run:

```sh
bash tools/run-real-client-regression.sh --validate-run .analysis/real-client-runs/<run_id>
```

Validation checks the artifact shape, fails while the observations remain
`not-run`/prepared, rejects observed scenario ids that are absent from the
manifest, and rejects a passed observed scenario when its manifest requires
screenshots but `observations.json` does not point at an existing file under
`screenshots/` or points at a file that is not a structurally valid PNG. The
runner rejects protocol bot/mock commands before `--run`; `--validate-run` does
not re-authenticate a manually edited artifact directory. The in-client agent
driver also rejects invalid captured PNG artifacts before writing a passed
observation.

## In-Client Agent Driver

The invasive real-client path uses a Solaris-owned client agent that exposes a
loopback-only JSON bridge from inside the actual Minecraft client process. The
approved external driver is `tools/real-client-agent-driver.py`.

Agent-driver mode is opt-in:

```sh
SOLARIS_REAL_CLIENT_KIND=prism-launcher \
SOLARIS_REAL_CLIENT_COMMAND='<real PrismLauncher or vanilla client command>' \
SOLARIS_REAL_CLIENT_AGENT_SECRET='<per-run secret>' \
  bash tools/run-real-client-regression.sh --run
```

Before using the default in-client Java agent, build its jar:

```sh
client-mod/solaris-client-agent/gradlew \
  -p client-mod/solaris-client-agent :java-agent:jar --no-daemon
```

By default the runner injects
`client-mod/solaris-client-agent/java-agent/build/libs/java-agent-0.1.0.jar`
through `JDK_JAVA_OPTIONS` and uses `http://127.0.0.1:39094/rpc`. Override
the jar with `SOLARIS_REAL_CLIENT_AGENT_JAR`, the port with
`SOLARIS_REAL_CLIENT_AGENT_PORT`, or the full bridge URL with
`SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL`. The per-run bridge secret is passed to
the Java agent through `SOLARIS_CLIENT_AGENT_SECRET`, not on the
JVM options command line. The runner also adds `--add-modules jdk.httpserver`
because the loopback bridge uses the JDK HTTP server module.

Two-client agent gates additionally require
`SOLARIS_REAL_CLIENT_SECOND_COMMAND` and
`SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET`; `SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT`
defaults to `39095`, and `SOLARIS_REAL_CLIENT_SECOND_AGENT_BRIDGE_URL` defaults
from that port when the second secret is set. When using PrismLauncher for both
clients, launch the second client from a separate Prism application root such as
`--dir /home/kaiserroman/solaris/.analysis/prism-second`; otherwise
PrismLauncher can route the second command to the existing launcher process and
the second Java agent bridge never starts.

Set `SOLARIS_REAL_CLIENT_SERVER_ADDR=<host:port>` when the in-client driver
should connect somewhere other than `127.0.0.1:25565`. The default scenario is
`m94-02b-rejected-block-resync`; override it with
`SOLARIS_REAL_CLIENT_AGENT_SCENARIO`.

The Java-agent `m94-02a-solid-place-break-drop` scenario prepares held items
through Solaris `/debug give` commands, uses the real client's item-use path for
placement, and sends named vanilla `ServerboundPlayerActionPacket` destroy
actions from inside the real client process for the server-timed break/drop
check. `m94-02b-rejected-block-resync` prepares held items the same way and uses
the normal item-use path for rejection/resync checks.
`m94-02c-water-bucket-place-pickup` uses the normal item-use path for focused
accepted water-bucket placement and source pickup, while keeping lava, broad
spread, water-lava interaction, and swim feel degraded.
`m94-02-blocks-fluids-farming-drops` composes the focused solid
break/drop/place and water-bucket place/pickup probes. It intentionally returns
`blocked`, not `passed`, until door/trapdoor, crop/bonemeal, sugar cane
support/cascade/drop, broad fluid spread, water-lava interaction, and swim feel
have dedicated in-client primitives and server evidence.
`m94-04a-regular-sign-place-text` uses the normal item-use path for focused oak
sign placement, waits for the real sign editor, sends plain four-line text from
inside the client process, checks the client-side sign block entity, closes the
editor, and checks the text again after close. It keeps hanging signs,
waxed/styled/filtered/clickable text, bed sleep/respawn, campfires, restart
persistence, and broad visual parity assertions degraded.
`m94-04-signs-beds-campfires-and-block-entities` composes that regular-sign
probe. It intentionally returns `blocked`, not `passed`, until beds, campfires,
restart persistence, hanging/waxed/styled signs, sounds/statistics/events, and
broader visual assertions have dedicated in-client primitives and server
evidence.
`m94-03a-inventory-oak-log-to-planks` gives the client one oak log, sends the
vanilla place-recipe packet for inventory container `0` and the current
configured vanilla sidecar oak-planks recipe display id, and checks that one oak
log is consumed and four oak planks are added relative to starting inventory
counts. It keeps crafting-table UI, cursor recovery, recipe-book discovery UI,
containers, stations, malformed clicks, and broad recipe execution degraded.
`m94-03-inventory-crafting-containers-stations` reuses the same inventory recipe
probe, then places a chest through the real client, opens the vanilla
`ContainerScreen`, and closes it. It intentionally returns `blocked`, not
`passed`, until broad cursor transfer, barrel/furnace/common-station clicks,
malformed clicks, and recovery paths have dedicated in-client primitives and
server evidence.
`m94-03b-two-client-shared-chest` is a focused two-client route. The primary
bridge places a chest, moves dirt into chest slot `0` through the vanilla
menu-click path, and writes marker coordinates into the run directory; the
secondary bridge opens that marked chest and waits for dirt in slot `0`. It
requires two real clients/bridges. The first passing local artifact is
`.analysis/real-client-runs/20260622T221125Z-m94-regression-pack`; broad
containers, stations, cursor recovery, malformed clicks, and contention remain
degraded.
`m94-03c-two-client-shared-chest-live-update` is a focused two-client route for
open-screen convergence. The primary bridge first moves the operator-controlled
primary client back to the shared spawn area, places a chest, moves dirt into
chest slot `0`, and keeps the container screen open. The secondary bridge opens
the same chest, quick-moves slot `0` into its inventory, and closes. The primary
bridge then waits for slot `0` to become empty on the still-open screen before
closing. It requires two real clients/bridges. The first passing local artifact
is `.analysis/real-client-runs/20260622T232643Z-m94-regression-pack`; broad
cursor recovery, malformed clicks, barrels, furnace-family UIs, common
stations, deliberate stale-click injection, performance, vanilla oracle, and
soak evidence remain degraded or unrun.
`m94-05-entities-combat-death-respawn` summons a visible cow near the real
client, kills the survival player through Solaris' debug damage path, observes
the real `DeathScreen`, performs the vanilla respawn client command, and waits
for the client to leave the level-loading screen. It intentionally returns
`blocked`, not `passed`, until hostile combat, melee damage/knockback, mob
drops, XP pickup, projectiles, shield timing, vehicles, and broad AI/pathing
have dedicated in-client primitives and server evidence.
`m94-06-save-restart-two-client-visibility` is runner-managed. The public broad
scenario id stays blocked on its own; the approved runner decomposes it into
`m94-06-save-restart-before`, a graceful Solaris restart, and
`m94-06-save-restart-after`. The before phase places a dirt marker through the
real client, sends `save-all`, and writes marker coordinates into the run
directory. The runner stops Solaris with `kill -INT`, restarts it, then calls the
after phase with `--append-observations` so both phase results stay in one
`observations.json`. The after phase reconnects the same real client and checks
that the marker persisted. If the second-client environment is configured, the
runner then starts the secondary real client and appends
`m94-06-two-client-live-visibility`: the primary bridge places a fresh dirt
marker and the secondary bridge observes it. The same secondary-client launch
then appends `m94-06-two-client-shared-drop`: the primary bridge breaks a
dirt-like block, records a visible dirt item entity, and the secondary bridge
observes the shared drop. The runner then appends
`m94-06-two-client-shared-pickup`: both clients first observe the shared drop,
the primary bridge collects it through the real client, and the secondary bridge
observes the item removal. The broad result intentionally remains `blocked`, not
`passed`, until broader two-client join/move/edit coverage, shared container
convergence, and contention have dedicated evidence.
`m94-07-m40-m41-route-with-metrics` composes the accepted water-bucket,
solid/drop/pickup, and visible passive-entity probes into the named M40/M41
route. It intentionally returns `blocked`, not `passed`, until swim feel, sugar
cane support/cascade/drop, the owner frozen-world route, and full TPS/lock
performance evidence have dedicated real-client or manual gates.

Run them against a local test config whose `[admin]` section allows
the launched profile to use debug commands, for example by setting
`allow_local_dev_operators = true` on a loopback-only bind or by listing the
profile in `operators`.

This path remains non-green until a real client bridge writes
`observations.json` with `"client_gate": "agent-run-real-client"` and
`tools/run-real-client-regression.sh --validate-run "$RUN_DIR"` passes. Fake
bridge tests only validate driver plumbing; they do not count as real-client
evidence. The driver only records a passed scenario when the in-client bridge
returns an explicit `run_scenario` result of `passed`; bridge connection,
`wait_play`, and screenshots by themselves are not a gameplay pass.
Validation also requires passed observed scenarios with
`screenshots_required: true` to reference at least one existing
`screenshots/` artifact, and both the runner and driver reject screenshots that
are not structurally valid PNGs before accepting passed observations. These
checks prove artifact shape only, not screenshot content quality or gameplay
correctness. Observed scenario ids must exist in the copied manifest, so a typo
or fabricated id cannot validate as M94 coverage.

The Solaris-owned bridge source lives under
`client-mod/solaris-client-agent/`. The `bridge-core` module is a pure Java
loopback JSON-RPC transport plus fakeable command facade and can be checked
without a Minecraft client:

```sh
client-mod/solaris-client-agent/gradlew \
  -p client-mod/solaris-client-agent :bridge-core:test --no-daemon
```

Passing `bridge-core` tests prove only transport/command-contract behavior.
They do not prove that a vanilla/PrismLauncher client launched, reached play,
or executed `m94-02b-rejected-block-resync`.

The `java-agent` module compiles against the local named 26.1.2 client jar in
`.analysis/client-automation/versions/26.1.2/client.jar` and packages the
bridge classes into the agent jar. Its `run_scenario` implementation covers the
focused M94 solid place/break/drop, rejected-block, accepted water-bucket,
regular sign place/plain-text, and inventory oak-log-to-planks scenarios plus
the blocked broad `m94-02`, `m94-03`, `m94-04`, `m94-05`, `m94-06`, and `m94-07`
compositions, but a packaged jar proves only bridge and adapter plumbing until a
real vanilla/PrismLauncher client run validates the artifacts.

## Current Pack

- [`manifests/m94-regression-pack.json`](manifests/m94-regression-pack.json) is
  the bounded M94 scaffold. It covers the M94 checklist with focused scenarios
  and marks each one `manual-pending` until a real client run exists. The runner
  provides an approved automation entrypoint, not completed evidence by itself.
- `scoped_rows_manual_pending` records scoped rows that are not runnable as part
  of the bounded scenario pack yet; they stay non-green until a later real-client
  or owner-accepted degraded gate supplies artifacts.
