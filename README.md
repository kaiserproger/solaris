# Solaris

Solaris is an authoritative Minecraft Java Edition server written in Rust. It
targets the vanilla **Minecraft Java Edition 26.1.2** protocol and also supports
optional client content through Solaris Loader.

> **Development status:** `main` is the in-progress `v0.0.3-alpha.1` line. The
> latest published release is **`v0.0.2-alpha.1`**. Solaris is suitable for
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
curl -fsSL https://raw.githubusercontent.com/kaiserproger/solaris/main/install.sh | \
  SOLARIS_VERSION="v0.0.2-alpha.1" bash

curl -fsSLo server.toml \
  https://raw.githubusercontent.com/kaiserproger/solaris/v0.0.2-alpha.1/example.toml
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
  `[plugins].directory` (normally `plugins/`). Bundled examples are disabled
  unless named in `[plugins].bundled`. Use strict deployment and an exact
  `expected` list for a controlled server.
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
- Inspectable examples: [`examples/plugins/`](examples/plugins/)

Solaris Loader is not needed for a server whose selected plugins are all
`server_only`. `solaris --check --config server.toml` reports each discovered
plugin's deployment, supported loaders, requested permissions, bundle identity,
and artifact size.

## Current alpha boundaries

Ordinary survival, persistence, multiplayer, entities, trading, and plugins are
implemented far enough for active field testing, not full vanilla parity. Rare
redstone/vehicle behavior, species-specific behavior, broad production
performance envelopes, and parts of village behavior remain incomplete. The
latest field test also identified movement/collision and item-pickup issues
tracked as alpha-3 blockers.

Do not assume an existing Solaris world will remain compatible with a newer
alpha. Solaris can read supported vanilla Anvil data, but unversioned imports
have stricter generation constraints and are not a promise of complete vanilla
server replacement.

## Test

The full repository gates are:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Debug builds are the normal development loop. Release and real-client gates are
run only for their documented release/checkpoint scopes.

## More documentation

- [`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md) — design and compatibility scope
- [`docs/releases/v0.0.2-alpha.1.md`](docs/releases/v0.0.2-alpha.1.md) — latest released alpha
- [`docs/PUBLIC_ALPHA3_PLAN.md`](docs/PUBLIC_ALPHA3_PLAN.md) — current development work
- [`docs/REPLACEMENT_READINESS.md`](docs/REPLACEMENT_READINESS.md) — replacement-readiness limits
- [`docs/VALIDATION_LEDGER.md`](docs/VALIDATION_LEDGER.md) — recorded evidence

## Repository layout

```text
crates/                              Rust workspace
client-mod/solaris-client-agent/     Loader adapters and real-client tooling
examples/plugins/                    Luau plugin examples
examples/loader-live-gate/           Loader-required integration fixture
docs/                                contracts, guides, ADRs, and evidence
tools/                               extraction and validation tools
```

## License

Dual-licensed under either Apache-2.0 ([`LICENSE-APACHE`](LICENSE-APACHE)) or MIT
([`LICENSE-MIT`](LICENSE-MIT)), at your option.
