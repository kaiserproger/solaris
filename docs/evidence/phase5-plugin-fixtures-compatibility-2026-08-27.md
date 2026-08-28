# Phase 5 plugin fixtures and client compatibility — 2026-08-27

## Scope

This checkpoint closes Phase 5 item 7:

> Ship server-only and client-required fixtures, API documentation, permission examples, harness coverage, and vanilla-client/Loader compatibility gates.

The close condition is the repository's explicit plugin client matrix from `manual-client-test-gates.md`: a server-only plugin must accept the ordinary real-client automation profile without Solaris Loader; the same no-Loader profile must be rejected clearly by a client-required fixture; and every claimed Loader platform must complete the exact permission/bundle handshake and enter Play.

## Shipped fixtures and documentation

Server-only production examples remain under `examples/plugins/`, including:

- `basic-economy`;
- `land-claims`;
- `colony-villager-scaffold`;
- `geological-mines`;
- `online-roster`;
- `settlement-prototype`.

Their public API is documented in `docs/PLUGINS.md`, including the derived `server_only | server_and_client` deployment model, client bundle schema, supported Loader platforms, exact permissions/content kinds, plugin capabilities, storage, timers, commands, menus, world/entity/player mutation APIs and targeted result handlers.

The client-required two-owner fixture is `examples/loader-live-gate/`. Its README now exposes repeatable commands for both the three Loader profiles and the complementary no-Loader compatibility gate.

`tools/build-loader-live-gate-fixture.sh --check` passes:

```text
Loader live-gate fixture is reproducible and current.
```

`cargo test -p mc-script --features lua-runtime lua::loader_tests -- --nocapture` is 11/11 PASS. It covers server-only discovery, all Loader content/permission/cache fences, invalid artifact/hash/path rejection, dependency ordering, and the shipped two-owner fixture.

`cargo test -p mc-server --test cli -- --nocapture` is 38/38 PASS, including derived deployment reporting.

## No-Loader real-client matrix

`tools/run-plugin-client-compat-gate.py` runs an isolated Xvfb display, server processes, worlds and client game directories. The client launcher is `tools/run-minecraft-client-mcp.sh`, i.e. the normal real-client automation profile, not any `loader-*` project.

The `:fabric-agent:runtimeClasspath` dependency report contains `bridge-core` and the Minecraft/NeoForge dev runtime but no `loader-core`, `loader-fabric`, `loader-neoforge`, or `loader-forge` dependency. The profile therefore provides client automation but does not implement the Solaris Loader Configuration payloads.

Fresh result:

```text
.analysis/plugin-client-compat/20260827T092538/result.json
```

### Server-only fixture accepts the no-Loader client

The gate copies shipped `basic-economy` into an isolated plugin root and starts a fresh server/world. The no-Loader client:

- reaches Play;
- sends `/economy` through the real client command path;
- receives the plugin's real server-owned `ChestMenu`;
- observes `net.minecraft.client.gui.screens.inventory.ContainerScreen`.

Result excerpt:

```text
server_only.passed = true
server_only.in_play = true
server_only.plugin = basic-economy
server_only.economy_menu_screen = net.minecraft.client.gui.screens.inventory.ContainerScreen
```

This proves the server-only plugin path does not require the Solaris Loader handshake and remains usable from the ordinary client compatibility path.

### Client-required fixture rejects the same no-Loader client

The same client launcher, still without any Solaris Loader module, is then connected to a fresh isolated `examples/loader-live-gate` server. It does not enter Play. `MinecraftClientObservation` now exports the public vanilla `DisconnectedScreen.getNarrationMessage()` as read-only `disconnect_reason`, allowing the gate to assert the actual user-visible reason instead of parsing a log or private field.

Exact observed reason:

```text
Connection Lost. This server requires Solaris Loader. Supported loaders: Fabric, NeoForge, Forge. Required bundles: ruby-live:rich-content@1, sapphire-live:rich-content@1. Install Solaris Loader and reconnect.
```

Result:

```text
client_required_rejection.passed = true
client_required_rejection.in_play = false
```

This closes the no-Loader accept/reject half of the manual matrix with a real client rather than only raw TCP.

## Three-platform Loader real-client matrix

`tools/run-loader-live-gate.py <fabric|neoforge|forge>` now owns the repeatable platform gate. Each invocation creates an isolated Xvfb display, fresh world, per-run game directory, per-run Loader cache/permission directory, server/client logs and `result.json`.

The gate requires, in order:

1. production Solaris server readiness;
2. real Loader client MCP readiness and TitleScreen bootstrap;
3. exact Loader permission confirmation;
4. Play transition;
5. exactly two cached bundle identities;
6. `/loader_ruby` -> exact `Ruby Loader Fixture` screen -> `Confirm Ruby` -> first carrier item grant;
7. `/loader_sapphire` -> exact `Sapphire Loader Fixture` screen -> `Confirm Sapphire` -> second carrier item grant;
8. client still in Play after both owner actions.

The cached bundle identities are identical on all three platforms:

```text
ruby-live/rich-content/1/70dd527ac0c5075faf1dff65e8e426f657746d42215e4fc4fd18244ac5b9d765.bundle
sapphire-live/rich-content/1/6c16425b2bf9c5415184345c4cb6bc10e98bf41a3e73dc27b3915aa7962418a5.bundle
```

### Fabric

Fresh artifact:

```text
.analysis/loader-live-gate/runs/20260827T092358-fabric/result.json
```

Result:

```text
passed = true
permission_confirmed = true
bundle_cache_count = 2
Ruby inventory paper count = 1
Sapphire inventory paper count = 2
in_play_after_owner_actions = true
```

### NeoForge

Fresh artifact:

```text
.analysis/loader-live-gate/runs/20260827T092432-neoforge/result.json
```

Result is the same semantic PASS: permission confirmed, both exact bundles cached, both owner screens/buttons succeed, grants reach counts 1 then 2, and the client remains in Play.

### Forge

Fresh artifact:

```text
.analysis/loader-live-gate/runs/20260827T092505-forge/result.json
```

Forge now passes the same semantic gate and is no longer an unrun platform.

The first Forge attempt exposed an unrelated incomplete debug-only loading-warning helper in `SolarisForgeLoader` which referenced unavailable symbols and prevented compilation. The helper was removed rather than binding production Loader code to unstable Forge loading-screen internals. `:loader-forge:test` then passed and the fresh real Forge run above completed the full gate.

## Per-run cache isolation

The audit also found that Fabric already put its Loader cache under its per-run game directory, while NeoForge and Forge still fell back to `~/.solaris/loader-cache`. Both MCP profiles now set:

```text
solaris.loader.cacheDir=<per-run-game-dir>/solaris-loader-cache
```

This makes permission decisions and exact bundle caches isolated across all three platforms and prevents one platform/run from satisfying another run's handshake via stale global state.

Both NeoForge and Forge `tools/run-loader-client-mcp.sh <platform> --check` launch-graph checks pass after the change.

## Platform package gates

The complete Loader unit/package suite passes:

```text
./gradlew --no-configuration-cache \
  :loader-core:test \
  :loader-fabric:test \
  :loader-neoforge:test \
  :loader-forge:test

BUILD SUCCESSFUL
```

The Java client observation and real-client bridge also pass:

```text
./gradlew --no-configuration-cache :java-agent:test :fabric-agent:classes
BUILD SUCCESSFUL
```

Both Python gate drivers compile with `python3 -m py_compile`.

## Harness and public API coverage

The previously closed item-5 shipped plugin suite remains 5/5 PASS and covers actual `basic-economy`, `land-claims`, inventory, protection and colony/villager compositions. Item 7 adds the real-client deployment/compatibility proof on top of that functional harness coverage.

The compatibility gate does not claim that every future plugin UI/content feature is represented. It proves the public-alpha deployment contract that is currently advertised: server-only plugins work without Solaris Loader; client-required plugins reject clients that do not implement the Loader contract; Fabric, NeoForge and Forge all complete the same exact two-owner permission/bundle/content path.

## Quality gates

- Loader fixture reproducibility check — PASS;
- `mc-script` Loader tests — 11/11 PASS;
- `mc-server` CLI/deployment tests — 38/38 PASS;
- full Loader Gradle package suite — PASS;
- Java agent tests / Fabric agent classes — PASS;
- no-Loader real-client accept/reject matrix — PASS;
- Fabric real Loader gate — PASS;
- NeoForge real Loader gate — PASS;
- Forge real Loader gate — PASS;
- Python gate-driver syntax — PASS;
- scoped `git diff --check` — PASS;
- `cargo fmt --all -- --check` — PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo clippy --workspace --all-targets -- -D warnings` — PASS.

Benchmark: not applicable. These are startup/connection/content compatibility gates, not a steady-state runtime path.

## Independent review

Exactly one bounded independent read-only reviewer returned **CHANGES** with four finite findings, all in gate strictness/documentation rather than Loader runtime behavior:

1. compare cached bundle paths against the exact Ruby/Sapphire identities instead of only checking count;
2. require the no-Loader disconnect narration to name Solaris Loader, Fabric, NeoForge, Forge, and both required bundle ids;
3. describe the automation shell accurately as a no-Loader profile rather than literally unmodified vanilla;
4. restore the required MCP bearer-token environment variable in the manual launcher example.

All four findings are fixed. No second reviewer was started. Post-fix self-validation reran the three real Loader clients and the no-Loader matrix successfully: Fabric `20260827T092358-fabric`, NeoForge `20260827T092432-neoforge`, Forge `20260827T092505-forge`, and no-Loader matrix `20260827T092538`. Scoped `git diff --check` remains PASS.

## Disposition

Phase 5 item 7: **CLOSED**. Server-only and client-required fixtures, API/permission documentation, harness coverage, no-Loader accept/reject behavior, and fresh Fabric/NeoForge/Forge real-client compatibility gates are reproducible on the current tree.
