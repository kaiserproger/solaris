# Agent Tooling

`AGENTS.md` contains the rules; this file contains the local wiring.
`python3 -m tools.harness list` is authoritative for the profile catalog and
`python3 -m tools.harness explain` for the scenario catalog; prose here never
overrides them. Never invoke `tools/harness/backends/` directly: those engines
are private and only a profile run leaves a canonical receipt.

## Harness entrypoint

Run from the repository root:

```sh
python3 -m tools.harness list
python3 -m tools.harness run PROFILE [--release] [--timeout-seconds N] [--client java|native] [--platform fabric|neoforge|forge] [--plugins-dir PATH] [--windowed] [--server-bin PATH] [backend args]
python3 -m tools.harness explain [scenario-id]
python3 -m tools.harness run scenario <id>
python3 -m tools.harness client [--check] [--platform fabric|neoforge|forge]
python3 -m tools.harness mcp [--server-address host:port] [--exercise-input] [--disconnect] [--scenario-id ID]
```

Typed-option constraints (fail closed with exit 2 on violation):

- `--server-bin` is only supported by `loader-live` (immutable candidate
  executable). Other launch paths honor the read-only `SOLARIS_SERVER_BINARY`
  environment candidate instead of building.
- `--plugins-dir` requires native gameplay, `java-v2-p5-towns`, or
  `loader-live`. The directory must already exist; it is used as-is.
- `--windowed` requires `--client native`.
- `--client native` is enabled only for `survival`, `inventory`, `entities`,
  `core-client`, `native-lifecycle`, `logical-native`, `native-scale`,
  `title-connect`, and `dialogs`. Other profiles fail before launching Java.
- `--release` only changes `build`; every other profile ignores it.
- `--timeout-seconds N` overrides the scenario default where the backend
  honors it (`core-client`/`inventory` 600 s, `loader-live` 600 s,
  `playable` 1500 s, `replay` 180 s). Graphical runs additionally need Linux
  `inotify`/`pidfd`/`Xvfb` plus client credentials (see below).

Receipt statuses are `passed` | `failed` | `prepared` | `interrupted`. Only
real completion is `passed`. `--check`/`--prepare` (and other
preparation-only modes) report `prepared` with exit 0 — never a gameplay
pass. Unknown profiles, unlaunchable or nonzero commands, and missing
scenario evidence fail closed with a nonzero exit. Every timeout is failure,
never success.

`scene-shot` runs scripted real native-client captures: set `SOLARIS_SERVER_BINARY`,
`SOLARIS_CLIENT_BINARY`, the native resource environment and `SCENE_SHOT_SCENARIO`
to a JSON file, then run `python3 -m tools.harness run scene-shot --client native
--windowed`. The JSON contains `steps` with one of `cmd`, `tp` (five coordinates),
`wait_ticks`, `wait_play`, `shot`, `observe`, `use_item_on`,
`click_screen_button`, `scan_blocks`, `read_block`, `navigate_to_block`,
`list_entities`, or `key` (`keys` and `ticks` MCP arguments; released when
the step finishes). Optional `seed` defaults to 100, `world` copies an existing
world, `server_view_distance` defaults to 4, and `client_settings` accepts
`graphics_preset`, `render_distance` and `fov` (default 70: the owner's saved fov is never
inherited). PNG paths are recorded in `result.json`; settings and world are isolated per run.
With a client supporting held MCP input, `{"shot":"tab","hold_keys":["key.keyboard.tab"]}`
captures the visible player list and releases the key even if capture fails (native only).

`--client java --windowed` runs the same scenario against the vanilla Java client
(Loader `:fabric-agent:runClientMcp`) as a rendering oracle: `shot` uses
`minecraft_screenshot`, and the client gets the same 1280x720 window, fov, GUI scale 3 and render
distance through its `options.txt`. Run the same scenario with both clients and compare the
PNGs; Mojang's client is a test-time reference only, never a source for Solaris models.

## Profiles

| Profile | Scope | Recipe |
|---|---|---|
| `correctness` | rust | Full L2: `fmt`, strict workspace Clippy, `code-health`, then the nextest suite. `python3 -m tools.harness run correctness` |
| `fmt` | rust | `cargo fmt --all -- --check` |
| `clippy` | rust | `cargo clippy --workspace --all-targets -- -D warnings` |
| `code-health` | rust | `cargo run -p xtask -- code-health` (fail-only structural tripwire, not gameplay evidence) |
| `test` | rust | Nextest workspace/all-targets gate; trailing package/target/name selectors use the same runner, e.g. `python3 -m tools.harness run test -p mc-core --lib inventory`. See below. |
| `build` | rust | `cargo build --locked -p mc-edge --bin solaris-v2`; `--release` selects release |
| `harness-check` | harness | Fail-closed receipt/process lifecycle regressions plus private shell-engine syntax (`bash -n` over every backend) |
| `java` | loader | Gradle bridge/agent/Loader module tests. Requires a Minecraft client jar via `SOLARIS_CLIENT_JAR` or the Loader's documented local path; CI downloads the version declared in Loader `gradle.properties` and verifies Mojang's SHA-1. No test retries. |
| `fixture-check` | loader | Reproducible Loader fixture verification (`tools/build-loader-live-gate-fixture.sh --check`); requires `ffmpeg` with `libvorbis`, plus ZIP and coreutils |
| `standard-pack` | plugins | Rebuilds and byte-verifies the explicit first-party `standard-pack` sibling via `../solaris-default-plugins/tools/run_integration.py`; `SOLARIS_DEFAULT_PLUGINS_ROOT` selects that checkout. `--real-client` adds the sibling graphical `/money` scenario under Xvfb. |
| `installer` | release | Installer self-test backend |
| `survival` | real-client | One shared Python MCP mine/pickup/place scenario, Java (default) or windowless native. `python3 -m tools.harness run survival --client native` |
| `dialogs` | real-client | Native server-driven dialog (`--client native`; `--windowed` adds screenshots): the server runs with `SOLARIS_DIALOG_PROBE=1`, the client types the text input, steps the slider, flips the checkbox, presses Submit, and the server log must show the exact typed payload. Requires `SOLARIS_SERVER_BINARY`. |
| `title-connect` | real-client | Native Title screen type-address+name, Connect, disconnect-back-to-Title (`--client native`; `--windowed` adds screenshots). Requires `SOLARIS_SERVER_BINARY` (reuses the survival-built server). |
| `army-qa` | real-client | Settlement army dialogs, equipment, movement, combat and restart (`--client native --windowed`). The guard-return fixture waits for every live guard within 2.5 blocks of its post before spawning a zombie 11 blocks away, inside the 12-block tether; the same guard must leave by more than 4 blocks and return within 2.5. Before combat, the command-order check requires an `Ordered` reply newer than its chat fence and a dialog roster showing Patrol for every soldier; Return deliberately does not engage. Requires `SOLARIS_SERVER_BINARY`; its canonical plugin directory is empty. |
| `core-client` | real-client | Server/own-chat continuity subset only (Java default, native subset). The receipt explicitly excludes Wasm, economy, and Loader-required rejection evidence. |
| `inventory` | real-client | Shared crafting/cursor/selection probe (Java default, native, or named `run scenario inventory-client --client java`). Ordinary slot clicks, no `minecraft_run_scenario` in the probe. |
| `entities` | real-client | One shared bounded pig feeding/damage/death/drop probe with declared operator fixture and UUID/type fencing (`--client native` or `java`) |
| `native-lifecycle` | real-client | Reconnect, received death/respawn, and 30-minute stationary RSS soak; the isolated world uses peaceful difficulty during the soak and restores its original difficulty (`--client native|java`; `--soak-seconds 0` is smoke-only) |
| `logical-native` | functional-native | Cheap headless movement/look plus rejected-use stable-state subset. Explicitly not `playable`/`regression`/`replay`; `--windowed` is rejected. |
| `native-scale` | functional-native | Headless native CPU/RSS canaries beside real Rust protocol bots; non-exclusive diagnostic only |
| `loader-live` | real-client | Loader gate via `loader.run`; requires `--platform fabric|neoforge|forge`, an immutable `--server-bin`, and a matching `--plugins-dir`. `python3 -m tools.harness run loader-live --platform neoforge --server-bin /absolute/solaris-v2 --plugins-dir /absolute/packages` |
| `regression` | real-client | Real-client regression backend over `regression.sh`; bare run defaults to `--run`. Rejects protocol bots, wire-probes, `mc-test-harness` clients, and mocks as participants. |
| `playable` | real-client | Ordinary twenty-minute no-debug survival loop (`docs/playable/real-client-playable-loop.json`, fresh per-run world, default timeout 1500 s). Never shorten it or substitute a debug scenario. `--check`/`--prepare` only prepare. |
| `replay` | real-client | Core replay seed-81 gate (`docs/real-client-regression/manifests/core-replay-seed-81.json`, fresh world, default timeout 180 s). Not a general regression short-cut. |
| `oracle` | oracle | Vanilla oracle backend: bare `oracle` is readiness inspection (`prepared`, blocked/degraded when artifacts are absent); `run oracle --run` executes comparisons. |
| `oracle-check` | oracle | Oracle runner self-check backend |
| `quirk-parity` | oracle | Real 26.1.2 vanilla oracle vs Solaris on player-relied quirks (quasi-connectivity, TNT duping, update order, 0-tick, attribute swap, diagonal speed) and farm rates; `python3 -m tools.harness run quirk-parity -- --family tnt-duping`. See [QUIRK_PARITY.md](QUIRK_PARITY.md). |
| `catalog-load` | scale | Owner-catalog v2 open-loop protocol loads with full public fixtures and classified receipts. Explicit prebuilt `--server-bin`/`--bots-bin`; no implicit builds. See below. |
| `constantinople` | fixture | Imported vanilla-city functional fixture: deterministic population, street walking, native save/reopen. Not a throughput or performance verdict. See below. |
| `seed-review` | terrain | Seed owner-review backend (module invocation, default timeout 120 s). Captures evidence; it never grants owner `ACCEPT`. |
| `seed-contact-sheet` | terrain | Seed contact-sheet capture preparation; always `prepared`, never a gameplay pass |
| `bucket-resync` | debug | Real-client bucket resynchronization debug loop; `--check` prepares only, `--real-client` (default) executes |

`correctness` is the L2 close gate. `regression`, `playable`, and `replay`
append `--run` unless a mode token is already present (`--check`,
`--prepare`, `--run`, `--validate-run`); `--check` validates the Gradle
runClient adapter, `--prepare` creates a run directory and observation
templates, `--validate-run <dir>` checks an existing run shape.

## Test gate (nextest)

`run test` (including the `test` stage of `correctness`) requires
`cargo-nextest` (CI installs 0.9.143) and runs:

```sh
cargo nextest run --profile default --ignore-default-filter --no-fail-fast --no-tests=fail --workspace --all-targets
```

No retries, no default-filter exclusions locally: the full suite runs even
after failures, and a missing binary, failed test, or empty selection fails
closed. Criterion 0.5.1 `harness = false` bench targets stay in the
all-targets gate in test mode; explicit `cargo bench` remains the
performance-measurement path. Per-test durations are retained in `test.log`.

Concurrency: `RUST_TEST_THREADS` (first priority), else
`NEXTEST_TEST_THREADS` (default `num-cpus`), selects the gate default;
nextest's `--test-threads=N` CLI option overrides it (place it before any
`--` separator). Unsupported Cargo/libtest and non-executing flags such as
`--no-run` are rejected, never rerouted to another runner. The
`component-host` nextest group keeps the measured four-concurrent-test host
budget; other packages fill the remaining CPU slots. Do not add a process-wide
guest mutex/semaphore or change production Wasmtime limits for a heavy run —
use an explicit environment override for a proven-heavy scope instead.

CI short run: `SOLARIS_NEXTEST_PROFILE=ci` selects the `ci` profile in
`.config/nextest.toml`, which filters out tests that took more than 3 s in
the last local L2 run. The local gate keeps the full suite. Successful
`test` receipts record `memory_peak_bytes`, `memory_limit_bytes`,
`memory_cgroup`, `memory_events`, `memory_swap_peak_bytes`,
`rust_test_threads_env`, `test_runner`, `nextest_test_threads_env`, and
`nextest_profile`; unavailable counters are reported as unavailable, never
zero. Scope peaks include Cargo, compilation, and cached file pages — for a
concurrency decision, build first with `cargo test -p PACKAGE --test BINARY
--no-run`, run only that executable in its own mem-run scope, and read
`memory.stat` anonymous versus file pages.

## Resource bounds

`run` and `client` put the whole invocation into one bounded systemd user
scope before doing any work; every child (Cargo, test binaries, harness
server/client/Xvfb, private backends) inherits that one cgroup, so each bound
is a total, not per-process.

| Property | Default | Env |
|---|---|---|
| `CPUQuota` (+`CPUWeight=50`) | every physical core | `SOLARIS_HARNESS_CPU_QUOTA` |
| `MemoryHigh` | `off` (no throttle) | `SOLARIS_HARNESS_MEMORY_HIGH` |
| `MemoryMax` | `12G` for `test`/`correctness`; `4G` for other profiles and clients | `SOLARIS_HARNESS_MEMORY_MAX` |
| `MemorySwapMax` | `1G` | `SOLARIS_HARNESS_MEMORY_SWAP_MAX` |

```sh
python3 -m tools.harness run correctness              # the defaults above
SOLARIS_HARNESS_CPU_QUOTA=150% python3 -m tools.harness run test
python3 -m tools.harness run test -p mc-core --lib inventory  # focused Rust loop
SOLARIS_HARNESS_MEMORY_MAX=12G python3 -m tools.harness client   # heavier client runs
SOLARIS_HARNESS_CPU_QUOTA=off  python3 -m tools.harness run test  # unbounded, opt-out
```

A cap only protects the session if the run fits in free machine memory:
`run`/`client` refuse admission when `MemAvailable` is below the selected
`MemoryMax` + 1 GiB, naming the knob to change. That refusal is not
acceptance evidence. Full `run test` / `run correctness` take every mem-run
slot exclusively:

```sh
# Full workspace gate: exclusive, all slots.
.analysis/tools/mem-run.sh -x -m 12G env CARGO_BUILD_JOBS=6 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  python3 -m tools.harness run test
# Focused/per-crate work retains the smaller shared scope.
.analysis/tools/mem-run.sh -m 8G env CARGO_BUILD_JOBS=6 \
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p mc-core --lib entity_state_persistence_tests
```

A run without a user systemd scope prints a warning and continues
unbounded; the receipt records the profile's own result either way.

The ignored `mc-core` test `native100_like_entity_phase_micro` is an internal
in-process tick workload, not the real-protocol native-100 capacity gate.
`N100_MOBS` (default `104000`, divisible by eight) and `N100_VILLAGERS`
(default `64`) select its populations; it always joins 100 in-process players
and measures 60 real ticks. For the 100k-mob/20k-villager PGO comparison, set
`N100_MOBS=100000 N100_VILLAGERS=20000` for both training and every candidate.
Build the release test executable in the ordinary build lane, then run it
with `--ignored --nocapture native100_like_entity_phase_micro` in a Main-approved
`-x -p` window. Each `NATIVE100` sample reports total milliseconds and existing
test-only villager brain/roster microseconds; the seven `villager_phase_us`
entries are refresh, admission, evaluation, social, acquisition, full and
skipped. These counters overlap and must not be summed as disjoint phases.
The flat scene has no authored village buildings or workstations, so its
villager timings do not establish inhabited-settlement workload coverage.

## Receipts and failure diagnosis

Each `run` creates an isolated
`.analysis/validation/<UTC stamp>-<profile>-<id>/` directory with
`result.json`, per-command logs, and scenario evidence. `result.json` records
`profile`, `client`, `status`, `scope`, `commands`, `exits` (an unlaunchable command
records `null`, never a fabricated exit code), `logs`
(repo-relative), `started_at`/`ended_at`, `duration_seconds`, `details`, and
`error`. An unknown profile still writes a `failed` (`unknown-profile`)
receipt listing the known profiles. Graphical profiles share one
process/readiness implementation and clean up their owned server, client, and
Xvfb processes; readiness is driven by process, log, and client-state events.

- Start from `result.json`: take the nonzero entry in `exits`, read the
  matching `commands` argv and `logs` file. `details.error` names the failing
  backend; `details.mode` tells preparation (`prepared`) from execution.
- Hosted `test` and `loader` jobs upload their failed receipts as
  `test-failure-receipts` and `loader-failure-receipts` (two-day retention).
  Read the actual command error there; the console's `see .../test.log` line
  alone is not the underlying failure.
- Failing backends fail closed: nonzero commands raise with the log path,
  and `loader-live`/`core-client`/`inventory` additionally raise when the
  scenario report is not `passed`. A `prepared` receipt after expected
  execution means a mode flag was passed (or defaulted) wrong — not a green
  run.
- `KeyboardInterrupt` yields `interrupted`, exit 130, keeping every command,
  exit, and log. Owned processes are stopped by the shared runtime; if a port
  stays held, stop the stray Gradle/Minecraft/Xvfb process — never delete
  `.analysis/validation/` evidence or historical logs.
- Second real-client (multiplayer) failures almost always mean a missing
  `SECOND_AGENT_SECRET`/bridge URL/port pair; the backend errors name the
  exact variable.

## Client launch, check, and credentials

The default (platformless) development client lives under the sibling
`../solaris-loader/fabric-agent/` directory, but despite that name it is a
**NeoForge** mod (entrypoint
`dev.solaris.agent.neoforge.SolarisClientAgentMod`, fixed launch task
`:fabric-agent:runClientMcp`). The three Loader adapters
(`loader-fabric`, `loader-neoforge`, `loader-forge`) register the same
Configuration-state manifest/ack payloads through their native 26.1.2
networking APIs and launch as `:loader-<platform>:runClientMcp`. Set
`SOLARIS_LOADER_ROOT` to use a different Loader checkout; the default is the
sibling `../solaris-loader`, never an in-core copy. Wasm package coverage
comes from `standard-pack` over `../solaris-default-plugins`, not an in-core
plugin copy.

Required credentials for `client` (launch and `--check` alike):

- `SOLARIS_CLIENT_MCP_TOKEN`: random bearer token, no default.
- `SOLARIS_CLIENT_MCP_PORT`: free IPv4 loopback port, default `39095`.
- `SOLARIS_CLIENT_MCP_GAME_DIR`: isolated game dir; default
  `../solaris-loader/fabric-agent/run-mcp`, or
  `.analysis/minecraft-loader-mcp/<platform>` when `--platform` is given.
- `SOLARIS_CLIENT_MCP_USERNAME`: `1..16` ASCII letters/digits/underscores;
  default `SolarisMcp` (platformless) or `SolarisLoader` (Loader platforms).

```sh
SOLARIS_CLIENT_MCP_TOKEN=local-check-token \
  python3 -m tools.harness client --check
export SOLARIS_CLIENT_MCP_TOKEN="$(openssl rand -hex 32)"
export SOLARIS_CLIENT_MCP_PORT=39095
export SOLARIS_CLIENT_MCP_GAME_DIR=.analysis/minecraft-mcp-primary
export SOLARIS_CLIENT_MCP_USERNAME=SolarisMcpA
python3 -m tools.harness client
python3 -m tools.harness client --platform neoforge   # Loader client on another port/dir/user
```

Normal launch refuses to start when the MCP port is already in use. An HTTP
401 means the caller reached an endpoint with a different bearer token —
verify the port and process instead of retrying it as a transient failure.
`--check` validates Java 25, configuration, MCP transport tests, and the
Gradle adapter without launching Minecraft. Use a second port, token, game
directory, and username for multiplayer gates. The endpoint is
`http://127.0.0.1:<port>/mcp`.

MCP smoke against an already-running client (`SOLARIS_CLIENT_MCP_TOKEN`
still required):

```sh
python3 -m tools.harness mcp
python3 -m tools.harness mcp --server-address 127.0.0.1:25565 --exercise-input --disconnect
```

The smoke checks the tool catalog, calls `minecraft_observe`, and optionally
connects, waits for Play, reads the block below the player, scans blocks,
lists entities, reads the recipe book, exercises one input, runs a
deterministic in-client scenario (`--scenario-id`), and disconnects.
Inventory, entity, login, and client lifecycle waits block on packet/lifecycle
state notifications; the separate tick notification drives only tick
progression itself. Canonical interaction checks fence reach, raycast, and
authoritative world state first.

The regression runner is fail-closed on scenario provenance: for `--check`
and `--run`, `SOLARIS_REAL_CLIENT_AGENT_SCENARIO` must name exactly one
scenario in `SOLARIS_REAL_CLIENT_MANIFEST`; an implemented but undeclared
debug scenario is not valid evidence for a no-debug playable manifest.

## Scenario catalog

`python3 -m tools.harness explain` lists scenario ids; `explain <id>` prints
one entry's profile, exact selector, required tests, declared prerequisites,
assertion sources, and what it proves and does not prove. `explain` starts no
server or client, downloads nothing, needs no credentials, and writes no
artifact. `run scenario <id>` dispatches to that existing profile plus its
exact selector through the one shared run path (same artifact directory,
bounded scope, receipt writer) with no free-form trailing arguments. The
receipt must show the selected check actually ran: `details.named_scenario`
records the id, evidence kind, resolved profile and selector, required test
paths, bound `scenario_id`/manifest/seed/fixture, prerequisites, assertion
sources, proves/limits, participating repository revisions, per-command cwds,
engine counts (`selected`/`executed`/`passed`/`failed`/`ignored`/
`filtered_out`), and any missing tests. A named run fails when the id is
unknown, extra arguments are passed, a required test is absent from the log,
the engine reports no counts, nothing was selected or executed, or a selected
test failed. Graphical entries never fall back to TCP evidence.

## Native client gameplay

`java` (real Mojang 26.1.2 client under the development mod loader) is the
default. `native` runs the sibling `../solaris-client` headless
(`offline-test --automation --headless`) against the harness-started server
over ephemeral loopback MCP — no Java, no Xvfb. Descriptor readiness uses
inotify complete-write events and process pidfd, followed by authenticated
MCP initialization; scenario readiness uses MCP state events, never sleeps.
The `survival` server uses an isolated seed-81 world with no plugins by
default, no operators, and hostile spawning disabled; by default it builds
the current debug server, while `SOLARIS_SERVER_BINARY` may select a known
matching prebuilt read-only (path and SHA-256 are receipted). CPU capacity is
derived by the native runtime; probes use the current core v2 configuration.
Retired `worldgen_mode` and `storage_mode` keys do not select runtime behavior.

Native prerequisites come from the native client's verified local
content/cache environment (`docs/AGENT_TESTING.md` in that sibling); the
harness inherits them and never downloads or fabricates private metadata.
Relevant variables: `SOLARIS_CLIENT_ROOT` (default `../solaris-client`;
debug binary built with debuginfo disabled when absent),
`SOLARIS_CLIENT_BINARY` (explicit override; missing override fails rather
than falling back), `SOLARIS_CLIENT_DATA_DIR` (required for the state-only
headless launcher), plus gameplay metadata overrides (including collision
shapes and terrain facts) installed below its `reports/` directory. Each run
uses an isolated descriptor/token directory under the native client's
`.analysis/harness-runs/` sandbox; the 0600 bearer token and private nonce
are not receipted. Normal server runs keep tracing events in
`logs/latest.log` and `logs/debug.log`; the canonical `run` entrypoint sets
`SOLARIS_HARNESS_LOG_STDOUT=1` for its subprocesses, adding an INFO stdout
layer so readiness events and per-run captured logs stay available
(`RUST_LOG` controls `logs/debug.log`, not the captured threshold; never
enable the stdout layer for ordinary operator launches).

For graphical (windowed native + capture) validation add `--windowed`:

```sh
python3 -m tools.harness run core-client --client native --windowed
```

This owns Xvfb, launches `play --windowed`, and additionally requires the
pinned `SOLARIS_CLIENT_RESOURCE_ROOT`, `SOLARIS_CLIENT_ASSET_ROOT`, and
`SOLARIS_CLIENT_ASSET_INDEX`. After success, `screenshots/native-final.png`
and its JSON receipt retain an actual-window scene target with frame id,
scene/state/resource revisions, and GPU completion; missing capture evidence
fails the run. `present_requested` does not prove physical monitor scanout.
The default remains functional headless and makes no visual claim.

Named native scenarios (all use the same isolated server/native lifecycle
and `ProbeSteps` receipts; per-scenario `scenario.json` and
`transcript.json` are retained even on failure; default deadline 180 s):

- `native-v2-p1-*` (armor-stand, signs, piglin, trader, leaves) and
  `native-v2-p2-{slime,enderman,fishing,creaking}`: `--client native
  --windowed` windowed runs with declared operator fixtures and ordinary
  block/menu/input actions. Rendered name/text/color/glow, piglin admiration,
  and merchant offers need screenshot review; inventory debit or menu
  admission alone is not visual acceptance. Action/renderer gaps fail closed.
- `native-v2-p4-{nameplates,advancements,damage,hopper,statistics}`: stop and
  restart the owned server on the same binary/world, reconnect the same
  player, and capture the recovered state. Statistics re-requests after
  restart must not double-count absolute mining/pickup counters.
- `native-v2-p5-humanoid-armor`: one windowed observer plus one headless
  actor; dyes and summon-equipment are required and fail closed.
- `native-v2-light-seam`: chunk-admission lighting gate comparing client
  `minecraft_scan_blocks`, the server's `LevelChunkWithLight`/`LightUpdate`
  stream (loopback-decoded), and a vanilla-propagation oracle across a
  24x7x16 border region; fails on any fully dark admitted chunk. Requires
  `--client native --windowed` and `SOLARIS_SERVER_BINARY`.
- `native-v2-redstone`, `-stations`, `-brewing`, `-jukebox`,
  `-tnt-fire-light`, `-dimensions`, `-ranged`, `-automation-storage`,
  `-storage-adversarial`: bounded received-state probes with exact
  conservation assertions. Headless is received-state evidence only;
  `--windowed` adds the shared final capture, not full pixel parity.
- `native-v2-p1-helmet-equip`, `native-v2-p1-player-observer` (two windows,
  explicit focus handoff), and the `minecraft_use_item_on` cursor passes:
  see `explain` for exact fixtures and fences.
- `java-v2-p5-parity` / `java-v2-p5-towns` and `loader-v2-settlements-smoke`
  run the real Java/Loader clients under private Xvfb against a frozen
  candidate (`SOLARIS_SERVER_BINARY` / `--server-bin` plus `--plugins-dir`);
  they never build a server. Towns stops each JVM between identities and
  repeats checks after a same-server/world restart.
- `survival`, `inventory`, `entities`, the native `core-client` subset, and
  `native-lifecycle` accept one optional `--plugins-dir PATH` with
  `--client native` (existing directory used directly as
  `[plugins].directory`). This generic option supplies no plugin-specific
  scenarios or unlock proofs.

V2 operator fixtures use the vanilla `ops.json` beside the isolated server
config (level 4, offline identities recorded in the receipt). Headless
survival has no operator grant; windowed survival grants its one test
identity for the separately labeled playthrough death fixture only, after
ordinary mining/pickup/placement acceptance. The respawn path uses the
bounded event-driven Play wait; an immediate `in_play=false` right after the
Death-screen Respawn click is not itself a disconnect, but there is no
protocol respawn fallback — a failed UI request, Play timeout, or missing
restored health fails the gate.

## Catalog load

`catalog-load` uses the private `catalog_load` backend, the ordinary **v2
release server**, and a separate `player_scale_bots` process. It builds no
binaries, runs no kernel-only benchmark, and impersonates no rendered
client. `tools/harness/catalog_load_scenarios.json` is the machine-readable
mapping: each stable scenario `id` records fixture/behaviors, wall-time
windows, required work, oracle/invariants, capability blockers, and threshold
provenance. The twelve control recipes live in
`tools/harness/catalog_controls.py`; MIX recipes/validators live in
`tools/harness/catalog_mixes.py`. Runnable means implemented, not an executed
PASS.

```sh
python3 -m tools.harness run catalog-load -- \
  --scenario JOIN-01 \
  --server-bin /absolute/prebuilt/solaris-v2 \
  --bots-bin /absolute/prebuilt/player-scale-bots \
  --clients 10 --smoke --out /absolute/new/catalog-load-smoke
```

Use a new output directory; repeat `--scenario` for explicit selections.
The `JOIN-01` public-command path uses one declared operator setup client to
create a bounded safe platform (65-by-65, five-column grass sample verified
at feet y=64 against actual section-palette cells — advertised heightmaps
are not the oracle), set spawn and gamerules, then issue `save-all flush`;
the received `Saved the game` fence precedes clean stop, hash, and restart
of that same isolated world. Exact `JOIN-01` measured clients have no
operator permissions and no creative/item grants; `join-ready-assisted`
retains the older bounded preparation, creative landing, and dirt grant, and
its `PASS` cannot close the exact catalog case.

Defaults are 10 clients, seed 42, view distance 4, readiness deadline 30 s,
and **20/10/20 wall seconds** for steady/burst/recovery (`--steady-seconds`,
`--burst-seconds`, `--recovery-seconds`, `--readiness-seconds`,
`--view-distance`; `--ramp-clients 1 10 25 50` selects strictly increasing
client rungs). Gameplay starts at a shared ready barrier and a monotonic
epoch; steady/burst/recovery boundaries use the bot's shared schedule epoch,
and tick selection keeps only ticks starting in `[start, end)`. Recovery
offers zero gameplay — connected readers plus mandatory protocol liveness
only, never repair commands. Measured demand never waits on responses; menu
and button bursts use predeclared fixed leads/spacings, and missing
authoritative pose/reach rejects the offer without retry, refill, or
extended windows. Reports after the wall schedule ends acquire no new work
credit; an interrupted rung keeps its precise failure classification.

Receipts separate offered, transport-admitted, skipped/dropped, actually
observed server admission/completion, and client-observed work: writing a
packet is not completion. Outcomes are `UNSUPPORTED_MECHANIC`,
`INFRA_BLOCKED`, `SETUP_FAILED`, `CORRECTNESS_FAIL`, `PERF_FAIL`, or `PASS`;
absent required oracle/telemetry cannot pass. The explicit operator defaults,
not catalog SLOs, are sustained TPS >= 19.5, tick p99 <= 50 ms, maximum tick
<= 250 ms, RSS <= 3072 MiB, and no correctness failures (overrides
`--min-tps`, `--sustained-seconds`, `--max-tick-p99-ms`, `--max-tick-max-ms`,
`--max-rss-mb`; effective values are receipted). Smoke windows total at most
120 seconds and are non-performance evidence: even `PASS` with
`smoke_only: true` is not a capacity/tail-latency result. `--worldgen-gpu`
sets the server's `[server].worldgen_gpu`; the receipt records it.

Runnable control/compound/long scope (see `list` plus the JSON mapping for
current capability):

| ID | Scope |
|---|---|
| `dense-hostile` | 1024 authored hostile UUIDs, real survival/damage/respawn, native-owner proof |
| `spread-passive` | Four 15-by-15-chunk arenas, 256 passives each, wandering plus 1024-UUID native ownership |
| `explore` | Assisted walking, decoded chunks, server snapshot-derived `SetCenterChunk` transitions (diagnostic; not exact CHUNK-01 closure) |
| `built-base` | 64 furnaces, 128 hoppers, 64 loose stacks, 512 physical buttons; real transforms, atomic native stock |
| `fluids` | 128 naturally clocked capped sources; finite plug releases, fresh topology/level cells |
| `village` | 64 actual farmers, legal native Brain, unique declared bed/job claims (not a day-cycle oracle) |
| `combat` | 128 hostiles, 96 finite primed TNT plus 96 chain targets; real blasts, health, removal |
| `projectiles` | 128 skeletons plus 128 finite initial arrows; continued UUID/shooter/victim/damage/removal proof |
| `farms` | All 11440 natural crops and substrates; legal fresh ages, positive measured growth |
| `plugin-heavy` | Current-WIT towns + economy `--plugins-dir`, at least two bots; concurrent town/claim and exact balance queries, idempotent payment, ordinary chest custody, per-package journal commits |
| `checkpoint-restart` | Full mixed base/128 passives/64 villagers/11440 crops, actual save ACK, same-world native restart |
| `long-session` | Finite full base/256 passives, minimum 600 s non-smoke windows, independent resource/queue/work/native-stock proof |
| `MIX-01` … `MIX-21`, `mixed-survival-long` | Separate MIX recipes/validators (tiny for `--smoke`, catalog scale otherwise); a control never silently replaces a compound crisis |

`plugin-heavy` requires `--plugins-dir /absolute/current-WIT/deployment`
with real `solaris-towns` and `solaris-economy` packages; only those
packages are copied intact into the isolated preparation directory. Keep
`RUST_LOG=info` (or at least
`plugin.runtime_ready=info,plugin.command_result=info`); missing state
readback, host evidence, or ordinary tick/resource telemetry fails closed.
`python3 -m tools.coreperf.capacity_ladder` (run inside `mem-run.sh -p`) is the fresh-world
player-capacity ladder (N bots, steady/burst/recovery, per-window phases, thread CPU, network,
checkpoint cost, optional perf capture): see [PLAYER_CAPACITY](performance/PLAYER_CAPACITY.md).

The retired `plugin-settlements` control is gone: core reserves `/settlement`
while the package manifest still declares it; native settlement coverage
lives in `loader-v2-settlements-smoke` with no package deployed. The
`python3 -m tools.coreperf.compare` Modes (`--mobs`, `--mode abab|v2-only`)
are a separate fixture comparison, not catalog closure evidence.

## Constantinople fixture

Deterministic imported-city functional fixture (no throughput or performance
verdict). Keep all third-party archives, worlds, census/plan payloads, and
receipts in ignored `.analysis/`; never commit map or Mojang bytes. Large
scans take the same memory slot and Main's GO as the import itself; heavy
ones run inside an ordinary `.analysis/tools/mem-run.sh -m 6G` slot.

```sh
python3 -m tools.coreperf.constantinople census --world CANONICAL_ANVIL_EXPORT \
  --output .analysis/maps/constantinople/census.json
python3 -m tools.harness run constantinople -- --plan DIR --world NATIVE_WORLD \
  --server-bin ABS_BIN --bots-bin ABS_BOTS --vanilla-data-dir ABS_DATA --seed SEED
```

- `census` scans a stopped native world's `solaris-v2 world export-anvil`
  output (`--counts-only` skips per-cell `terrain.json` for large comparison
  worlds). `solaris-v2 --config FILE world census --source STOPPED_NATIVE
  --receipt OUT.json` counts native entity roots/passengers, villagers,
  stored chunks, settlements, and persisted memberships/households directly;
  the config's seed/generation identity must match, a journal tail newer than
  the checkpoint is refused, and receipts lie outside the world. `solaris-v2
  --config FILE world probe --source STOPPED_NATIVE --receipt OUT.json --block
  X,Y,Z` (repeatable `--block`) reads one stopped world read-only under the same
  identity/journal-tail rules and reports each requested cell's stored block
  state and typed container slots plus every stored player inventory and every
  stored item entity, so custody/duplication questions are answered from the
  saved image instead of a live counter.
- `preservation --source STOPPED_IMPORT_INPUT --world CANONICAL_ANVIL_EXPORT
  --output .../preservation.json` compares import input with the stopped
  native export across every dimension (entity/passenger and block-entity
  counts, stored container/equipment totals including nested containers, each
  player's inventory/ender/equipment). A mismatch writes a FAIL receipt and
  exits nonzero: physical-item count preservation, not byte-exact or
  cosmetic/RNG parity.
- `plan --terrain PATH --output DIR --villagers N --animals 64 --bots K
  --seed 1453` treats N as the target total (tops up existing villagers),
  emitting seeded exclusive home/job pairs, planting commands, green-area
  placements, a cardinal street graph, and closed bot routes.
- `run constantinople` reuses catalog process ownership with an empty
  plugins directory and native console commands: move spawn to each receipted
  quarter-grid centre, `settlement create`, explicitly `settlement bind` each
  cohort UUID, restore the fixture spawn, wait 2400 actual game ticks on the
  dashboard's event-driven clock endpoint, stop cleanly, run native census,
  reopen, and re-check. Neither settlement creation nor membership is
  automatic; bind alone forms no household. The probe binary
  (`player-scale-bots` read-only native snapshot mode) never connects a
  player. Each stopped phase verifies every authored villager UUID and exact
  settlement assignment; reopen census must match the stopped run
  (`--reopen-only` re-checks without repeating create/bind).

## Prerequisites

- Repository root as cwd; `python3`, `cargo`, and (for Java-client profiles)
  Java 25 (`java -XshowSettings:properties -version` must report
  specification 25), an executable `../solaris-loader/gradlew`, and Xvfb for
  every Java graphical profile. Runtime readiness on Linux uses inotify/pidfd
  event waits plus Xvfb process management and MCP state events — not sleeps.
- Loopback must be free on the needed ports (default MCP `39095`, scenario
  server ports such as `25565`/`25567`, per-run agent bridge ports). A run
  that cannot bind fails closed.
- Client launch/check requires credentials in the environment (see above);
  without `SOLARIS_CLIENT_MCP_TOKEN`, `client` exits 2 before Gradle starts.

## Supported per-run environment overrides

The harness never reads a config file for these; export them before `run`:

| Variable | Effect (default) |
|---|---|
| `RUST_TEST_THREADS`, `NEXTEST_TEST_THREADS` | `run test` default concurrency (CPU count; `RUST_TEST_THREADS` wins). Nextest `--test-threads=N` overrides. |
| `SOLARIS_NEXTEST_PROFILE` | Nextest profile (`default` local full suite; CI sets `ci` short run) |
| `SOLARIS_REAL_CLIENT_MANIFEST` | Regression pack manifest (`docs/real-client-regression/manifests/m94-regression-pack.json`; `playable` presets its loop manifest, `replay` its seed-81 manifest) |
| `SOLARIS_REAL_CLIENT_RUN_ROOT` | Local artifact root (preset under the run artifact `regression/` dir) |
| `SOLARIS_REAL_CLIENT_SERVER_CONFIG` | Server config (`example.toml`; `playable` presets `playable.toml`) |
| `SOLARIS_REAL_CLIENT_FRESH_WORLD` | `1` copies the config into the run dir with a fresh per-run world (`playable`/`replay` preset `1`) |
| `SOLARIS_REAL_CLIENT_SERVER_SEED` | Optional signed 64-bit decimal overriding `data.seed` |
| `SOLARIS_REAL_CLIENT_SERVER_ADDR` | Server address for the in-client driver (`127.0.0.1:25565`) |
| `SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS` | Gradle runClient adapter timeout (180; `playable` 1500, `replay` 180). `--timeout-seconds` overrides it for `playable`/`replay`. |
| `SOLARIS_REAL_CLIENT_SERVER_READY_TIMEOUT_SECONDS` | Server-readiness wait (120) |
| `SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL` / `AGENT_SECRET` / `AGENT_PORT` / `AGENT_DRIVER` / `AGENT_SCENARIO` | Loopback JSON bridge URL, per-run bridge secret (required for agent-driver mode), bridge port, internal driver path, scenario id. Second-client mode additionally needs `SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET` plus bridge URL or port. |
| `M79_ORACLE_REPORT_DIR` / `M79_ORACLE_CHECK_ROOT` | Oracle report/check roots (harness presets them under the run artifact) |
| `SOLARIS_ENTITY_BENCH_*` | Scale workload knobs (preset under artifact `bench/`) |
| `SOLARIS_VALIDATION_RUN_DIR` | Canonical run dir injected into every backend env; used by seed-review evidence |

Artifact-building utilities (extract/dump scripts, fixture generators,
`prepare-real-client-scenario.sh`) are not validation entrypoints.

## Boundaries

- `playable` retains the complete twenty-minute no-debug survival loop.
- `replay` is the core seed-81 action replay, not a general regression
  short-cut.
- `regression` defaults to the m94 pack and rejects protocol bots,
  wire-probes, `mc-test-harness` clients, and mocks as participants.
- `oracle` without `--run` reports degraded/blocked readiness only; it needs
  the local artifacts named by each manifest to execute.
- Terrain capture (`seed-review`, `seed-contact-sheet`) never grants owner
  `ACCEPT`; that decision lives outside the harness.
- Retired `entity-scale`/`living-world-scale` results keep their original
  release-build scope as history; use `catalog-load` for current
  public-protocol workloads. Debug numbers are not optimized performance
  evidence.
- Normal operator launches never set `SOLARIS_HARNESS_LOG_STDOUT`,
  `SOLARIS_HARNESS_CPU_SCOPE`, or the memory-cap variables beyond the recipes
  above. Mojang bytes never enter Git; `.analysis/*` and `data/vanilla/*`
  stay ignored.

## Current tools

CodeGraph and the Minecraft client MCP provide code/client integration.
Owner-workstation extras (opencode plugins, RTK, Headroom,
agent-harness, Context7/Serena MCPs, global `/harness-*` commands) are absent
without `~/.config/opencode` and are never required: normal work uses the
repository rules, the harness, and the agent's own tools.

| Tool | Status | Use |
|---|---|---|
| CodeGraph | Codex MCP `codegraph` plus user-wide `codegraph_explore`; Solaris index lives in ignored `.codegraph/` | Targeted symbol-graph questions: callers/callees, mutation paths, lock holders, affected tests, blast radius. Refresh with `codegraph sync .` after edits before relying on it. |
| Minecraft client MCP | Embedded in the sibling Loader's 26.1.2 development client and Fabric/NeoForge/Forge Loader adapters; loopback Streamable HTTP endpoint | Structured real-client observation, connection, inventory/entity waits, input, and reusable multi-client core gates without screenshot assertions |

Do not route provider traffic through proxies or enable MCPs the owner did
not ask for; a forced proxy can break model access.

## Session logs

Inspect metadata before content and select at most three sessions whose
recorded `cwd` matches this repository. Large JSONL logs must be
stream-filtered, never dumped into model context.

| Source | Command |
|---|---|
| Codex CLI JSONL | inspect `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`; verify `session_meta.payload.cwd` first |
| OpenCode session list | `opencode session list` from the repository root |
| OpenCode SQLite DB | `sqlite3 ~/.local/share/opencode/opencode.db`; useful content is usually in `part.data` where `$.type == "text"` (join `part` with `message`/`session`) |

Avoid `opencode export --sanitize` for detailed local forensic work: it
redacts the text/tool payloads that usually contain the useful facts.

## Negative-code gate

For any non-trivial change, the one independent final review required by
`AGENTS.md` includes negative-code checks: scope fit, duplication, fake
abstractions, wider-than-needed config, and stale docs. Adapt the harness to
one final review phase for explicit harness requests; never run multiple
reviewer roles unless the owner explicitly asks. Final reports say whether
the negative-code review ran and what it found.
