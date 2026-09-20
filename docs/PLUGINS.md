# Plugins

Solaris production plugins are Wasmtime components implementing the versioned
WIT contract `solaris:plugin@0.7.0`. The host crate is
`crates/mc-plugin-host`; the WIT source is `crates/mc-script/wit`; Rust guest
source and the SDK live in the separate `sdk/rust/` workspace. The server
loads a compiled component artifact. It neither compiles guest source nor
requires a sibling checkout to start a production deployment.

A plugin is an addon, not a server mod. It receives typed, bounded values over
the WIT boundary; it never receives world, entity, region, session, socket,
lock, task, or scheduler handles. Native owners retain authority for every
world mutation, storage operation, and player session.

## Operator deployment

The configured plugin root contains one directory per package:

```text
plugins/
`-- my-plugin/
    |-- plugin.toml
    |-- plugin.wasm
    `-- config.toml       # optional
```

`plugin.toml` declares the package identity, API version, subscriptions,
command roots, requested capabilities, and optional client, world-generation,
or precommit declarations. `plugin.wasm` is a WebAssembly **component** for
`solaris:plugin@0.7.0`, not an unencoded core Wasm module. `config.toml` is
optional opaque package configuration: the host passes its text to the guest,
which defines and validates its own settings.

Configure an exact production deployment with strict discovery, the full
package roster, and a grant for every capability each package requests:

```toml
[plugins]
directory = "plugins"
strict = true
expected = ["my-plugin"]

[plugins.grants.my-plugin]
capabilities = ["storage"]
```

The grants are an operator authorization decision, not a suggestion to the
guest. In strict mode a missing, extra, malformed, duplicate, or ungranted
capability request refuses the package; it is never started with reduced
rights. Strict discovery also refuses stray entries, malformed packages,
duplicate ids or command roots, and a deployed set that differs from
`expected`. Use permissive discovery only for local iteration where skipping an
ordinary broken package is intentional.

Product packages are owned and built in their own repositories. A deployment
copies each selected package's `plugin.toml`, `plugin.wasm`, and optional
`config.toml` into its plugin root; the core and SDK do not carry product
sources or manifests.

Validate the exact deployment before serving:

```sh
solaris --check --config server.toml
solaris --config server.toml
```

`--check` performs discovery, grant validation, component compilation, and the
same startup phases the server would use. It does not create a world, write
plugin storage, bind a listener, apply commands, or start timers. A successful
component check prints the checked package ids:

```text
component plugins checked: my-plugin
```

Treat any check failure as a refused deployment, not a partial success. Inspect
the package path and reported manifest, artifact, capability, startup, or
roster error; do not remove strictness to make a production deployment start.

## Package manifest

A minimal server-only component package can declare no subscriptions,
capabilities, or player commands:

```toml
id = "hello"
name = "Hello"
version = "0.1.0"
api = "0.7.0"
```

The optional `entry` names a component file inside the package and defaults to
`plugin.wasm`. `events`, `capabilities`, `required_features`,
`player_commands`, `hooks`, `[client]`, and `[worldgen]` are validated package
declarations. Unknown fields and invalid or duplicate values refuse the
package before guest code is compiled.

`required_features` describes authored data core must load. It does not grant
access. Feature capabilities declared by a package must also appear in
`required_features`; capabilities themselves must be both declared by the
package and explicitly granted by the operator.

Packages without `[client]` bundles are `server_only` and support ordinary
vanilla clients. Any package that declares client bundles makes the relevant
deployment `server_and_client`: connecting players need a compatible Solaris
Loader and must approve the bundle's requested client permissions. Bundle paths,
hashes, sizes, content kinds, supported loaders, and permissions are validated
before a player is served content. There is no server-only substitute for a
package that requires client content. See [`SOLARIS_LOADER.md`](SOLARIS_LOADER.md)
for player installation.

## Build, encode, and deploy a Rust guest

The SDK is an API dependency, not a home for product plugins. A guest's source,
manifest, configuration, and deployed artifact belong to that plugin's own
repository. Build its `wasm32-unknown-unknown` module, then encode that module
as a component before placing it in its package:

```sh
rustup target add wasm32-unknown-unknown
cargo build --manifest-path path/to/plugin/Cargo.toml \
  --target wasm32-unknown-unknown --release
cargo run --manifest-path crates/mc-plugin-host/Cargo.toml \
  --example plugin-component -- \
  path/to/plugin.wasm plugins/my-plugin/plugin.wasm
```

`plugin-component` validates while encoding; a module that lacks the component
types cannot become a deployable artifact. The plugin repository owns
`plugin.toml` and optional `config.toml`; the core repository neither ships nor
compiles first-party product plugins.

Authors implement `solaris_plugin_sdk::Plugin` (or bindings for the same WIT
world) and use the WIT and SDK definitions as the API reference. Do not copy
internal host types into a guest or rely on a host implementation detail.

## Lifecycle and configuration

A component has two startup phases in distinct stores:

1. `configure` receives the package configuration in a short-lived store with
   no runtime capability. It may return a normalized startup contribution that
   is validated and fingerprinted into the world contract.
2. `init` receives the same configuration and initialization context in the
   component's runtime store. It creates the instance's runtime-local state and
   may return its opening command batch.

State retained during `configure` cannot reach `init`; initialize runtime state
from the configuration again. The host performs discovery, manifest validation,
startup contribution validation, and deployment-surface validation before it
opens a world or binds the listener. A conflicting startup contribution,
world-generation declaration, client surface, or command ownership refuses
startup.

`shutdown` is best effort. The host reclaims its own resources whether or not
it runs, and a trap can skip it, so no durable correctness decision may depend
on cleanup. Plugin storage remains keyed by the plugin identity; a component
reload begins with fresh runtime-local state rather than migrating a guest
instance.

On Unix, `SIGHUP` may reload an active strict deployment. The candidate is
rediscovered, compiled, configured, and initialized before it replaces the
active generation. Its ordered package catalog, grants, command and payload
ownership, precommit registrations, client surface, and startup contribution
must be unchanged; otherwise restart is required. World-generation or startup
plan changes additionally require a fresh world directory. A rejected candidate
leaves the active generation in place. There is no filesystem watcher or
polling reload loop.

## Events, commands, and capability boundaries

A manifest subscribes only to the named events it needs and claims only its own
player-command roots. The host delivers events to one component in order. Each
callback receives immutable, bounded records and returns a bounded command
batch; host admission validates every command before native work is scheduled.
An oversized, malformed, unauthorized, trapped, budget-exceeding, or rejected
callback does not expose a partial command batch. A command's eventual result
is delivered as a typed event when the owning native service completes it.

Capabilities gate privileged operations such as storage, player queries,
teleportation, zones, inventory, or world changes. They are not client
permissions and they do not make a request authoritative. Every native owner
still checks the live player session, ownership, revisions, input bounds,
capacity, and its domain invariants before committing work.

Use stable request and operation identifiers where the WIT operation requires
them. A successful submission is not confirmation of client rendering or a
future mutation; inspect the typed completion or refusal event. For durable
operations, reuse an operation id only for the same intent and payload.

Precommit hooks are a separate bounded decision path. A package can declare
`before-build` and/or `before-damage`; an operator must separately register a
declared hook in `[[plugins.hooks]]`. Hooks receive immutable contexts and
answer a decision rather than queuing commands. They cannot perform I/O,
submit asynchronous work, or mutate the world. The host applies the configured
failure policy when a hook cannot answer in time or is retired.

## Diagnosing a deployment

Use `--check` after changing any package artifact, manifest, capability grant,
client bundle, hook registration, or expected roster. It catches malformed
components and manifests, unknown API/capability/event names, duplicate ids or
command roots, missing client artifacts, grant failures, invalid startup
answers, and expected-set drift before serve would touch a world or listener.

At runtime, log and act on package traps, budget failures, admission refusals,
and reload reports. A failed component is not a successful degraded deployment:
its owned command roots are retired and the package must be repaired or
intentionally removed through a validated deployment change.
