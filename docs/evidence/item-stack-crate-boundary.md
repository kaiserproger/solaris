# Item-stack crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

The canonical gameplay `ItemStack` value lives in `mc-data::item_stack`, not in the packet layer.

`mc-data` owns stack data and protocol-neutral helpers: empty-stack identity, count/item id, damage, enchantments, custom name, item model, and deterministic mutation builders.

`mc-protocol` deliberately re-exports `mc_data::item_stack::ItemStack` at its existing Play API surface and remains responsible only for wire encoding/decoding of that value.

## Correctness fences

- `mc-data` directly tests empty-stack behavior, damage normalization, enchantment replacement/sorting, custom names, and item models.
- `xtask code-health` requires the canonical struct/empty/is-empty contract to remain in `mc-data` and requires `mc-protocol` to adapt by re-export rather than defining a competing stack type.
- Existing lower-crate reverse-dependency and transport/session leakage guards apply.

Benchmark: not applicable. This is a value-type ownership boundary with no runtime algorithm change.

## Validation

- `cargo test -p mc-data item_stack`: 1 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test -p mc-protocol --quiet`: 321 passed on the immediately preceding shared-value checkpoint.

No graphical/client or performance claim is made by this evidence.
