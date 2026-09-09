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

Preserve the existing bounded compatibility behavior during replacement:
resolved required tag registries and canonical furnace fuels; matching block
property versions/state coverage; nonempty item facts; recipe stack limits;
stable embedded recipe ordering with sidecar overrides; and the documented
simple-loot subset with embedded completion. Optional no-sidecar mining and
explosion tables remain absent for the server's existing embedded selection.
These are current constraints, not a claim of complete vanilla data coverage.

## Proof

A loader change must preserve normal startup, `mc-server --check` values and
provenance, and rejection before listener startup for invalid configured data.
Use focused server tests and an actual CLI/generated-startup run. Wire or
client-visible changes additionally require their matching vanilla/client
scenario; a data-loading refactor alone does not establish gameplay parity.

The replacement architecture and capacity policy are in
[ARCHITECTURE.md](../ARCHITECTURE.md). Current evidence is in
[MEMORY.md](../MEMORY.md), not historical milestone prompts.
