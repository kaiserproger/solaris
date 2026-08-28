# M89 file-backed access-control checkpoint — 2026-08-27

## Scope

This is one bounded M89 checkpoint, not completion of the whole auth/chat/anti-corruption milestone.

The checkpoint adds restart-stable, externally managed identity files for:

- operators;
- whitelist members;
- banned players.

It preserves the existing inline TOML policy and existing login/permission authority. It does not add a second auth stack, an in-game access-list editor, full permission levels, chat moderation, or a full anti-cheat claim.

## Current auth baseline

The historical M89 text had become stale: the current tree already implements online-mode encryption and Mojang session verification.

Current production behavior includes:

- RSA `EncryptionRequest` / encrypted response validation;
- encrypted login streams;
- Minecraft server-hash calculation;
- `MojangSessionVerifier` / `hasJoined` verification;
- optional client-IP verification through `prevent_proxy_connections`;
- public-bind rejection for offline mode;
- public-bind rejection when loopback-only automatic OP is enabled;
- whitelist/ban checks against the resulting profile name/UUID;
- duplicate profile/name fencing at Play registration.

Focused current-tree validation:

```text
cargo test -p mc-net --lib session_auth --quiet
14 passed; 0 failed

cargo test -p mc-net --lib login::tests --quiet
9 passed; 0 failed
```

`docs/milestones/M89.md` is updated to reflect that current baseline instead of claiming online mode is unimplemented.

## Public configuration

`[admin]` now accepts:

```toml
operators = ["InlineOp"]
operators_file = "ops.json"
allow_local_dev_operators = false
```

`[auth]` now accepts:

```toml
whitelist_enabled = true
whitelist = ["InlineAllowed"]
whitelist_file = "whitelist.json"
banned_players = ["InlineBan"]
banned_players_file = "banned-players.json"
```

`example.toml` documents the same fields.

Relative paths resolve from the directory containing the server TOML, not from the process working directory. Absolute paths remain explicit absolute paths.

The file identities are merged with inline TOML entries on each fresh `mc-server` startup. `mc-server --check` executes the same load before rendering the effective configuration.

## File format and bounds

Each configured file is a JSON array of vanilla-style profile objects. An object must contain a `name`, a `uuid`, or both. Extra fields are tolerated so ordinary vanilla-style records can be consumed without granting those extra fields any Solaris authority.

Examples:

```json
[
  {
    "name": "FileOp",
    "uuid": "11111111-1111-1111-1111-111111111111",
    "level": 4,
    "bypassesPlayerLimit": false
  }
]
```

and:

```json
[
  {
    "name": "Banned",
    "uuid": "33333333-3333-3333-3333-333333333333",
    "reason": "example"
  }
]
```

The identity reader fails closed with these limits:

- configured path must exist and resolve to a regular file;
- maximum file size: 1 MiB;
- Solaris opens one file handle, validates metadata from that handle, then reads through `take(1 MiB + 1)` so replacement/growth cannot cause an unbounded allocation before the size fence;
- maximum profile objects per file: 4096;
- names use the same `3..=16` ASCII alphanumeric/underscore syntax as Solaris login;
- UUIDs must parse and are normalized to canonical UUID strings;
- an object with neither identity is rejected;
- malformed JSON is rejected.

Names and UUID tokens are deduplicated in the reader and then pass through the existing normalized `CommandPermissionConfig` / `LoginAccessConfig` sets after merge.

The existing login authority retains ban-over-whitelist precedence. The JSON `level` field is deliberately ignored in this checkpoint because the current Solaris permission model is binary operator/member; this checkpoint must not imply vanilla permission-level parity.

## Production entry points

Both production entry paths load the files before policy translation:

- `mc-server --check`;
- normal `mc-server` serve startup.

Normal startup logs only bounded counts (`files`, operator identities, whitelist identities, banned identities). Semantic validation errors include field, entry index and resolved file path but deliberately omit the raw invalid name/UUID value, so a failed startup does not echo access-file identities.

An explicitly configured missing or malformed file prevents startup/check instead of silently falling back to inline policy.

## Executable evidence

### Bounded parser / merge / restart load

```text
cargo test -p mc-server --lib file_backed_access_control -- --nocapture
running 3 tests
...
test result: ok. 3 passed; 0 failed
```

The tests cover:

- config-relative path resolution;
- inline + file identity merge;
- vanilla-style extra JSON fields;
- name and UUID normalization;
- fresh-config reload after the same file is rewritten;
- missing file;
- non-array/malformed JSON;
- profile with no identity;
- invalid name;
- invalid UUID;
- 4097-entry rejection;
- >1 MiB rejection.

### Whitelist/ban policy across fresh server instances

```text
cargo test -p mc-server --test login \
  file_backed_whitelist_and_ban_reload_across_server_instances \
  -- --exact --nocapture
PASS 1/1
```

The first server instance loads `Allowed` from `whitelist.json`:

- `Allowed` reaches `SetCompression`;
- `Blocked` receives `LoginDisconnect` with `not whitelisted` before later login states.

The files are then rewritten without changing TOML. A fresh second server instance loads:

- whitelist: `Allowed`, `SecondUser`;
- ban: `Allowed`.

The second server proves:

- `Allowed` now receives the banned rejection despite still being whitelisted;
- `SecondUser` reaches `SetCompression`.

This is runtime evidence for fresh-start/restart semantics and for unchanged ban precedence.

### Operator permissions across fresh server instances

```text
cargo test -p mc-server --test play \
  file_backed_operator_permissions_reload_across_server_instances \
  -- --exact --nocapture
PASS 1/1
```

The test drives real Login -> Configuration -> Play and inspects the actual `ClientboundCommands` root tree:

- file-backed `FileOp` sees restricted `/time`;
- ordinary `MemberOne` does not;
- `ops.json` is rewritten to `SecondOp`;
- a fresh second server no longer exposes `/time` to `FileOp`;
- `SecondOp` now receives the restricted command root.

`allow_local_dev_operators = false` is explicit in the fixture, so loopback fallback cannot make this test pass accidentally.

### `mc-server --check`

```text
cargo test -p mc-server --test cli \
  check_file_backed_access_control_is_loaded_and_missing_or_malformed_files_fail_closed \
  -- --exact --nocapture
PASS 1/1
```

The check path proves:

- valid file-backed operator/whitelist identities are visible in effective config output;
- removing an explicitly configured whitelist file exits non-zero with `auth.whitelist_file` and file-path context;
- malformed JSON exits non-zero with `parsing auth.whitelist_file JSON` context;
- semantic invalid-UUID errors include the field and resolved file path while omitting the raw invalid UUID token;
- ordinary non-`--check` startup has the same redaction behavior before entering the serving lifecycle.

## Full affected validation

```text
cargo test -p mc-server --lib --quiet
28 passed; 0 failed

cargo test -p mc-server --test login --quiet
12 passed; 0 failed

cargo test -p mc-server --test play --quiet
17 passed; 0 failed

cargo test -p mc-server --test cli --quiet
39 passed; 0 failed

cargo test -p mc-server --quiet
all runnable suites PASS; no failures
main binary suite: 60 passed / 3 ignored
```

The first full Play run exposed one unrelated stale Phase-5 test fixture still using the pre-correlated `spawn_entity(player_id, ...)` call. The fixture was migrated to `spawn_entity("pet-spawn", player_id, ...)`; no production spawn or access-control behavior changed. The full Play suite then passed 17/17.

Static gates on the current checkpoint:

```text
cargo fmt --all -- --check
PASS

cargo run -p xtask -- code-health
summary: 0 fail
verdict: KEEP

cargo clippy --workspace --all-targets -- -D warnings
PASS

scoped git diff --check
PASS
```

Benchmark: not applicable. Access files are bounded startup/check input and are not read in the steady-state login or tick path.

## Security / claim boundary

This checkpoint provides durable file-backed startup policy, not server-managed mutable access databases. Solaris does not yet expose commands that rewrite these files.

It also does not close all of M89. Remaining M89 work includes a current-tree audit/disposition of chat policy, malformed/reach/container/resource-pack anti-corruption coverage, auth abuse/concurrency limits, and a public real-client deployment gate. Those should be evaluated against the current implementation rather than copied from the older draft milestone.

## Independent review

Exactly one bounded independent read-only reviewer returned **CHANGES** with three findings:

1. semantic validation failures needed the resolved access-file path, not only the config field/index;
2. invalid name/UUID values could be echoed through the startup error chain, contradicting the no-content logging claim;
3. `std::fs::read` did not enforce a hard memory/read bound if a file changed after metadata inspection.

All three findings are fixed. Semantic failures now carry field/index/path but no raw identity value; CLI tests prove both `--check` and ordinary startup redact an intentionally distinctive invalid UUID token; the reader now opens one file handle, validates that handle's metadata, and reads only through `take(MAX + 1)` before enforcing the size fence. Focused tests, the full `mc-server` suite, formatter, code-health, strict workspace Clippy, and scoped diff-check all pass after the fixes. Per repository policy, no second reviewer was started for this checkpoint.

## Disposition

The M89 file-backed access-control checkpoint is **CLOSED**. The whole M89 milestone remains open for the separately scoped chat/anti-corruption/public-deployment work listed above.
