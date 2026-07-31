# Plugin deployment reporting

Date: 2026-07-31

Checkpoint base: `26720e9` (`docs: define CodexPro goal launch contract`)

## Result

Plugin discovery derives every deployment field from validated client-bundle declarations. There is no second manifest flag or stored deployment authority:

- no client bundles: `server_only`;
- one or more client bundles: `server_and_client`.

`PreparedLuaPlugins::discovered_plugins` now exposes the plugin id, derived deployment class, sorted supported-loader set, sorted permission set, total artifact bytes, and every validated client bundle's identity and bounded manifest facts: id, version, relative artifact path, SHA-256, size, loaders, content kinds, and permissions.

Server startup logs the same operator-facing summary. `solaris --check` serializes it in the `discovered_plugins` JSON array, so operators can determine Loader requirements and artifact scope before starting the server. The six shipped bundled examples are explicitly labelled **Server-only** in their READMEs and `docs/PLUGINS.md`; the separate `examples/loader-live-gate` fixture is labelled **Requires Solaris Loader on client**.

A client-required server now sends the exact 26.1.2 Configuration-state disconnect packet when the Solaris Loader handshake fails. The bounded NBT text names the supported Loader platforms and required plugin/bundle/version identities before the connection closes. The normal server-only path remains unchanged and sends no Loader payload.

This also corrects stale continuity documentation: the earlier colony extraction is already complete in `4902a3d`, periodic natural spawning is implemented, and deployment reporting is no longer an unfinished implementation item. The graphical Loader compatibility matrix remains a separate external acceptance gate.

## Validation

- `cargo test -p mc-protocol`: 303 passed.
- `cargo test -p mc-script --features lua-runtime`: 166 passed.
- `cargo test -p mc-net`: 1,859 passed.
- `cargo test -p mc-server`: 161 passed, 6 documented ignored local-sidecar gates.
- Raw-TCP missing-ack regression decodes packet id `0x02` and asserts the exact Configuration-disconnect NBT text.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `git diff --check`: passed.
- Independent read-only bounded review: `No findings`, verdict `pass`.

Two full `cargo test --workspace` attempts were externally terminated while tests were still green. The complete affected-package suites above passed; the workspace-wide gate must not be claimed complete from those attempts.

## Remaining external acceptance

- ordinary vanilla 26.1.2 client acceptance for a server-only example;
- Fabric, NeoForge, and Forge permission/artifact acknowledgement for the client-required fixture;
- exact denial/disconnect presentation, asset/screen activation, logout cleanup, and reconnect behavior on a graphical client.

Benchmark: not applicable. Discovery reporting is startup-only, and the disconnect path runs only on a failed pre-Play handshake.
