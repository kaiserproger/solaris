# ADR 0002 — Unobfuscated vanilla protocol metadata is permitted as a reference

**Date:** 2026-05-12
**Status:** Accepted
**Extends:** [ADR 0001](0001-vanilla-data-as-runtime-input.md), PROJECT_SPEC §8.1
**Supersedes:** the part of §8.1's source-level prohibition that, taken
literally, also banned reading the bytecode-level **metadata** (class
names, field names, packet ID constants, record layouts) of the official
26.1+ vanilla server.

## Context

PROJECT_SPEC §2.1 locks Solaris to vanilla 26.1.x, which Mojang
explicitly released "fully unobfuscated" — the class and field names in
the official server jar **are** the canonical public names for those
constructs. There is no longer a separate "mojmap" mapping layer; the
production jar is itself the reference.

PROJECT_SPEC §8.1's original prohibition was written against the
historical pre-26.1 reality where reading mappings was a deliberate
de-obfuscation step that required external knowledge (Mojang's
mappings.txt). With 26.1+ that distinction has collapsed: reading the
class names from `client.jar` / `server.jar` is operationally the same
as reading [`minecraft.wiki`](https://minecraft.wiki) — both are
public documentation of the same identifiers.

M1.g.4 hit the practical consequence: the runtime wire-capture probe
verified three Play-state packet IDs (LoginPlay 0x31,
SynchronizePlayerPosition 0x48, GameEvent 0x26) but left
ClientboundKeepAlive / ServerboundKeepAlive / ConfirmTeleportation /
SetDefaultSpawnPosition unverified because they don't fire often
enough to land inside a short capture window. Continuing to pin them
by wire-observation is possible but slow and incomplete — vanilla has
~180 packets in the Play state alone, and even one wrong ID kicks a
real client off the connection. A metadata-level reference would let
us cross-check every packet at once.

## Decision

**Reading the metadata of the official, unobfuscated 26.1+ vanilla
jar is permitted as a reference input for Solaris' protocol code.**

Specifically, the following are now allowed:

- Running `javap`, `unzip`, `strings`, `xxd`, `nm` etc. against the
  bundled `client.jar` / `server.jar` to read class names, field
  names, field types, record component orders, enum members, and
  static `int`/`String` constants.
- Treating those names and constants as *documentation*: e.g., reading
  `ClientboundLoginPacket.dimensions()` to confirm that our
  `LoginPlay.dimension_names` field is a `List<Identifier>` and lives
  at the same wire position.
- Comparing extracted protocol ID maps (`ConnectionProtocol`-style
  registration tables) against the IDs we hard-code in
  `crates/mc-protocol/src/packets/*.rs`.

Still **disallowed**:

- Translating decompiled Java source code to Rust.
- Copying algorithmic logic (e.g., light-propagation, RNG, worldgen
  formulas) out of decompiled vanilla classes — those are M4+
  concerns and stay subject to the original §8.1 rule.
- Using Mojang's class names verbatim as our Rust type names. Our
  enum/struct naming is independent — for example, vanilla's
  `ClientboundLoginPacket` corresponds to our `LoginPlay` (located in
  `packets::play`). Cross-references go in code comments, not in
  identifiers.
- Naming any Rust constant after a non-public Mojang internal that
  isn't part of the wire format (e.g., field-mangled scratch
  variables in worldgen).
- Redistributing extracted class files. Like ADR 0001's data files,
  any extracted classes live outside the git tree (`.analysis/...`
  remains `.gitignore`d).

## Consequences

Positive:

- Every M1.g packet can be cross-checked against `javap` output
  rather than waiting for a 15-second keepalive heartbeat in a
  60-second wire capture.
- Future milestones that add packets (M2+ chunks, M3+ player actions,
  M5+ block updates) inherit the same cheap verification path.
- Reduces the risk of shipping a protocol bug that only manifests on
  a real client — the wire-probe captures we already have are
  necessary but not sufficient.

Negative:

- A nontrivial change to the legal posture set in PROJECT_SPEC §8.1.
  Mitigations:
  - The prohibition on translating *source code* and *algorithms* is
    intact. Only structural metadata (the same information available
    in any third-party Java protocol library) crosses the line.
  - We never redistribute Mojang's class files; extraction stays
    local under `.analysis/` (per ADR 0001's pattern).
  - The naming-independence rule means a casual reader of Solaris'
    Rust source cannot tell whether a packet struct was derived from
    `javap`, from minecraft.wiki, or from a wire trace. The shape of
    the code is the same.

## Implementation

- `.analysis/extracted/` keeps the unpacked classes from
  `tools/extract-vanilla-data.sh` (the script is extended in this
  ADR to also drop `client.jar` / `server.jar` contents under
  `.analysis/extracted/` when present).
- A short script `tools/dump-vanilla-protocol.sh` runs `javap -p`
  over the `net.minecraft.network.protocol` package and writes a
  human-readable summary to `.analysis/protocol-dump.txt`. The
  dump is not committed.
- PROJECT_SPEC §8.1 is amended to point at this ADR.
- Solaris commits that change packet IDs/field orderings as a
  result of this reference should cite `javap` output (and the
  vanilla class name) in the commit message, so future maintainers
  can re-derive the same conclusion.
- Configuration now sends `ClientboundUpdateEnabledFeaturesPacket` as packet
  `0x0c` with `minecraft:vanilla` before known-pack negotiation. The ID, set
  encoding, and ordering come from the local 26.1.2
  `ConfigurationProtocols`, `ClientboundUpdateEnabledFeaturesPacket`, and
  `ServerConfigurationPacketListenerImpl` sources.
- Chunk section encoding publishes both `nonEmptyBlockCount` and the exact
  non-empty fluid-state count. Local 26.1.2 `LevelChunkSection.read` stores the
  second short in `fluidCount`; `EntityFluidInteraction.hasFluidAndLoaded`
  checks `LevelChunkSection.hasFluid()` before scanning entity overlap. Sending
  zero therefore disables all client-local water contact even when the block
  palette itself contains valid source-water states.

## Notes

This decision was made on the express instruction of the project
owner during the M1.g.4 wire-validation work, after the runtime probe
verified three Play-state IDs and the remaining unverified ones were
diagnosed as practically unobservable in a reasonable capture window.
The owner's exact phrasing: "Может просто вытащишь код пакетов и сети
из ванилы напрямую?"
