# Solaris product contract

Updated 2026-09-05. This document states what Solaris must deliver, not what the
current tree has already proved. [MEMORY.md](MEMORY.md) owns current state;
[ARCHITECTURE.md](ARCHITECTURE.md) owns the replacement design.

## Product

Solaris is an authoritative Minecraft Java 26.1.2-compatible Rust server with a
sandboxed addon platform and a shared client Loader. Ordinary server-only play
works with the vanilla client. Negotiated Loader capabilities provide custom
content, input and presentation; unsupported client features are explicit, not
silently replaced by fake implementations.

The purpose is efficient common gameplay and multiplayer plus a broad, uniform
server/client API. Content such as colonies, economies, machines, recruits and
bosses belongs in addons rather than dedicated Rust gameplay entry points.
The owner-requested core rewrite minimizes mechanisms, duplicated authority and
unnecessary code while preserving useful domain implementations and behavior.

Priority order:

1. Ordinary vanilla-client gameplay, multiplayer and normal save integrity.
2. Production addon API, gameplay adapters and Loader-backed client functions.
3. Measured performance, regional ownership, ECS and autoscaling.
4. Uncommon parity edges and rare error interleavings.

## Compatibility and invariants

- Use the exact target release for protocol/layout evidence and local vanilla
  comparison. A patch upgrade is an explicit compatibility change.
- Preserve authoritative movement, item conservation, lifecycle ordering,
  permission checks and validated commits. Rejection mutates nothing; accepted
  mutations and committed effects must not disappear under overload.
- Preserve vanilla world-format compatibility and documented persistence/
  recovery guarantees. No stronger durability claim follows from a unit test.
- Vanilla behavior is the reference within declared capacity. Deliberate bug
  fixes and other divergences require explicit scope and evidence; neither
  bit-identical terrain/RNG nor global parity is implied by a playable demo.
- Beyond declared capacity, protect the kernel and healthy players through
  explicit admission refusal and local delay of expensive work. Do not lose
  items, partially commit transactions or silently drop accepted work.
- Solaris internals have no released compatibility surface. Replacements move
  every caller and remove the old authority, API or schema; no permanent shims.

## Extension contract

Use one package identity, capability/permission model, schema and SDK for server
and client functionality. Reuse the common Loader behind thin platform adapters.
Keep strict Luau unless a measured requirement justifies changing the runtime.

Expose owned values, opaque handles, immutable observations, bounded requests
and explicit completion/rejection. Addon authors do not manage engine threads,
locks, region ownership or migration. Limits cover induced engine work and
client resources as well as script execution; a small script must not bypass
budgets by requesting an enormous scan, edit, entity batch or rendered scene.

Server authority is independent of client input and presentation. Dynamic
connection activation and registration before Minecraft's registry freeze are
distinct lifecycle operations. Features requiring restart must say so. The
Loader is not a channel for downloading arbitrary Java/native code.

The executable API remains documented in [PLUGINS.md](PLUGINS.md); current client
support and limitations are in [SOLARIS_LOADER.md](SOLARIS_LOADER.md). A target
contract does not promote unimplemented API to supported behavior.

## Content acceptance workloads

The original modpack remains an extensibility workload, not binary compatibility
with its Java mods or a requirement to hard-code these mechanics into the core:

- Decoration: at least 50 blocks across furniture/building families, sitting,
  correct collision and crafting.
- Farming/cooking: at least eight tools/stations, 30 foods and four crops;
  growth, food properties, cooking UI/recipes and multi-output processing.
- Recruits/workers: hiring, follow/combat behavior, equipment, salary/feeding
  and at least lumberjack, miner and farmer professions.
- Equipment: at least four armor sets and eight weapons, custom models,
  shields, two-handed behavior and crafting.
- Siege: at least three engines, multiblock construction, riding/input,
  aiming, reloading and projectiles with block damage.
- Firearms: at least three weapons, reload timing, ballistics, muzzle effects,
  bayonets and ammunition consumption.

These describe observable capability. They do not promise copied assets,
identical balance, third-party mod compatibility patches or a completed pack.

## Platforms and scope

Linux x86_64 is the primary server platform; Windows x86_64 is best-effort.
Client support follows Minecraft and the actual Loader adapters. Cargo/toolchain
and Gradle configuration are authoritative for dependencies and build versions;
there is no second dependency or thread-pool specification in this document.

No Bedrock support, binary execution of Forge/Fabric/NeoForge server mods, or
custom launcher requirement. Players use a legitimate Minecraft installation
and the documented launcher/Loader setup. Mojang inputs stay local; licensing
and data provenance are governed by [ADR 0001](decisions/0001-vanilla-data-as-runtime-input.md)
and protocol evidence by [ADR 0002](decisions/0002-vanilla-protocol-metadata-as-reference.md).

## Acceptance evidence

- Exercise the real server and graphical client, including ordinary survival,
  multiplayer publication and save/restart. Raw-TCP tests do not replace the
  graphical gate in [playable/ACTIVE.md](playable/ACTIVE.md).
- Preserve workload populations, tick counts, assertions and measurement scope
  when comparing implementations. Report hardware, affinity, build mode and
  failures; one host is not a heterogeneous fleet.
- The retained product performance target is 20 concurrent players at view
  distance 8 above 18 TPS on a 4-vCPU/8-GiB VPS. It is a target, not current proof
  or permission to weaken a stricter existing workload.
- Public release additionally requires crash-recovery evidence, usable addon
  content, Loader distribution, operator/player setup and API documentation.
  Hard readiness claims follow [DEFINITION_OF_DONE.md](DEFINITION_OF_DONE.md)
  and its exact evidence matrix, not historical bootstrap prompts or line count.

Run and operator instructions belong in [README.md](../README.md) and
[OPERATING.md](OPERATING.md). Historical milestone/evidence records are retained
for provenance, not as competing implementation plans.
