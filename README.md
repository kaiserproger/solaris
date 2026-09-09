# Solaris

Solaris is an authoritative Minecraft Java Edition server written in Rust. It
targets the vanilla **Minecraft Java Edition 26.1.2** protocol and also supports
optional client content through Solaris Loader.

> **Development status:** this is the **`v0.0.3-alpha.1`** prerelease line.
> Maturity remains **draft**, not release-ready. Solaris is suitable for
> testing, plugin development, and bounded multiplayer field tests; it is not a
> production-safe replacement for vanilla, Paper, Fabric, Forge, or NeoForge.
> Alpha worlds, plugin APIs, and Loader contracts may change without migration.

Solaris ships an optional default-off read-only web dashboard
(`[dashboard]` in the config, see [`docs/OPERATING.md`](docs/OPERATING.md))
and the `operator add|remove|list` CLI. The optional standard plugin pack is
still planned. See [`docs/PUBLIC_ALPHA3_PLAN.md`](docs/PUBLIC_ALPHA3_PLAN.md).

## Install the released alpha (Linux)

Published archives are available for Linux x86_64 and AArch64. Pin the release
because GitHub's `latest` alias does not resolve prereleases:

```sh
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.3-alpha.1/install.sh | \
  SOLARIS_VERSION="v0.0.3-alpha.1" bash

curl -fsSLo server.toml \
  https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.3-alpha.1/example.toml
solaris --check --config server.toml
solaris --config server.toml
```

The installer verifies the published SHA-256 before replacing the binary. It
installs to `$HOME/.local/bin` for a regular user and `/usr/local/bin` for root;
set `SOLARIS_INSTALL_DIR` to override that destination. Windows and macOS do
not currently have prebuilt archives.

## Build and run the alpha-3 development tree

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
host firewall/NAT for the same TCP port, and enable online authentication.
Solaris rejects a public bind in offline mode; do not expose an offline-mode
server to an untrusted network. Detailed examples are in
[`docs/OPERATING.md`](docs/OPERATING.md#network-bind-and-port).

## Configuration essentials

- **Authentication:** `[auth].online_mode = true` uses Mojang session
  authentication. Offline mode is for trusted loopback/private testing only and
  does not prove a player's identity.
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
  It adapts bounded view distance and chunk work budgets to runtime pressure;
  `--check` prints the normalized limits and policy.
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

The local field-test archive was built and its isolated startup was verified:
`.analysis/releases/v0.0.3-alpha.1/solaris-x86_64-unknown-linux-gnu.tar.gz`.
SHA-256: `925a825b709e5e44d8ad17957d741e3751ed955dba3e510ee4baa8ee78ed6b36`.
It is frozen before the repository split, not published and not a full-survival
acceptance. Extract it, run `./solaris --config example.toml`, and connect a
Minecraft Java 26.1.2 client to `127.0.0.1:25565`. Choose `[data].seed` before
first startup; use a new world directory to test another seed.

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
- [Current alpha release](https://github.com/kaiserproger/solaris/releases/tag/v0.0.3-alpha.1) — binaries, checksums and release notes
- [`docs/PUBLIC_ALPHA3_PLAN.md`](docs/PUBLIC_ALPHA3_PLAN.md) — current development work
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
