# Solaris v0.0.3-alpha.1 Field-Test Plan

Date: 2026-08-28
Target: `v0.0.3-alpha.1`

`v0.0.2-alpha.1` is closed with every automated release gate green. The first
owner run of that release is now the source of truth for alpha 3 priorities.
The server stayed notably light in memory (~80 MiB cold, ~140 MiB with one
player by owner observation), but ordinary survival exposed correctness and
operator/product gaps that take priority over adding breadth.

## P0 — Fix the survival blockers first

1. [x] Fix ordinary player collision validation. The reopened false corrections were
   traced to the player verifier reusing the general continuous diagonal entity sweep,
   while vanilla 26.1.2 resolves Y then the larger horizontal component then the other
   component before applying the moved-wrongly residual. The verifier now has that
   axis-separated path without changing general entity/projectile sweep semantics or
   widening the `0.0625` residual budget; deliberate tunnelling and residency fences
   remain. Evidence: `docs/evidence/alpha3-core-move-001-2026-08-29.md`.
2. [x] Fix item pickup authority/publication ordering. Item candidates require actual
   player/item touch-box overlap (XP and grounded arrows retain radius semantics), and
   the owner transaction revalidates the current player pose, current claimed item
   snapshot and exact claim token under the commit lock before inventory credit.
   Move-away and claimed-item-relocation races roll back without publication.
3. [x] Re-close executable and real-client regression coverage. `mc-physics` is 79/79,
   movement-focused `mc-net` is 57/57, full `mc-net --lib` is 1984 passed / 5 ignored,
   scoped strict Clippy/fmt and code-health pass. Post-review graphical validation ran
   1800 ordinary sprint+jump client ticks / 90 seconds across twelve chunk coordinates,
   inventory and reconnect with **0 `SweptCollision` / `WorldUnavailable` / runtime
   unavailable / panics**, then two natural spruce-log face-pressure + diagonal-release
   routes also passed with zero corrections. Deliberate tunnelling remains rejected.
   Evidence: `docs/evidence/alpha3-core-move-001-2026-08-29.md`.

## P1 — Natural population control

1. [x] Replace cadence-only tuning with an operator-visible spawn policy. Keep
   friendly/hostile cadence, but expose bounded category/global caps and rotating
   active-chunk budgets. Document simulation-distance, support/light/collision,
   and despawn/refill interaction. Evidence:
   `docs/evidence/alpha3-core-spawn-002-2026-08-29.md`.
2. [x] Raise the starter/default population from the sparse alpha baseline to a
   visibly alive but still lightweight common-survival profile. The selected
   profile is measured at friendly `32`, aquatic `20`, hostile `70`, with
   friendly/hostile intervals `400`/`20` and friendly chunk budget `48`.
3. [x] Keep biome/species rules explicit; spawn-density controls do not bypass
   support, darkness, player-distance, collision, or loaded-simulation-chunk
   fences. Focused policy coverage is recorded in the same evidence.

## P1 — Server configuration and operator UX

1. [x] Make network port/bind configuration obvious in the starter config/README
   and CLI help/check output. `[network].bind_address` / `port` are now first-class
   quick-start settings in README, `docs/OPERATING.md`, `example.toml` and
   `playable.toml`; `--check` reports the normalized endpoint. The underlying port
   support already existed — this closes the owner-facing discoverability defect.
2. [x] Add a normal operator workflow: `mc-server --config server.toml operator
   add|remove|list <name-or-uuid>` manages a bounded persisted profile. With no
   `admin.operators_file`, the CLI uses `ops.json` beside the config and startup
   auto-loads it when present; malformed identities/files fail closed. No
   local-dev mode or hand-editing is needed. See `docs/OPERATING.md`.
3. [x] Return useful Solaris server identity/info in the vanilla client's F3/server
   diagnostics where the protocol permits it. Configuration publishes the tested
   `Solaris <version> (MC 26.1.2)` brand without Loader.
4. [x] Add an **optional, default-off, lightweight plain dashboard**: first-party
   hand-rolled HTTP endpoint + embedded single-file page (no new dependencies),
   read-only, loopback-bound by default with explicit `allow_remote`
   acknowledgement, covering uptime/version, players, TPS/tick latency per
   stage, memory, autoscale, chunk counters, entity categories, spawn metrics,
   save state, network pressure, plugin status, and a bounded warnings ring.
   Evidence: `docs/evidence/alpha3-dashboard-003-2026-08-30.md`.

## P1 — World generation after owner rejection

Owner disposition for seed `712816`: **REJECT for alpha-3 quality work**. Concrete
field defect: the traversed world reads as repeated/banded endless savanna and lacks
large-scale biome cohesion/credible contiguous biome regions. Automated traversal,
restart, survival and throughput evidence remain valid and should not be reopened
unless they regress.

1. [x] Run a critical seed review (agent + mosaics + first-person evidence)
   focused on biome spatial coherence, transition width, regional identity and
   repeated striping/layering. Do not optimize aesthetics from one screenshot alone.
2. [x] Replace the coherence defect with a measured multi-scale biome-domain model:
   broad regions must stay internally coherent while still allowing climate-driven
   transitions, rivers/coasts and local variation. Add multi-seed metrics that can
   detect excessive directional striping and over-fragmentation as well as one
   biome dominating the map.
3. [x] Add an optional `realistic_deposits` ore profile: deterministic geological
   deposits/lenses/ore bodies can form mine-like targets instead of only
   vanilla-style independent veins. The canonical profile is selected by the
   shipped plugin manifest, validates every normal/deepslate resource at startup,
   and has deterministic adjacent-chunk/large-component coverage. Vanilla remains
   the default; profile changes remain a fresh-world contract field.
4. [x] Re-run seed `712816` plus a multi-seed visual/fingerprint matrix only after
   the coherence model changes.

Item 1's owner disposition remains **REJECT** for the pre-model terrain. The
post-model capture is fresh agent-run evidence; it does not silently change the
owner's verdict. The next acceptance action is an explicit owner review of the
new terrain.

## P1 — Documentation and Loader usability

1. [x] Rewrite/update the root README for the current alpha-3 development line rather
   than the older prototype state: install/run/check, config/auth/world contract,
   bind/port, operator CLI workflow, autoscale and plugin deployment are current;
   the optional dashboard and standard pack remain named explicitly as future work.
2. [x] Expand plugin documentation around manifests, capabilities, deployment
   (`server_only` vs Loader-required), storage/events/commands/menus, lifecycle/reload,
   examples, strict expected-set operation and debugging in `docs/PLUGINS.md`.
3. [x] Document Solaris Loader installation for Fabric, NeoForge and Forge 26.1.2 in
   `docs/SOLARIS_LOADER.md`, including exact tested baselines, jar build/placement,
   first-connect permission/cache behavior, Loader-required disconnect semantics and
   troubleshooting; all three Loader jars were built in the docs checkpoint.

## P2 — Optional first-party standard plugin pack

The server core must remain useful without this pack. Installation is opt-in from a
starter/bundle mechanism and individual plugins remain independently removable.
Research popular server-plugin workflows before freezing scope; do not clone giant
legacy plugins wholesale.

Initial candidates:

1. [x] `solaris-essentials` — lightweight homes/warps/spawn/back/tpa/admin utility
   subset with permission-aware commands; ordinary commands remain usable
   server-only (no Loader requirement).
2. [x] `solaris-economy` — simple durable balances, pay/admin balance commands and
   idempotent transfer tokens with a bounded ledger; single money authority.
3. [x] `solaris-towns` — deliberately small Towny-like claims/towns/members/roles
   and leader protection rules.
4. [x] `solaris-audit` — CoreProtect-like bounded block/container/player-action
   history with lookup/inspect primitives; storage stays append-oriented and
   bounded, off the hot path.
5. [x] Scope decision recorded after Bukkit/Paper ecosystem research: the pack
   ships `solaris-permissions` plus the four plugins above as independent
   external-directory API 0.6 packages with no bundled-selection wiring;
   intentionally omitted components (giant command catalogs, multiworld
   teleport, auctions, nations/war, bulk editing, WorldGuard-style regions,
   unbounded logs, guessed rollback) and rationale live in
   `examples/plugins/standard-pack/README.md`.
   Evidence: `docs/evidence/alpha3-plugin-pack-005-2026-08-30.md`
   (integration coverage: `crates/mc-test-harness/tests/plugin_standard_pack.rs`).

## Release-3 closeout rule

Do not tag `v0.0.3-alpha.1` while P0 collision or pickup ordering remains reproducible.
After P0 is green, close feature slices independently with focused tests and one
independent review; run full release L2 and real-client smoke only at the final
release checkpoint.
