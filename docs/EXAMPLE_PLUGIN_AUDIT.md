# Shipped Luau plugin audit

Date: 2026-07-29

This audit evaluates the exact files under `examples/plugins/` against API
`0.6.0`, production adapters, and the shipped wire tests. “Complete” means
complete for the plugin's deliberately bounded advertised scope, not a claim of
a general ecosystem-ready implementation.

| Plugin | Status | Current boundary | Runtime API gaps |
| --- | --- | --- | --- |
| `basic-economy` | Complete for scope | Configurable physical-item shop, zone/command entry, durable purchase/refund ledger, and one inventory/storage commit | Transaction rejection is only a boolean, so the plugin cannot distinguish insufficient currency, full inventory, stale storage, or unavailable player. General payments, auctions, and multiple currencies are intentionally absent. |
| `land-claims` | Complete for the single-dimension chunk-claim scope, but not crash-atomic as one policy transaction | Durable claim list plus generic protected zones; owner/operator create/remove flow and wire-proven break/place denial | Player-command snapshots lack dimension. Storage CAS and zone registration are two commits with compensation, so a process crash between them can leave durable intent and installed policy temporarily divergent. Trust lists, transfer, subdivisions, and richer policy predicates need generic policy APIs rather than claim-specific Rust code. |
| `online-roster` | Complete for scope | Fresh bounded online-player query rendered as a server-owned inventory menu | No blocker for the current `/who` menu. Rich actions would need menu metadata/action correlation beyond fixed slot clicks. |
| `colony-villager-scaffold` | Partial | Configurable Luau-owned colony metadata and durable role/order intent over an ephemeral villager token | The first extraction is complete: Rust no longer stores colonies or interprets `home`/`hold`; the plugin maps those orders to bounded movement/idle goals and handles typed binding failures. Roles still have no behavior, one player owns at most one durable member record, and general work execution, villager inventory/memory, and durable entity identity remain absent. |
| `geological-mines` | Thin selector, not a complete plugin | Its manifest selects the Rust-owned `geological_deposits` profile; Luau only prints status | Needs bounded data-driven startup ore descriptors and ownership validation. Per-chunk Luau callbacks are intentionally not proposed because deterministic generation, thread ownership, and throughput remain engine responsibilities. |
| `settlement-prototype` | Partial declarative content package | The manifest chooses a Rust-owned prototype and supplies a bounded building/inhabitant/extension plan | Needs a more open startup template-composition and marker schema. Placement, deterministic generation, template decoding, and resident materialization should remain engine primitives; settlement policy and content selection should be plugin data/Luau logic. |

## Direction

Rust owns authoritative mechanics: validation, bounded queues, persistence
primitives, inventory/world/entity commits, regional ownership, deterministic
world generation, and wire publication. Luau owns domain vocabulary and policy:
economies, claims, colonies, roles, orders, shops, progression, and content
selection.

The first concrete extraction removes the Rust colony registry and hard-coded
colony orders. The engine exposes only an opaque, expiring villager binding and
bounded goal operations. The shipped colony plugin stores its own colony record,
configuration, role/order intent, and maps `home`/`hold` to those primitives.
