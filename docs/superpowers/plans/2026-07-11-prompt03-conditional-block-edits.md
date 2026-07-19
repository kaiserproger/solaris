# Prompt 03 Conditional Block Edit Plan

Quality label: `stabilization`.

## Scope

Move only Prompt 02's mutation-token-protected survival break and placement
commit through ADR 0004's bounded owner. Keep creative, unconditional
interaction, scheduled/random/fluid, falling-block, block-entity, container,
and persistence mutations explicitly legacy.

## Tasks

- [x] Factor conditional block application into a synchronous storage-level
  operation shared by the legacy test path and owner path.
- [x] Add typed `ApplyBlockEdits` command/outcome with state and mutation-token
  preconditions.
- [x] Use non-blocking world acquisition in the owner; expose typed busy and
  unavailable outcomes plus runtime counters.
- [x] Preserve inventory/tool state and authoritative resync when enqueue,
  world acquisition, or precondition validation fails.
- [x] Add direct-vs-owner, duplicate-token, and busy-world no-mutation tests.
- [x] Pass all 462 `mc-net` library tests, stale/ABA break, concurrent
  same-target placement, and all 66 `block_edit` tests.
- [x] Rerun checked Prompt 02 replay and the short 4+1 soak.
- [x] Run the full Cargo/format/code-health checkpoint after documentation.
- [ ] Run P4/P42 only after the remaining Prompt 03 authority slices close.
