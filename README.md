# Solaris

Solaris is an authoritative Minecraft Java Edition server written in Rust. It
targets the vanilla **Minecraft Java Edition 26.1.2** protocol and also supports
optional client content through Solaris Loader.

> **Development status:** **`v0.0.7`** is a preliminary release.
> Maturity remains **draft**, not release-ready. Solaris is suitable for
> testing, plugin development, and bounded multiplayer field tests; it is not a
> production-safe replacement for vanilla, Paper, Fabric, Forge, or NeoForge.
> Alpha worlds, plugin APIs, and Loader contracts may change without migration.

Solaris ships an optional default-off read-only web dashboard
(`[dashboard]` in the config, see [`docs/OPERATING.md`](docs/OPERATING.md))
and the `operator add|remove|list` CLI, plus a `pregenerate` subcommand that
generates and stores a bounded block-coordinate region before startup. The
standard plugin pack is explicitly opt-in; see
[plugin installation](docs/PLUGINS.md#standard-plugin-pack).

## Install v0.0.7 (Linux)

Published archives are available for Linux x86_64 and AArch64. Pin the release
because GitHub's `latest` alias does not resolve prereleases:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.7/install.sh | \
  SOLARIS_VERSION="v0.0.7" bash

curl -fsSLo server.toml \
  https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.7/example.toml
solaris --check --config server.toml
solaris --config server.toml
```

The installer verifies the published SHA-256 before replacing the binary. It
installs to `$HOME/.local/bin` for a regular user and `/usr/local/bin` for root;
set `SOLARIS_INSTALL_DIR` to override that destination. Windows and macOS do
not currently have prebuilt archives.

**Upgrading from an earlier Solaris release:** back up the existing world and
configuration first. Solaris validates its persisted world contract and rejects
an incompatible world rather than silently mixing generation. Use a fresh
`[data].world_dir` when that contract changes. Do not delete or hand-edit the
old world's contract to bypass the check. Changing an effective component
startup contribution also requires a fresh world directory.

## Build and run the v0.0.8 development tree

Use the repository's debug profile for development:

```sh
cargo build --bin mc-server
cargo run --bin mc-server -- --check --config example.toml
cargo run --bin mc-server -- --config example.toml
```

`--config` selects the TOML file; without it the binary looks for
`config.toml`. `--check` parses and validates the deployment, prints the
effective configuration as JSON, and exits without binding a listener. Review
`operator_warnings`, `effective_autoscale`, and `discovered_plugins` before
serving.

For the exact locked release workspace build that release gates use, run it
through the harness (never hand-roll the flags):

```sh
python3 -m tools.harness run build --release
```

That is `cargo build --locked --release --workspace`. Full harness
interaction — profiles, preparation versus execution, credentials, artifact
lookup, and current limits — is documented in
[`docs/AGENT_TOOLING.md`](docs/AGENT_TOOLING.md#validation-harness).

### Interactive server console

When attached to a terminal, the server opens a status dashboard with a separate
command line and command replies, without streaming runtime logs into the UI.
Use `help` to list commands; examples include `status`, `profile`, `list`, `save-all`,
`time set night`, `weather rain`, `gamerule doDaylightCycle false`, and
`operator list`. Use `stop` for the normal save-and-drain shutdown.

Enter submits a command; Up/Down recall command history; Backspace deletes the
last character; Ctrl+U clears the line. Ctrl+C/Ctrl+D also request shutdown.

Pass `--no-console` for plain stdin commands without the dashboard. The foreground
server owns stdin; do not run a second reader alongside it.
Runtime events go to `logs/latest.log` (INFO/WARN/ERROR) and `logs/debug.log`
(DEBUG and above; `RUST_LOG` controls this file). Both files start fresh on launch.
The `profile` console command writes `logs/profile.json`: measured tick-stage
latencies, RSS/cgroup memory, chunks, populations and network counters. This is a
runtime metrics profile, not a CPU stack-sampling or heap-allocation trace.
`--check` and the `operator` subcommands leave existing server logs untouched.
If chunks blink, send `blink` in ordinary chat and save `logs/debug.log` before
restarting. The marker and chunk radius/send/unload events can be correlated as
described in [the operating guide](docs/OPERATING.md).

## Network address and port

The listen address and Minecraft port are ordinary, existing configuration
settings:

```toml
[network]
# Loopback: only this machine can connect.
bind_address = "127.0.0.1"
# Change this to run Solaris on another TCP port.
port = 25565
```

Clients connect to `host:port` (for example, `192.168.1.20:25570`). To accept
remote connections, choose an appropriate interface address, configure the
host firewall/NAT for the same TCP port, and preferably enable online authentication.
The starter configuration listens on `0.0.0.0`. Both authentication modes permit
public listening; offline-mode player and operator names can be impersonated.
Detailed examples are in
[`docs/OPERATING.md`](docs/OPERATING.md#network-bind-and-port).

## Configuration essentials

- **Authentication:** `[auth].online_mode = true` uses Mojang session
  authentication. Offline mode permits public hosting but does not prove a
  player's identity; restrict access to trusted players or an authenticated proxy.
- **World:** `[data].world_dir` is required. A missing directory is created as a
  fresh Solaris world. Solaris persists a world contract covering generation
  revision, seed, mode, geometry, and plugin worldgen profiles; incompatible
  changes require a new world directory. Back up alpha worlds before updating.
- **Operators:** configure identities with `[admin].operators` or
  `operators_file`. Use `mc-server --config server.toml operator
  {add|remove|list} <name-or-uuid>` to manage the persisted file without
  enabling local-dev operators or editing JSON. Keep
  `allow_local_dev_operators = false` outside throwaway loopback development.
- **Autoscale:** `[autoscale]` is enabled by default with the `balanced` profile.
  It changes bounded view distance and chunk work budgets one unit at a time,
  only after more than 60 continuous seconds of overload or stable recovery.
  Each step starts a fresh window; `--check` prints normalized limits and the
  `scale_down_after_seconds` / `scale_up_after_seconds` policy.
- **Plugins:** external Wasmtime component packages are discovered below
  `[plugins].directory` (normally `plugins/`). Each package supplies
  `plugin.toml`, `plugin.wasm`, optional `config.toml`, and declared resources.
  Use strict deployment, an exact `expected` list, and explicit capability
  grants for a controlled server.
- **Vanilla data:** required baseline data is embedded. Setting
  `[data].vanilla_data_dir` opts into an authoritative extracted sidecar, which
  must be complete and exactly match 26.1.2.

See [`example.toml`](example.toml) for the complete commented starter profile
and [`docs/OPERATING.md`](docs/OPERATING.md) for network, authentication,
world, operator, autoscale, and check-output guidance.

## Plugins and Solaris Loader

Solaris runs Wasmtime components implementing `solaris:plugin@0.7.0`. The
out-of-tree `sdk/rust/` workspace contains the generic Rust SDK and examples;
product guest source belongs to its own repository. Production core loads
encoded `plugin.wasm` artifacts and neither compiles product source nor requires
a sibling checkout.

Packages without a `[client]` bundle are `server_only` and accept an ordinary
vanilla 26.1.2 client. A package with client bundles is `server_and_client`;
connecting players must install the matching Solaris Loader adapter and approve
the requested permissions.

- Operator and author guide: [`docs/PLUGINS.md`](docs/PLUGINS.md)
- First-party product source: sibling `../solaris-default-plugins/sources/`
- Fabric/NeoForge/Forge client installation: [`docs/SOLARIS_LOADER.md`](docs/SOLARIS_LOADER.md)

Solaris Loader is not needed for a server whose selected packages are all
`server_only`. `solaris --check --config server.toml` validates component
packages and prints the checked ids. Core declares Loader wire **3** with
artifact index schema **2**; a Loader build that speaks an older wire is
refused rather than silently served reduced content.

## Current alpha boundaries

Ordinary survival, persistence, multiplayer, entities, trading, plugins, and
core vanilla village generation are implemented far enough for active field
testing, not full vanilla parity. Rare redstone/vehicle behavior,
species-specific behavior, and broad production performance envelopes remain
incomplete.

The owner defers real-client acceptance of the settlements work (R0/R1) by
explicit decision, so no client-verified claim is made for it: what exists is
the code-level half plus the Rust gates. The load-sensitive red in
`mc-test-harness --test settlement_pause_repro` is recorded as a known gate
failure, not silently retried away; `docs/MEMORY.md` carries its receipts.

One fixed seed proves nothing about a world: owner field testing comes first,
followed by varied-seed graphical exploration with the seeds recorded for
reproduction. Solaris has no released compatibility surface, so superseded
Solaris APIs and schemas are deleted rather than carried.

Do not assume an existing Solaris world will remain compatible with a newer
alpha. Solaris can read supported vanilla Anvil data, but unversioned imports
have stricter generation constraints and are not a promise of complete vanilla
server replacement.

## Test

Run the gates through the harness from the repository root so every run leaves
a receipt under `.analysis/validation/`:

```sh
python3 -m tools.harness list
python3 -m tools.harness run correctness
python3 -m tools.harness run inventory
python3 -m tools.harness run playable --check   # preparation only, never a pass
```

`correctness` runs the four Rust L2 gates (formatter, strict workspace Clippy,
code-health, workspace/all-target tests). Debug builds are the normal
development loop. Release and real-client gates run only for their documented
release/checkpoint scopes; see
[`docs/AGENT_TOOLING.md`](docs/AGENT_TOOLING.md#validation-harness) before
launching a graphical or twenty-minute profile.

## Documentation map

`docs/` root holds what describes current behaviour and current process.
Milestone logs, dated evidence, phase reviews, and the session archive live in
`docs/milestones/`, `docs/evidence/`, `docs/performance/`, `docs/memory/`,
`docs/playable/`, and `docs/releases/`; open those only when a task asks for
that era.

- [`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) — design and compatibility scope
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — target runtime design
- [`docs/OPERATING.md`](docs/OPERATING.md) — network, auth, world, operator, and check output
- [`docs/PLUGINS.md`](docs/PLUGINS.md) — component package and API `0.7.0` guide
- [`docs/SOLARIS_LOADER.md`](docs/SOLARIS_LOADER.md) — client Loader installation
- [`docs/VILLAGE_GENERATION.md`](docs/VILLAGE_GENERATION.md) — vanilla village generation and its declared divergence
- [`docs/MEMORY.md`](docs/MEMORY.md) — current work and evidence cursor
- [`docs/DEFINITION_OF_DONE.md`](docs/DEFINITION_OF_DONE.md) — readiness labels and the evidence matrix
- [`docs/AGENT_TOOLING.md`](docs/AGENT_TOOLING.md) — harness wiring, profiles, and receipts
- [`docs/REPLACEMENT_READINESS.md`](docs/REPLACEMENT_READINESS.md) and [`docs/VALIDATION_LEDGER.md`](docs/VALIDATION_LEDGER.md) — the readiness claim and the recorded evidence behind it
- [Current preliminary release](https://github.com/kaiserproger/solaris/releases/tag/v0.0.7) — binaries and checksums; [changelog](docs/releases/v0.0.7.md)

## Repository layout

```text
crates/                              Rust server workspace
sdk/rust/                            Out-of-tree Rust guest SDK and packages
../solaris-loader/                   Independent Loader/real-client Gradle repo
examples/loader-live-gate/           Loader-required integration fixture
docs/                                contracts, guides, ADRs, and evidence
tools/                               extraction and validation tools
```

Production core builds and deployments need no sibling repository. Java/client
validation needs `../solaris-loader` (override with `SOLARIS_LOADER_ROOT`):

```sh
git clone https://github.com/kaiserproger/solaris-loader.git ../solaris-loader
```

The Loader owns its sources, development instructions, and Git history. Server
tags and commit IDs do not identify Loader revisions; use compatible revisions
together.

## License

Dual-licensed under either Apache-2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)) or MIT
([`LICENSE-MIT`](LICENSE-MIT)), at your option.
