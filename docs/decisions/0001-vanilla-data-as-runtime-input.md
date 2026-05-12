# ADR 0001 — Vanilla Mojang data files are permitted as runtime/build input

**Date:** 2026-05-12
**Status:** Accepted
**Supersedes:** PROJECT_SPEC §8.1 (original wording)

## Context

The original legal posture in PROJECT_SPEC §8.1 was:

> - We do not use decompiled Mojang source in our code
> - We do not use Mojmap mappings to inform our code — we write our own,
>   working from documentation
> - All assets (textures, models, sounds) are original or community-made
>   under permissive licenses

In M1.e we hit the first place where this posture becomes load-bearing:
the Configuration state requires the server to send `Registry Data`
packets describing dimension types, biomes, damage types, painting
variants, enchantments, instruments, jukebox songs and many more. In
26.1.2 this is roughly **2,000 individual JSON entries** across ~25
top-level registries. Hand-authoring them from `minecraft.wiki`
documentation is a multi-day busy-work item that produces no game
behaviour — the data must match vanilla bit-for-bit or clients refuse
to enter Play, so there is no design space to "do better".

## Decision

**Vanilla data files** (the contents of `data/` inside the official
server jar — JSON entries for registries, tags, loot tables, recipes,
worldgen, etc.) **are permitted as build-time and/or runtime input for
Solaris.**

Crucially, they are **not** vendored in our git repository. The
extraction script in `tools/extract-vanilla-data.sh` populates
`data/vanilla/` locally from a player- or developer-supplied
`.analysis/server.jar`. The contents of `data/vanilla/data/` and
`data/vanilla/version.json` are listed in `.gitignore`; only
documentation files (`README.md`, future schema notes) live in version
control. Distribution of a Solaris release therefore bundles the
extraction script and instructions, not Mojang's bytes.

What is **still** off-limits:

- Decompiled Mojang Java source code or its translation to Rust.
- Mojmap mappings (e.g. `client.txt`, `server.txt`) used to inform
  Rust type/method names.
- Bundled binary assets that aren't strictly data (textures, models,
  sounds, fonts).
- Bytecode-level copy of any class file.

What is now **permitted**:

- The JSON contents of `data/minecraft/**` inside the vanilla server
  jar (registries, tags, loot tables, recipes, worldgen JSON).
- Binary NBT structures (`data/minecraft/structure/**/*.nbt`) used as
  worldgen building blocks, when we eventually need them in M4+.
- Pack metadata files (`pack.mcmeta`).

## Consequences

Positive:

- M1.g (and later milestones) can ship working Registry Data and
  UpdateTags packets without hand-authoring thousands of entries.
- The data shipped is identical to vanilla, which is the *only*
  correct answer for protocol-level compatibility.
- Server upgrades to new Mojang patch releases mean extracting a fresh
  copy with `tools/extract-vanilla-data.sh`, not weeks of manual
  re-derivation.

Negative:

- The build/run pipeline now depends on `data/vanilla/` being
  populated, which means a fresh clone of the repo cannot run the
  server without first dropping a `server.jar` into `.analysis/` and
  running the extraction script. The Solaris README needs to document
  this clearly.
- Code consuming the data has to handle the "registry not populated"
  case gracefully (e.g. fail with a clear error pointing at the
  extraction script rather than crashing).
- Releases ship the extraction script and instructions, not the
  Mojang data itself. We never redistribute Mojang's bytes through
  our git history, our crates.io publishes, or our Modrinth/CurseForge
  uploads.

## Implementation

- `tools/extract-vanilla-data.sh` extracts the relevant subset from
  `.analysis/server.jar` into `data/vanilla/`. Re-runnable.
- `data/vanilla/README.md` (tracked) documents the provenance and
  reproduction; `data/vanilla/data/**` and `data/vanilla/version.json`
  are `.gitignore`d.
- `.analysis/` itself is `.gitignore`d except for `*.md` files, which
  is the place engineering notes about the bundled jar live.
- `PROJECT_SPEC.md` §8.1 updated with a forward link to this ADR.

## Notes

This decision was made on the express instruction of the project owner
during the M1.e → M1.g handoff. The original PROJECT_SPEC §8.1 wording
reflected an earlier, more cautious stance.
