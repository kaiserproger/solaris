# Agent Tooling

This file keeps agent setup details out of milestone docs. `AGENTS.md`
contains the rules; this file contains the local wiring.

## Current Tools

| Tool | Status | Use |
|---|---|---|
| CodeGraph | installed globally via npm as `@colbymchenry/codegraph@1.2.0`; Codex MCP server `codegraph` registered; omp user-wide MCP `~/.omp/agent/mcp.json` exposes `codegraph_explore` (the v1.2.0 MCP surface is explore-only; `callers`/`callees`/`impact` stay CLI); rust-analyzer via omp `lsp.yml` answers hover/references; telemetry disabled; Solaris index lives in ignored `.codegraph/` | Targeted symbol graph questions: callers/callees, mutation paths, lock holders, affected tests, and blast-radius checks. Refresh with `codegraph sync .` after edits before relying on it. |
| Serena | enabled globally through opencode MCP | Optional targeted Rust symbol search/editing and project memories; do not use it as mandatory startup context. |
| Context7 | enabled globally through opencode MCP and verified 2026-06-11 | External library/framework docs. Use `resolve-library-id` before `query-docs`. |
| RTK | installed at `/home/kaiserroman/.cargo/bin/rtk` | Compact shell output. OpenCode plugin installed globally at `~/.config/opencode/plugins/rtk.ts`; restart opencode before relying on auto-rewrite. |
| Headroom | installed by `uv tool install "headroom-ai[all]"` at `/home/kaiserroman/.local/bin/headroom` | Optional context compression/proxy/MCP/learning. Do not route opencode provider traffic through Headroom unless explicitly asked. |
| Agent harness | installed globally under `~/.config/opencode/bin/agent-harness` | Optional opencode workflow only when explicitly requested; normal Codex work uses the repo rules and exactly one final reviewer. |
| Minecraft client MCP | embedded in the repo's Fabric 26.1.2 development client; optional Loader clients also support NeoForge and Forge; loopback Streamable HTTP endpoint | Structured real-client observation, connection, inventory/entity waits, input, selected-item drop, and reusable multi-client core gates without screenshot assertions. |

## Validation Harness

The repository validation entrypoint is `python3 -m tools.harness`, run from the
repository root. Profiles reuse the existing Cargo, Gradle and scenario engines;
they do not lower the acceptance bar or turn preparation into gameplay evidence.
Never invoke `tools/harness/backends/` directly: those engines are private, and
only a profile run leaves a canonical receipt. There are 23 profiles; `list`
is authoritative if this table drifts:

```sh
python3 -m tools.harness list
python3 -m tools.harness run PROFILE [--release] [--timeout-seconds N] [--platform fabric|neoforge|forge] [backend args]
python3 -m tools.harness client [--check] [--platform fabric|neoforge|forge]
python3 -m tools.harness mcp [--server-address host:port] [--exercise-input] [--disconnect]
```

### Prerequisites

- Repository root as cwd; `python3`, `cargo`, and (for client profiles) Java 25
  (`java -XshowSettings:properties -version` must report specification 25),
  an executable `../solaris-loader/gradlew`, and Xvfb for every
  graphical profile. Runtime readiness on Linux uses inotify/pidfd event waits
  plus Xvfb process management and MCP state events — not sleeps.
  Set `SOLARIS_LOADER_ROOT` to use a different Loader checkout; the default
  is the sibling `../solaris-loader`, never an in-core compatibility copy.
- Loopback must be free on the needed ports (default MCP `39095`, scenario
  server ports such as `25565`/`25567`, per-run agent bridge ports). A run that
  cannot bind fails closed; there is no invented exit code.
- Client launch/check requires credentials in the environment (see below).
  `python3 -m tools.harness client` without `SOLARIS_CLIENT_MCP_TOKEN` exits 2
  before Gradle starts.

### Profiles and recipes

| Profiles | Coverage and recipe |
|---|---|
| `correctness` | Rust L2: formatter, strict workspace Clippy, code-health, workspace/all-target tests. `python3 -m tools.harness run correctness` |
| `fmt`, `clippy`, `code-health`, `test` | The individual Rust gates; CI uses the same profiles. |
| `harness-check` | Fail-closed receipt/process lifecycle regressions plus private shell-engine syntax (`bash -n` over every backend). `python3 -m tools.harness run harness-check` |
| `build` | Debug server build (`cargo build --bin mc-server`); `--release` explicitly selects the locked release workspace build (`cargo build --locked --release --workspace`). `python3 -m tools.harness run build --release` |
| `java` | Gradle bridge/agent/Loader module tests (`:bridge-core`, `:java-agent`, `:loader-core`, `:loader-fabric`, `:loader-neoforge`, `:loader-forge`). Requires a Minecraft client jar via `SOLARIS_CLIENT_JAR` or the Loader's documented local path. CI downloads the version declared in Loader `gradle.properties` and verifies Mojang's SHA-1 before testing. `python3 -m tools.harness run java` |
| `fixture-check` | Reproducible Loader fixture verification (`tools/build-loader-live-gate-fixture.sh --check`); requires `ffmpeg` with `libvorbis`, plus ZIP and coreutils commands. CI installs the audio encoder explicitly. |
| `installer` | Installer self-test backend. |
| `core-client` | Real no-Loader client: natural currency mining, plugin purchase and required-Loader rejection (direct `compatibility.run`, no inventory path). `python3 -m tools.harness run core-client` |
| `inventory` | Inventory-path gate: crafting, cursor round-trip and held-item selection in an isolated operator fixture, on top of the core loop. `python3 -m tools.harness run inventory` |
| `loader-live` | Loader gate via `loader.run`; `--platform` is required: `python3 -m tools.harness run loader-live --platform neoforge` (also `fabric`, `forge`). |
| `regression` | Real-client regression backend over `regression.sh`; bare run defaults to `--run`. `python3 -m tools.harness run regression --check` prepares only. |
| `playable` | Ordinary twenty-minute no-debug survival loop (`docs/playable/real-client-playable-loop.json`, scenario `playable-04-twenty-minute-survival-loop`, `playable.toml`, fresh world, default timeout 1500 s). `python3 -m tools.harness run playable` executes; `--check`/`--prepare` only prepare. |
| `replay` | Core replay seed-81 gate (`docs/real-client-regression/manifests/core-replay-seed-81.json`, scenario `core-actions-seed-81`, fresh world, default timeout 180 s). `python3 -m tools.harness run replay` |
| `oracle` | Vanilla oracle backend: bare `oracle` is readiness inspection (`prepared`, blocked/degraded when artifacts are absent); `python3 -m tools.harness run oracle --run` executes comparisons. |
| `oracle-check` | Oracle runner self-check backend. |
| `entity-scale`, `living-world-scale` | Explicit performance workloads. Both run `entity_scale.sh` in release mode (`cargo test --release -p mc-test-harness --test load_scenarios`); living-world presets 50 clients / 25 regions / 4000 entities per region with p95 50 ms and p99 60 ms budgets. Evidence lands under the run artifact `bench/` dir. |
| `seed-review` | Seed owner-review backend (module invocation, default timeout 120 s). Captures evidence; it does not grant owner `ACCEPT`. |
| `seed-contact-sheet` | Seed contact-sheet capture preparation; always `prepared`, never a gameplay pass. |
| `bucket-resync` | Bucket resynchronization debug-loop backend. |

`--release` only changes `build`; every other profile ignores it.
`--timeout-seconds N` overrides the scenario default for `core-client` /
`inventory` (600 s), `loader-live` (600 s), `playable` (1500 s) and `replay`
(180 s); regression/playable/replay require positive whole-second values.
Fractional values fail explicitly. Unknown profiles, nonzero commands,
exceptions, and prepared-only runs never pass.

### Preparation versus execution

`regression`, `playable` and `replay` append `--run` unless a mode token is
already present (`--check`, `--prepare`, `--run`, `--validate-run`). `--check`
validates the Gradle runClient adapter; `--prepare` creates a run directory and
observation templates; `--run` starts Solaris, launches the client, and records
logs; `--validate-run <dir>` checks an existing run shape. `--check` and
`--prepare` receipts carry `mode: prepared` and exit 0 with status `prepared` —
that is explicitly not a gameplay pass. `oracle` without `--run` and
`seed-contact-sheet` always behave the same way.

### Client and MCP launch, check, and credentials

The default (platformless) development client lives under the
`../solaris-loader/fabric-agent/` directory, but despite that
directory name it is a **NeoForge** mod: it applies `net.neoforged.moddev`,
its entrypoint is `dev.solaris.agent.neoforge.SolarisClientAgentMod`, and its
fixed launch task is `:fabric-agent:runClientMcp`. The three Loader adapters
(`loader-fabric`, `loader-neoforge`, `loader-forge`) register the same
Configuration-state manifest/ack payloads through their native 26.1.2
networking APIs and launch as `:loader-<platform>:runClientMcp`.

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

Normal launch refuses to start when the MCP port is already in use; the
in-client bind remains the final authority if a process races for the port.
Explicit token/port values override stale JVM properties from an earlier run.
An HTTP 401 means the caller reached an endpoint with a different bearer token
— verify the port and process instead of retrying it as a transient failure.
`--check` validates Java 25, configuration, MCP transport tests and the Gradle
adapter without launching Minecraft. Use a second port, token, game directory,
and username for multiplayer gates. The endpoint is
`http://127.0.0.1:<port>/mcp`.

MCP smoke against an already-running client (`SOLARIS_CLIENT_MCP_TOKEN` still
required; default endpoint derives from `SOLARIS_CLIENT_MCP_PORT`):

```sh
python3 -m tools.harness mcp
python3 -m tools.harness mcp --server-address 127.0.0.1:25565 --exercise-input --disconnect
```

The smoke checks the tool catalog, calls `minecraft_observe`, and optionally
connects, waits for Play, reads the block below the player, scans blocks,
lists entities, reads the recipe book, exercises one input, runs a
deterministic in-client scenario (`--scenario-id`), and disconnects. Inventory,
entity, login, and client lifecycle waits block on packet/lifecycle state
notifications; the separate tick notification drives only tick progression
itself. Every timeout is failure, not success. The current bridge also provides
push-driven motion and entity-removal waits; ordinary primary/secondary
container-slot clicks wait for an applied server container update, so plugin
inventory menus work without coordinate clicks. Canonical interaction checks
fence reach, raycast, and authoritative world state first. Focused bridge,
Java, and client-mod tests are tooling-path evidence only.

The regression runner is fail-closed on scenario provenance: for `--check` and
`--run`, `SOLARIS_REAL_CLIENT_AGENT_SCENARIO` must name exactly one scenario in
`SOLARIS_REAL_CLIENT_MANIFEST`; an implemented but undeclared debug scenario is
not valid evidence for a no-debug playable manifest.

### Supported per-run environment overrides

The harness never reads a config file for these; export them before `run`:

| Variable | Effect (default) |
|---|---|
| `SOLARIS_REAL_CLIENT_MANIFEST` | Regression pack manifest (`docs/real-client-regression/manifests/m94-regression-pack.json`; `playable` presets its loop manifest, `replay` its seed-81 manifest). |
| `SOLARIS_REAL_CLIENT_RUN_ROOT` | Local artifact root; the harness presets it under the run artifact `regression/` dir (backend default `.analysis/real-client-runs`). |
| `SOLARIS_REAL_CLIENT_SERVER_CONFIG` | Server config (`example.toml`; `playable` presets `playable.toml`). |
| `SOLARIS_REAL_CLIENT_FRESH_WORLD` | `1` copies the config into the run dir with a fresh per-run world (`playable`/`replay` preset `1`). |
| `SOLARIS_REAL_CLIENT_SERVER_SEED` | Optional signed 64-bit decimal overriding `data.seed`. |
| `SOLARIS_REAL_CLIENT_SERVER_ADDR` | Server address for the in-client driver (`127.0.0.1:25565`). |
| `SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS` | Gradle runClient adapter timeout (180; `playable` 1500, `replay` 180). `--timeout-seconds` overrides it for `playable`/`replay`. |
| `SOLARIS_REAL_CLIENT_SERVER_READY_TIMEOUT_SECONDS` | Server-readiness wait (120). |
| `SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL` / `AGENT_SECRET` / `AGENT_PORT` / `AGENT_DRIVER` / `AGENT_SCENARIO` | Loopback JSON bridge URL, per-run bridge secret (required for agent-driver mode), bridge port, internal driver path, scenario id. Second-client mode additionally needs `SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET` plus bridge URL or port. |
| `M79_ORACLE_REPORT_DIR` / `M79_ORACLE_CHECK_ROOT` | Oracle report/check roots (harness presets them under the run artifact). |
| `SOLARIS_ENTITY_BENCH_*` | Scale workload knobs (`MODE`, `OUT_DIR` preset under artifact `bench/`, clients/regions/entities, warmup/measure ticks, budgets, cpuset). |
| `SOLARIS_VALIDATION_RUN_DIR` | Canonical run dir injected into every backend env; used by seed-review evidence. |

Existing `SOLARIS_REAL_CLIENT_*` scenario/configuration overrides remain
supported. Artifact-building utilities are not duplicate validation entrypoints.

### Artifact and result lookup

Each `run` creates an isolated `.analysis/validation/<UTC stamp>-<profile>-<id>/`
directory with `result.json`, per-command logs, and scenario evidence. `status`
is `passed`, `failed`, `prepared`, or `interrupted`; only real completion is
`passed`, and failure exits nonzero. `result.json` records `profile`, `status`,
`scope`, `commands`, `exits` (a command that cannot start keeps an invented
`null`, never a fabricated code), `logs` (repo-relative), `started_at` /
`ended_at`, `duration_seconds`, `details`, and `error`. An unknown profile still
writes a `failed` receipt (`unknown-profile`) listing the known profiles.
Graphical profiles share one Linux process/readiness implementation and clean up
their owned server, client, and Xvfb processes; readiness is driven by process,
log, and client-state events.

### Failing-command diagnostics and interruption cleanup

- Start from `result.json`: take the nonzero entry in `exits`, read the matching
  `commands` argv and `logs` file. `details.error` names the failing backend;
  `details.mode` tells preparation (`prepared`) from execution (`run`).
- Hosted `test` and `loader` jobs upload their failed receipts as
  `test-failure-receipts` and `loader-failure-receipts` (seven-day retention).
  Download the artifact to read the actual command error; the console's
  `[harness] ... see .../test.log` line alone is not the underlying failure.
- Failing backends fail closed: nonzero commands raise with the log path, and
  `loader-live`/`core-client`/`inventory` additionally raise when the scenario
  report is not `passed`. `--check`/`--prepare` never produce a gameplay pass,
  so a `prepared` receipt after you expected execution means a mode flag was
  passed (or defaulted) wrong — not a green run.
- `KeyboardInterrupt` (Ctrl-C) yields status `interrupted`, exit 130, and keeps
  every command, exit, and log recorded so far. Owned server/client/Xvfb
  processes are stopped by the shared runtime; if a port is still held after an
  interruption, find the stray Gradle/Minecraft/Xvfb process holding it and stop
  that process — do not delete `.analysis/validation/` evidence or historical
  logs.
- Second real-client (multiplayer) failures almost always mean a missing
  `SECOND_AGENT_SECRET`/bridge URL/port pair; the backend errors name the exact
  variable.

### Original scenario and performance boundaries

- `playable` retains the complete twenty-minute no-debug survival loop; do not
  shorten it or substitute a debug scenario and call it playable.
- `replay` is the core seed-81 action replay, not a general regression short-cut.
- `regression` defaults to the m94 pack and rejects protocol bots, wire-probes,
  `mc-test-harness` clients, and mocks as participants.
- `oracle` without `--run` reports degraded/blocked readiness only; it needs the
  local artifacts named by each manifest to execute.
- `entity-scale` / `living-world-scale` carry their original release-build and
  budget requirements; numbers from debug builds are not performance evidence.
- Terrain capture (`seed-review`, `seed-contact-sheet`) never grants owner
  `ACCEPT`; that decision lives outside the harness.

### Current evidence and limits (2026-09-06)

- Green: `harness-check` (9 tests plus shell syntax), 49 manifest tests,
  `java` / `installer` / `fixture-check` / `oracle-check`, and the graphical
  inventory plus core-client compatibility gate at
  `.analysis/validation/20260906T084650-inventory-njrj95iy/result.json`.
- Red: the full alpha `playable` loop currently **fails** after natural spruce
  pickup/crafting — no dry crafting-table placement target in the snowy terrain
  under test (Main investigates; never mark green). The frozen load matrix
  remains 20 PASS / 22 FAIL with no owner terrain `ACCEPT`, and the full core
  redesign is incomplete.
- Workspace version is `0.0.3-alpha.1`; the latest **published** release remains
  `v0.0.2-alpha.1`. The frozen local field-test archive is
  `.analysis/releases/v0.0.3-alpha.1/solaris-x86_64-unknown-linux-gnu.tar.gz`.
  It passed isolated version/config/startup checks, not full survival acceptance.
  SHA-256: `925a825b709e5e44d8ad17957d741e3751ed955dba3e510ee4baa8ee78ed6b36`.
- Owner correction: stop fixed-seed tuning. Use the frozen archive for manual
  field testing first, then graphical exploratory runs across varied seeds.
  Record each seed, generated config, steps, logs and screenshots; retain failing
  seeds as reproductions, not as the only terrain used for acceptance.

## Useful External Candidates

| Tool | Decision |
|---|---|
| `lean-ctx` | Researched 2026-06-11. It offers MCP, shell compression, memory, and context governance, including OpenCode setup. Do not install it on top of RTK+Headroom by default; overlapping hooks/context layers can conflict. Revisit only if RTK/Headroom are not enough. |
| Headroom bundled tools | Available through `headroom sg`, `headroom diff`, and `headroom loc` for AST search, structural diffs, and LOC/repo-shape probes. Use explicitly when they add value; do not replace normal repo validation. |

## OpenCode Commands

Global commands already present in `~/.config/opencode/commands/`:

| Command | Purpose |
|---|---|
| `/agent-harness` | Router that points to the native harness commands. |
| `/harness-run` | Spec-first implementation with native subagent cards. |
| `/harness-refactor` | Behavior-preserving refactor with review gates. |
| `/harness-cleanup` | Behavior-preserving repo slop cleanup. |
| `/harness-cleanup-cli` | Old all-in-CLI cleanup path; use only when explicitly requested. |
| `/harness-preflight` | Deterministic agent/config/repo checks. |
| `/harness-dry-run` | Generate harness prompts/artifacts without LLM phases. |

## Headroom Notes

Headroom's own CLI help says `headroom wrap opencode` does not exist. The
supported opencode route is `headroom proxy` plus provider base URL overrides.
Do not enable that automatically in this repo because the global opencode setup
uses provider auth/plugins and a forced proxy can break model access.

Headroom MCP is not enabled in `opencode.json`. If the owner asks for it, use
`/home/kaiserroman/.local/bin/headroom mcp serve` as the local command and
check startup cost before leaving it enabled.

## Session Logs

Inspect metadata before content and select at most three sessions whose recorded
`cwd` matches this repository. Large JSONL logs must be stream-filtered rather
than dumped into model context.

| Source | Command |
|---|---|
| Codex CLI JSONL | inspect `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`; verify `session_meta.payload.cwd` first |
| OpenCode session list | `opencode session list` from `/home/kaiserroman/solaris` |
| OpenCode SQLite DB | `sqlite3 ~/.local/share/opencode/opencode.db` |
| Text parts | Query `part` joined with `message`/`session`; useful content is usually in `part.data` where `$.type == "text"`. |

Avoid `opencode export --sanitize` for detailed local forensic work because it
redacts the text/tool payloads that usually contain the useful facts.

## Negative-Code Gate

For any non-trivial change, include negative-code checks in the one independent
final review required by `AGENTS.md`:

| Diff size | Gate |
|---|---|
| Small/single-file | Self-review, then ask one concise independent reviewer to check scope, duplication, fake abstractions, wider-than-needed config, and stale docs. |
| Non-trivial or risky | Give the same single reviewer the concrete behavior, diff scope, and validation evidence. Do not add a separate slop-review agent. |
| Explicit harness request | Adapt the harness to one final review phase; do not run multiple reviewer roles unless the owner explicitly asks. |

Final reports should say whether the negative-code review ran and what it found.
