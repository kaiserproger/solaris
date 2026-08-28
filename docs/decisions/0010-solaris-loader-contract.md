# ADR 0010 - One untrusted client-bundle contract across loaders

**Date:** 2026-07-23
**Status:** Accepted, staged implementation

## Problem

Rich plugins need client-side blocks, items, screens, assets, and interactions.
Implementing separate server contracts for Fabric, NeoForge, and Forge would
let their security, version, and cache rules drift. Treating a plugin artifact
as trusted merely because the server supplied it would also give server content
implicit access to the client.

## Decision

Use one versioned JSON descriptor and acknowledgement protocol across all three
loaders. Plugin discovery owns the source manifest and rejects the server at
startup unless every bundle has:

- a bounded plugin-local id and version;
- a relative canonical artifact path;
- an exact lowercase SHA-256 and declared byte size;
- one or more supported loaders and content kinds;
- explicit permissions matching every content kind; and
- a cache identity derived from plugin id, bundle id, version, and SHA-256.

The server aggregates descriptors in deterministic plugin order. During the
Minecraft Configuration state it sends protocol 1 on
`solaris:loader/manifest`. Before accepting the normal finish acknowledgement,
it requires `solaris:loader/ack` with the same protocol, a supported platform,
a bounded loader version, every required permission, and every exact cache
identity. When no plugin declares client content, Solaris sends no loader
payload and does not require a modded client.

The Java `loader-core` module owns the matching codec and validation. Fabric,
NeoForge, and Forge adapters supply only platform identity, loader version,
granted permissions, and cached identities. Platform networking and lifecycle
code must call this shared core rather than reinterpret the plugin manifest.

Downloaded bytes remain untrusted. A missing exact identity is requested
explicitly; the server may answer only with the matching validated plugin
artifact. Transfer chunks carry the exact cache identity, contiguous byte
offset, and final marker. The client writes them to staging in the destination
filesystem, verifies size and SHA-256, and atomically publishes the declared
identity. Registration may use only permissions the user granted for that
server/content. A failed check cannot fall back to partially loaded content or
a stale same-version artifact.

## Current staged boundary

The implemented first slice validates plugin descriptors, sends the server
manifest, validates the acknowledgement, and provides the shared Java core plus
three platform adapters. Fabric, NeoForge, and Forge register manifest,
request, artifact, and acknowledgement payloads during Configuration through
their native 26.1.2 networking APIs. Missing bundles are streamed in bounded
chunks, verified, and atomically published before the shared core emits the
acknowledgement. Before any request or staging, the shared core resolves an
allow/deny decision keyed by normalized server address and the exact requested
permission set. An unknown decision opens a Minecraft confirmation screen on
all three platforms and is atomically persisted under the Loader cache. Denial
disconnects without downloading or acknowledging. The active Loader exchange
uses a bounded two-minute read timeout so the user can answer; ordinary
pre-Play reads retain the ten-second timeout. Each network connection owns its
transfer controller and generation: an inactive connection cannot persist a
late prompt answer, and its artifact packets cannot reach a newer connection's
staging session.

After every exact cache file passes verification, the shared core requires the
first ZIP entry to be a closed `solaris-client.json` schema. The current
activation slice accepts owner-namespaced screen, item, and interaction
definitions plus declared asset bytes, rejects unknown fields and archive
entries, verifies each asset's exact size and SHA-256, and bounds the combined
immutable registry. Fabric,
NeoForge, and Forge publish that registry before acknowledgement and retain it
into Play. Denied, malformed, unverified, or not-yet-supported content cannot
activate. Disconnect clears the process registry, preventing content from one
server from leaking into a later vanilla connection.

The Configuration outcome carries the exact Loader acknowledgement into the
same Play session. A host-attested plugin may request only an
owner-namespaced activated screen backed by `screens` and `open_screens`.
Solaris publishes the bounded raw id payload only to that Loader-eligible
session. Each platform adapter captures the packet's originating connection
before queueing client-thread work and opens the activated title/body view only
if that exact connection is still current.

Verified asset entries are exposed through one shared in-memory Minecraft
client pack, keyed by their exact `assets/<namespace>/<path>` archive
locations. Fabric registers the transient repository source through a narrow
accessor because vanilla does not expose source insertion there; NeoForge and
Forge use their public repository hook. Loader acknowledgement waits until the
Minecraft resource reload returns the exact bytes from that pack. The mount is
owned by the Configuration origin and is removed by that connection's close
notification, so stale disconnect work cannot clear a newer mount. Blocks
remain outside this stage.

The interaction slice extends the closed archive index with bounded
owner-namespaced actions. Each action references a screen from the same bundle
and carries a bounded label plus static UTF-8 payload. All three adapters render
those actions and send one raw Play payload only while the exact definition and
originating connection remain active. The server accepts that channel only from
the same Play session that completed Loader acknowledgement, requires the
owner's `interactions` plus `send_interactions` declaration, and publishes a
required targeted `loader.interaction` event solely to that Luau owner. The
plugin receives the client payload as untrusted data.

The item-presentation slice adds up to 128 owner-namespaced item declarations.
Each declaration names one known vanilla base item and derives its client model
identifier from its own id. Activation requires the exact verified
`assets/<namespace>/items/<path>.json` definition, `items` content, and
`register_items` permission. A screen may reference an item from the same
bundle. After the resource pack reload, all three adapters build the same local
vanilla `ItemStack`, apply Minecraft 26.1.2's `ITEM_MODEL` and `CUSTOM_NAME`
components, and display it through the standard item widget. This does not
mutate the frozen vanilla item registry. That local item declaration remains a
presentation path; the block-specific server grant is defined below.
Player-driven use of generic Loader item declarations remains outside the
implemented boundary.

The block-presentation slice accepts up to eight owner-namespaced block
declarations with bounded names and owner model ids. Activation requires
`blocks`, `register_blocks`, and the exact verified
`assets/<namespace>/models/<path>.json`. Every platform pre-registers the same
bounded carrier set (`solaris_loader:loader_block` through
`solaris_loader:loader_block_7`) before registry freeze. After the verified
pack mounts, owner block ids are sorted and mapped by index to those blockstate
and item definitions. A Loader screen renders the corresponding custom block
through the standard block item without substituting a vanilla block or
performing late registry mutation. The client resolves every carrier's exact
runtime default-state id and sends the explicit owner-id-to-state
`carrier_block_state_ids` map in its acknowledgement. Solaris independently
reads all owned block ids from the already size/SHA-verified plugin artifacts
and accepts the ACK only when its identities match exactly and its carrier
state ids are usable and distinct. The resulting mapping remains scoped to the
acknowledged connection. Solaris registers every owner identity as a full,
opaque, non-emitting state after the frozen vanilla range in its canonical
server block and light tables before opening the world. Those states persist
by owner name through the normal world format; client runtime ids never enter
storage. Block updates and chunk palettes project each canonical state to that
exact session's acknowledged carrier id.
Projected chunk frames bypass the cross-session prepared-frame cache, so one
client's runtime id cannot leak to another.

The owning host-attested Luau plugin may call
`solaris.place_loader_block(request_id, block_id, x, y, z)` for its exact
verified block identity. The server rejects unattested, foreign-owner, unknown,
and out-of-world requests before mutation, then commits the canonical state
through the existing server-owned block-edit transaction. Only after that owner
outcome does the host publish required targeted `loader.block_placement_result`.
Publication includes every loaded session and applies each recipient's
connection-scoped projection.

The same exact owner may call
`solaris.grant_loader_block_item(request_id, player_id, block_id, count)` for a
player whose current Play session acknowledged that block carrier. Solaris
represents the item canonically as the known vanilla `minecraft:paper` protocol
id plus the verified block name and its deterministic
`minecraft:item_model = solaris_loader:loader_block[_N]` component. The session
owner merges it under the existing inventory transaction gate, persists the
updated canonical inventory before publishing it, rejects a full inventory
without mutation, and returns required targeted `loader.item_grant_result` with
the exact semantic outcome. The vanilla item registry and client runtime ids
remain out of persistence.

For `UseItemOn`, Solaris recognizes only a non-empty `minecraft:paper` stack
with one exact bounded carrier item-model component in the current live session
that acknowledged that owner block. It resolves that specific model to its
canonical server state and then uses the ordinary
survival placement transaction unchanged: the exact expected hand stack,
game mode, loaded target and mutation tokens are revalidated under the player
persistence lock; the conditional world edit commits before one item is
persisted as consumed and before block/inventory publication. A missing ACK,
closed session, wrong base item, wrong model, stale hand, or rejected placement
does not mutate inventory or world state.

Survival breaking resolves a Loader drop only when the broken canonical state
matches one exact live acknowledged session projection. The ordinary loot
planner is replaced for that root state with the same canonical named
`minecraft:paper` plus that owner's carrier item-model stack. The
existing authoritative item entity owns the drop: its component-bearing stack
is published to clients, persisted in the vanilla entity `Item` compound,
preserved across partial claims, and credited by the existing simulation-owner
pickup transaction. Loader blocks do not bypass world-item lifetime, pickup,
or player persistence authority.

## Consequences

- Plugin authors describe one bundle instead of maintaining three server
  schemas.
- Vanilla clients remain compatible with servers whose plugins declare no
  client content.
- A rich-content server fails closed instead of silently substituting vanilla
  blocks or inventory GUIs.
- Cache reuse is content-addressed; display-version reuse cannot substitute
  different bytes.
- Platform code remains necessary for transport and content registration, but
  it cannot weaken the shared trust contract.
