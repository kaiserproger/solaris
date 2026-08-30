# Alpha-3 operator and deposits checkpoint

Date: 2026-08-30
Status: **complete — focused implementation and validation green**

## Operator workflow

`mc-server` now exposes a discoverable `operator` subcommand with `add`,
`remove`, and `list` actions. The command edits the configured vanilla-style
JSON operator profile, or the deterministic `ops.json` profile beside the
selected config when no path is configured. It normalizes Minecraft
usernames/UUIDs, deduplicates identities, preserves unknown profile metadata,
emits deterministic listing order, and fails closed on malformed identities or
files. No local-development operator mode or hand-editing is required.

`admin.operators_file` remains optional in TOML; server startup auto-loads the
default `ops.json` only when that file exists. Add/remove changes are
restart-stable and are not hot-reloaded into a running server. Invalid `add`
identities have no file side effect.

Focused coverage:

- `crates/mc-server/src/main.rs`: CLI parsing and default-path dispatch;
- `crates/mc-server/src/lib.rs`: persistence, normalization, deduplication,
  default-file loading, removal, and unknown-metadata preservation;
- `docs/OPERATING.md` and `README.md`: operator procedure and boundaries.

Smoke evidence from `.analysis/operator-cli-smoke-default/`:

```text
cargo run --quiet --bin mc-server -- --config .analysis/operator-cli-smoke-default/server.toml operator add SmokeOp
operator SmokeOp added

cargo run --quiet --bin mc-server -- --config .analysis/operator-cli-smoke-default/server.toml operator list
smokeop

cargo run --quiet --bin mc-server -- --config .analysis/operator-cli-smoke-default/server.toml operator remove SmokeOp
removed operator SmokeOp
```

The resulting config remains unchanged and the final operator file is the valid
empty JSON array `[]` after removal.

## Realistic deposits profile

`realistic_deposits` is now the canonical Lua manifest, persisted world-contract,
shipped `geological-mines` example, and terrain API spelling. The old
`geological_deposits` parser/API aliases were removed; vanilla remains the
no-declaration/default profile. Startup validates every normal and deepslate
ore resource. The profile uses deterministic cross-chunk deposits and keeps
vanilla independent veins disabled for the selected fresh-world contract.

Focused coverage:

- `shipped_realistic_deposits_plugin_selects_the_startup_ore_profile`;
- `realistic_deposits_require_normal_and_deepslate_ore_resources`;
- `realistic_deposits_place_deterministically_across_adjacent_chunks`;
- `realistic_profile_replaces_vanilla_veins_with_cross_chunk_deposits`.

## Validation

```text
cargo test -p mc-server --bin mc-server --quiet
61 passed; 0 failed; 3 ignored

cargo test -p mc-server --lib --quiet
31 passed; 0 failed

cargo test -p mc-script --lib --quiet
85 passed; 0 failed

cargo test -p mc-worldgen --lib --quiet
114 passed; 0 failed; 5 ignored

cargo test -p mc-worldgen --test core_rules --quiet
12 passed; 0 failed

cargo run -p xtask -- code-health
0 fail / KEEP

cargo clippy -p mc-server -p mc-script -p mc-worldgen --all-targets -- -D warnings
PASS

cargo fmt --all -- --check
PASS
```

This closes the operator and optional deposits rows in
`docs/PUBLIC_ALPHA3_PLAN.md`. It does not claim final alpha release readiness:
the optional dashboard and standard plugin pack remain open, the owner terrain
review for seed `712816` remains explicit, and the final workspace/release-host
and owner-run survival gates are still required before any release tag.

## Review disposition

Exactly one independent read-only reviewer returned `changes` for two issues:
line-based TOML rewriting could corrupt dotted/inline admin syntax, and invalid
fresh `add` could leave an empty file side effect. The final implementation
removes TOML rewriting, auto-loads a default `ops.json` only when present, and
normalizes identities before any file write. Main and library focused gates were
rerun after both fixes; no second reviewer was spawned.
