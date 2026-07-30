# Plugin deployment reporting

Date: 2026-07-30

Checkpoint base: `bcfcf6ab19919d838fa7799cc122321e15e55b0b`

## Result

Plugin discovery now derives each plugin's deployment class directly from its
validated client-bundle declarations:

- no client bundles: `server_only`;
- one or more client bundles: `server_and_client`.

No second manifest flag or stored deployment authority was added.
`PreparedLuaPlugins::discovered_plugins` exposes the plugin id and derived
class. Server startup emits both fields for every discovered plugin, and
`solaris --check` includes them in its `discovered_plugins` JSON array.

This closes the first two bounded changes in the PUBLIC_ALPHA plugin deployment
section. Loader identities, permissions, total artifact bytes, documentation
labels, and the Loader-handshake disconnect remain separate follow-up work.

## Validation

- `cargo test -p mc-script --lib`: 85 passed.
- `cargo test -p mc-server --bin mc-server --test cli`: 92 passed,
  0 failed, 2 documented ignored sidecar gates.
- `cargo clippy -p mc-script -p mc-server --all-targets -- -D warnings`:
  passed.
- `cargo fmt -p mc-script -p mc-server -- --check`: passed.
- `cargo run -p xtask -- code-health`: 0 failures, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Independent read-only review: passed with no findings.

The focused tests prove both derived classes and exact `--check` JSON for two
simultaneously discovered plugins. No real-client gate was run: server-only
vanilla acceptance and client-required Loader acknowledgement belong to the
remaining acceptance slice.

Benchmark: not applicable. This change reports existing validated manifest
state and adds no runtime loop or performance contract.
