# Shared gameplay-value crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

Protocol-independent gameplay value types are owned by `mc-domain` rather than by the packet layer.

`mc-domain` owns:

- `GameMode` and its Java numeric id mapping;
- `Direction`, ordinal decoding, and block-normal vectors;
- `InteractionHand` and ordinal decoding.

`mc-protocol` deliberately re-exports these values at its existing packet API surface so protocol callers remain source-compatible while the canonical type identity stays below transport.

`mc-net` consumes `mc-domain` directly for gameplay/session logic instead of depending on packet definitions for these semantic enums.

## Correctness fences

- `mc-domain` has no dependencies and directly tests Java-compatible ids/ordinals and direction normals.
- `xtask code-health` requires all three canonical enums to remain in `mc-domain` and requires `mc-protocol` to adapt by re-export rather than recreating them.
- `mc-domain` is included in the lower-crate reverse-dependency gate, so it cannot depend on `mc-net`.
- The generic lower-crate transport/session leakage scanner now covers `mc-domain` as well.

Benchmark: not applicable. This is a type-ownership cutover with no runtime algorithm.

## Validation

- `cargo test -p mc-domain`: 1 passed.
- `cargo test -p mc-protocol --quiet`: 321 passed.
- Full workspace was green immediately before this guard/documentation-only checkpoint.

No graphical/client or performance claim is made by this evidence.
