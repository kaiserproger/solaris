# ADR 0001 — Vanilla data as local runtime input

Status: accepted. Updated 2026-09-05 for the core rewrite.

## Contract

Solaris targets Minecraft Java 26.1.2. Registry, tag, recipe, loot, component,
block-property, protocol and world-format facts must come from the matching
local vanilla distribution, not remembered packet layouts or guessed values.
The owner-supplied oracle is `.analysis/server.jar`.

Local extraction, `wire-probe`/`javap`, decompiled-source inspection and
side-by-side execution establish observable behavior. They do not authorize
copying Mojang implementations or mechanically translating them into Rust.
Mojang source, mappings, class files and assets are not redistributed in Git,
crates or release bundles. Solaris ships its own implementation and extraction
tools; `.analysis/` and extracted `data/vanilla/` remain local.

## Ownership

- `mc-data` owns decoded immutable data and the deliberately supported embedded
  data set. Protocol evidence belongs to ADR 0002.
- `mc-server::startup_data` owns startup selection, provenance and validation.
  `main` composes the selected values into the running server and check report;
  it does not implement loader policy.
- Every loader returns one `Effective<T>` representation: owned value and static
  source label. This adds no runtime queue, cache, trait object or allocation.
- `startup_validation` continues to own configuration and persisted-world
  validation. Data loading uses its sidecar-version check.

## Selection and failure semantics

An explicitly configured sidecar is validated and loaded or startup fails with
an actionable error. Missing or malformed configured data must not silently
select embedded data. The existing no-sidecar path remains explicit.

Update 2026-09-14 (owner direction; implemented). The two-mode selection above
is superseded: Solaris runs on the real vanilla data with no manual sidecar step
and no embedded-subset mode. One managed source exists — a distributable, staged
importer (`mc-server content import --version 26.1.2 [--from <jar>|--download]`)
that derives a complete cache from the operator's own licensed Minecraft
artifact (local jar, or a launcher-style download from Mojang's public metadata
verified against the manifest's size and SHA-1) and publishes it atomically, so a
failure leaves the previous valid cache intact. A normal launch calls the same
packaged import path automatically before binding the listener when no valid
cache exists; startup otherwise only *discovers* a complete cache
(`[data].vanilla_data_dir` override → `SOLARIS_CONTENT_CACHE` → the user-level
cache → `./data/vanilla`) and reuses it offline. Absence of any licensed source
fails loudly, naming the prerequisite, the searched locations and the command to
run. `mc-server::startup_data` is the only consumer of the resolved directory and
its loaders now take a required path — no per-domain hardcoded selection and no
optional parity mode. The input/output classification and the observed launcher
flow are recorded in [MEMORY.md](../MEMORY.md).

Preserve the existing bounded compatibility behavior during replacement:
resolved required tag registries and canonical furnace fuels; matching block
property versions/state coverage; nonempty item facts; recipe stack limits;
stable embedded recipe ordering with cache overrides; and the documented
simple-loot subset with embedded completion. These are current constraints, not a
claim of complete vanilla data coverage.

The embedded fallbacks in `mc-data` (`solaris_required_*`, the conservative
light table) are no longer a startup selection: with the importer landed every
loader reads the cache. They remain seed data for tests and for the
block/entity/biome domains no cache currently supplies, listed as the delta in
[MEMORY.md](../MEMORY.md).

## Proof

A loader change must preserve normal startup, `mc-server --check` values and
provenance, and rejection before listener startup for invalid configured data.
Use focused server tests and an actual CLI/generated-startup run. Wire or
client-visible changes additionally require their matching vanilla/client
scenario; a data-loading refactor alone does not establish gameplay parity.

The replacement architecture and capacity policy are in
[ARCHITECTURE.md](../ARCHITECTURE.md). Current evidence is in
[MEMORY.md](../MEMORY.md), not historical milestone prompts.
