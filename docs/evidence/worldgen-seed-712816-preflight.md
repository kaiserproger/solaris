# Seed 712816 graphical preflight

## Scope

This checkpoint runs the existing no-debug `playable-01-join-generated-spawn`
scenario against a fresh `tellus_like` world generated from `playable.toml`
with seed `712816`. The runner uses a real Minecraft Java 26.1.2 client through
the repo-owned Gradle adapter.

The checked manifest is
[`../playable/worldgen-seed-712816-preflight.json`](../playable/worldgen-seed-712816-preflight.json).
The invocation is:

```sh
SOLARIS_REAL_CLIENT_MANIFEST=docs/playable/worldgen-seed-712816-preflight.json \
SOLARIS_REAL_CLIENT_AGENT_SCENARIO=playable-01-join-generated-spawn \
SOLARIS_REAL_CLIENT_SERVER_SEED=712816 \
SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS=240 \
bash tools/run-playable-client-gate.sh --run
```

`SOLARIS_REAL_CLIENT_FRESH_WORLD=1` and `playable.toml` remain the wrapper
defaults. The effective per-run `server.toml` and world directory are retained
with the ignored run artifacts.

## Runner contract

`SOLARIS_REAL_CLIENT_SERVER_SEED` is optional. When present, the runner accepts
only a signed 64-bit decimal integer, normalizes it to a valid TOML integer,
replaces exactly the `[data].seed` entry in the effective per-run config, fails
if the source config has no such entry, and records the normalized override in
`automation-driver.txt`. When absent, the source config's seed remains
unchanged.

## Agent-run graphical result

The 2026-07-30 run passed:

- Candidate base: `9a164c50d392a7af9b70bae5731d5a4eab5ba4e1`.
- Ignored artifact root:
  `.analysis/real-client-runs/20260730T105339Z-worldgen-seed-712816-preflight-Sjrfa0`.
- Effective config: fresh isolated world, `seed = 712816`,
  `worldgen_mode = "tellus_like"`, and `operators = []`.
- The real client entered `minecraft:overworld` at `(0.5, 71.0, 0.5)`,
  completed `playable-01-join-generated-spawn`, and disconnected cleanly.
- The view-distance-4 stream emitted all `81` visible chunks with
  `absent=0`, `degraded_delivery=false`, and no pressure abandonment.
- The `854x480` RGBA screenshot renders dry grassland terrain, trees, the
  player hand, health, hunger, and hotbar without a void or visible chunk hole.
  The development-client interaction overlay and unsigned-chat notice remain
  visible, so this is not vanilla UI-parity evidence.

Artifacts and SHA-256:

| Artifact | SHA-256 |
| --- | --- |
| `manifest.json` | `3c4c44adb376a19b6ed8ef772f39c72792f6bff92037a41bf93b95d2bb38aa66` |
| `server.toml` | `85429d12251ce13e8994df6b82ff6148c7ef151a8de4e9895af0ea3a5603c092` |
| `observations.json` | `1c7d9f5a788c7eb34c68468bf2271d2b95a2bfc625d8ebaab1316b12b5134e67` |
| `screenshots/playable-01-join-generated-spawn.png` | `2f9718a995c81cbaf60a8e898a4d5017e42d55c16eef8f9452a5657a635f99d6` |

The runner's fail-closed validator accepted the complete artifact directory.
The client log contains ordinary command-ambiguity and NeoForge shader warnings
but no runtime error or disconnect reason; the server log contains no warning,
error, or panic.

## Validation

- `bash -n tools/run-real-client-regression.sh`: PASS.
- Seeded `--prepare`: fresh world path, `seed = 712816`, Tellus mode, no
  operators, and matching `server_seed_override=712816`: PASS.
- Unset override preserves source `seed = 0` and emits no override metadata:
  PASS.
- Leading-zero `001` normalizes to `seed = 1`; invalid `12x` and out-of-range
  `9223372036854775808` overrides fail before preparation with status `2`:
  PASS.
- Manifest JSON projection, Markdown paths, and scoped diff checks: PASS.
- The complete graphical run and its built-in fail-closed artifact validation:
  PASS.

`cargo run -p xtask -- code-health`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo fmt --all -- --check`: PASS.

## Evidence boundary

Passing this route proves that the exact seed reaches Play and renders generated
spawn terrain and the player HUD without operator privileges or debug commands.
It does not replace the owner's subjective terrain/playability disposition,
ordinary traversal, restart/rejoin, or release-host throughput gates.

Manual/client disposition: the agent-run graphical preflight passed. The owner
playtest remains not run.

Benchmark: not applicable. The observed stream timings are diagnostic run
metadata, not the pending same-host release throughput comparison.
