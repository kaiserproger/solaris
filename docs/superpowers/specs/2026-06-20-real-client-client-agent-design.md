# Real-Client Client-Agent Design

Date: 2026-06-20
Quality label: stabilization

## Goal

Build the first autonomous real-client evidence path for Solaris by running a
real Minecraft 26.1.2 client with a small injected control agent. The first
deliverable is not a full MCP server and not gameplay parity. It is a
client-side bridge that can prove a real client reached play, expose stable
state observations, execute a minimal scripted action path, and write artifacts
compatible with the existing real-client regression runner.

This replaces the failed launcher-wrapper approach. The runner must no longer
only launch a client and wait for a human-edited `observations.json`; it should
be able to talk to code inside the actual client process.

## Scope

The MVP targets a single loader path first: Fabric-style client mod with
Mixins/accessors. NeoForge support stays a later adapter unless 26.1 local
tooling makes Fabric impossible. The committed repository must contain only
Solaris-owned mod source, build files, scenario specs, and bridge protocol
types. Mojang jars, mappings caches, launcher instances, screenshots, logs, and
decompiled client references remain local-only under `.analysis/`.

The first automated scenario is `m94-02b-rejected-block-resync`: occupied-target
placement rejection, out-of-reach placement rejection, and occupied-target
water-bucket fallback. Other M94 scenarios remain manual-pending until they are
ported one by one.

## Architecture

Add a new `client-mod/solaris-client-agent/` subproject. It builds a
client-only mod jar for the pinned 26.1.x client. The mod starts a loopback-only
JSON bridge after client initialization and registers client-thread commands
through a small executor.

The bridge has three layers:

- Transport: TCP or WebSocket bound to `127.0.0.1` only, with a per-run shared
  secret supplied by the runner.
- Command protocol: JSON request/response messages with stable command names,
  monotonic request ids, timeouts, and structured errors.
- Client adapters: thin hooks over Minecraft client objects for connection,
  world/player state reads, input/action calls, screenshots, current screen,
  selected slot, inventory snapshots, and block-state reads.

The external driver remains outside the client. `tools/run-real-client-regression.sh`
will keep creating local artifact directories, starting Solaris, and collecting
logs. A new driver command will launch the instrumented client, connect to the
bridge, execute scenario steps, and write `observations.json` automatically.

## Command Set

The MVP command set is intentionally small:

- `ping`: proves the bridge is alive and returns client/mod version metadata.
- `connect`: connects the real client to `127.0.0.1:25565` in offline/dev mode.
- `wait_play`: waits until the local player and client world exist.
- `state`: returns dimension, player position, selected hotbar slot, current
  screen, disconnect reason, and loaded block states for requested positions.
- `set_hotbar_slot`: selects a hotbar slot through the client path.
- `look_at_block`: points the camera at a block/face using client state.
- `use_item_on`: triggers the normal client interaction manager path for a
  clicked block face.
- `screenshot`: saves a local screenshot into the current run artifact dir.
- `disconnect`: cleanly leaves the server.

All commands that touch Minecraft state run on the client thread. Bridge I/O
never mutates client state directly.

## Artifacts

Each run writes the existing artifact shape under `.analysis/real-client-runs/`:

- `manifest.json`
- `client.log`
- `server.log`
- `observations.json`
- `screenshots/`
- `git.txt`
- `toolchain.txt`
- `automation-driver.txt`

For agent-driven runs, `observations.json` must include
`"client_gate": "agent-run-real-client"` and each scenario must include command
transcripts, final state snapshots, screenshot paths, and pass/fail status. A
prepared or timed-out run remains non-green.

## Error Handling

The bridge is fail-closed. It refuses non-loopback binds, missing shared secrets,
unknown commands, stale request ids, and commands issued before `wait_play`
succeeds. Every failed command records a structured error code and enough client
state to diagnose the failure without claiming evidence.

The driver treats any bridge disconnect, client crash, server disconnect, action
timeout, missing screenshot, or unfilled observation as a failed real-client run.
It may still preserve artifacts for debugging.

## Validation

The first implementation is accepted only when these gates pass:

- The client mod builds in debug/dev mode without adding Mojang bytes to git.
- The bridge can answer `ping` from a running real client.
- The driver can launch the instrumented client, connect to Solaris, run
  `wait_play`, capture a screenshot, and write a non-prepared observation file.
- `m94-02b-rejected-block-resync` can be executed by the real client through the
  bridge and records pass/fail observations for ghost blocks and held-slot state.
- Existing repository gates still pass for touched Rust/scripts/docs:
  `cargo run -p xtask -- code-health`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo fmt --all -- --check`.

This does not make M94 or the replacement ledger ready. It only creates the
first approved real-client automation path for one focused scenario.

## Non-Goals

- No full MCP server in the MVP. The MCP layer should wrap the CLI after the
  bridge and driver are stable.
- No raw jar patching as the first route.
- No committing Mojang client jars, generated decompiled sources, launcher
  profiles, run logs, or screenshots.
- No broad gameplay automation before the M94 rejected-block scenario works.
