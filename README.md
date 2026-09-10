# Solaris

Solaris is an authoritative Minecraft Java Edition server written in Rust. It
targets the vanilla **Minecraft Java Edition 26.1.2** protocol and also supports
optional client content through Solaris Loader.

> **Development status:** **`v0.0.5`** is a preliminary release.
> Maturity remains **draft**, not release-ready. Solaris is suitable for
> testing, plugin development, and bounded multiplayer field tests; it is not a
> production-safe replacement for vanilla, Paper, Fabric, Forge, or NeoForge.
> Alpha worlds, plugin APIs, and Loader contracts may change without migration.

Solaris ships an optional default-off read-only web dashboard
(`[dashboard]` in the config, see [`docs/OPERATING.md`](docs/OPERATING.md))
and the `operator add|remove|list` CLI. The standard plugin pack is explicitly
opt-in; see [plugin installation](docs/PLUGINS.md#standard-plugin-pack).

## Install v0.0.5 (Linux)

Published archives are available for Linux x86_64 and AArch64. Pin the release
because GitHub's `latest` alias does not resolve prereleases:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.5/install.sh | \
  SOLARIS_VERSION="v0.0.5" bash

curl -fsSLo server.toml \
  https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.5/example.toml
solaris --check --config server.toml
solaris --config server.toml
```

The installer verifies the published SHA-256 before replacing the binary. It
installs to `$HOME/.local/bin` for a regular user and `/usr/local/bin` for root;
set `SOLARIS_INSTALL_DIR` to override that destination. Windows and macOS do
not currently have prebuilt archives.

**Upgrading from alpha-3:** back up the existing world and configuration first.
Alpha-4 uses Solaris world-contract schema 4 and worldgen revision 19; an older
Solaris world contract is rejected rather than silently mixing generation.
Use a fresh `[data].world_dir`. Do not delete or hand-edit the old world's
contract to bypass the check. Changing an effective startup `rules.lua` plan
also requires a fresh world directory.

## Build and run the v0.0.5 development tree

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
- **Plugins:** external Luau packages are discovered below
  `[plugins].directory` (normally `plugins/`). Deploy selected packages from
  the sibling `solaris-default-plugins` repository. Use strict deployment and
  an exact `expected` list for a controlled server.
- **Vanilla data:** required baseline data is embedded. Setting
  `[data].vanilla_data_dir` opts into an authoritative extracted sidecar, which
  must be complete and exactly match 26.1.2.

See [`example.toml`](example.toml) for the complete commented starter profile
and [`docs/OPERATING.md`](docs/OPERATING.md) for network, authentication,
world, operator, autoscale, and check-output guidance.

## Plugins and Solaris Loader

Solaris runs sandboxed, strict Luau plugins under current API `0.6.0`. Plugins
without a `[client]` bundle are `server_only` and accept an ordinary vanilla
26.1.2 client. A plugin with client bundles is `server_and_client`; connecting
players must install the matching Solaris Loader adapter and approve the
requested permissions.

- Operator and author guide: [`docs/PLUGINS.md`](docs/PLUGINS.md)
- Fabric/NeoForge/Forge client installation: [`docs/SOLARIS_LOADER.md`](docs/SOLARIS_LOADER.md)
- Inspectable packages: [`solaris-default-plugins`](https://github.com/kaiserproger/solaris-default-plugins)

Solaris Loader is not needed for a server whose selected plugins are all
`server_only`. `solaris --check --config server.toml` reports each discovered
plugin's deployment, supported loaders, requested permissions, bundle identity,
and artifact size.

## Current alpha boundaries

Ordinary survival, persistence, multiplayer, entities, trading, and plugins are
implemented far enough for active field testing, not full vanilla parity. Rare
redstone/vehicle behavior, species-specific behavior, broad production
performance envelopes, and parts of village behavior remain incomplete.

Known honest limits as of 2026-09-06: the full twenty-minute playable survival
loop is **blocked** after natural spruce pickup/crafting because the scenario
found no dry crafting-table placement target. This is not proof that survival
works on other terrain. Stop tuning one fixed seed: owner field testing comes
first, followed by varied-seed graphical exploration with seeds recorded for
reproduction. The frozen load matrix remains 20 PASS / 22 FAIL with no owner
terrain `ACCEPT`; the full core redesign is incomplete.

Alpha-4's reported-seed graphical survey confirmed natural cod/squid, separated
herds, nearby trees, forest and mountain terrain, and natural clay in two
water sites. It used operator travel and does not close the blocked no-debug
survival scenario above. Startup Luau gameplay rules and the interactive server
console are documented in the [plugin guide](docs/PLUGINS.md) and console
section above. [Solaris Loader v0.1.0](https://github.com/kaiserproger/solaris-loader/releases/tag/v0.1.0)
provides matching protocol-2 player adapters.

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

## More documentation

- [`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) — design and compatibility scope
- [Current preliminary release](https://github.com/kaiserproger/solaris/releases/tag/v0.0.5) — binaries and checksums; [changelog](docs/releases/v0.0.5.md)
- [`docs/MEMORY.md`](docs/MEMORY.md) — current work and evidence cursor
- [`docs/REPLACEMENT_READINESS.md`](docs/REPLACEMENT_READINESS.md) — replacement-readiness limits
- [`docs/VALIDATION_LEDGER.md`](docs/VALIDATION_LEDGER.md) — recorded evidence

## Repository layout

```text
crates/                              Rust workspace
../solaris-loader/                   Independent Loader/real-client Gradle repo
../solaris-default-plugins/          Independent Luau package repo
examples/loader-live-gate/           Loader-required integration fixture
docs/                                contracts, guides, ADRs, and evidence
tools/                               extraction and validation tools
```

Core production builds need neither sibling repository. Java/client validation
needs `../solaris-loader` (override with `SOLARIS_LOADER_ROOT`); integration tests
of first-party plugin behavior need `../solaris-default-plugins`. Clone the
companion repositories next to this checkout:

```sh
git clone https://github.com/kaiserproger/solaris-loader.git ../solaris-loader
git clone https://github.com/kaiserproger/solaris-default-plugins.git ../solaris-default-plugins
```

Each repository owns its sources, development instructions and Git history.
Hosted CI checks out this same sibling layout. Server tags and commit IDs do
not identify Loader or plugin revisions; use compatible revisions together.

## License

Dual-licensed under either Apache-2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)) or MIT
([`LICENSE-MIT`](LICENSE-MIT)), at your option.
