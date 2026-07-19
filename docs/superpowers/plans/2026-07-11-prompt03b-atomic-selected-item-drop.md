# Prompt 03B Atomic Selected Item Drop Plan

Quality label: `stabilization`.

## Scope

Commit the selected-slot debit and spawned item entity in one session-fenced
simulation-owner turn for `DropItem` and `DropAllItems`. The connection keeps
packet validation and socket writes, then mirrors only the returned inventory.

Cursor/container throws, death drops, disconnect cursor settlement, and generic
server-origin item spawns remain separate Prompt 03B work.

## Tasks

- [x] Add RED commit, drop-all, stale-slot/stack/session, duplicate-command, and
  requester-loss tests.
- [x] Add one bounded `CommitSelectedItemDrop` owner command.
- [x] Commit the inventory debit, item entity, pickup delay, and owner pickup
  block under one registry turn; dispatch visibility before responding.
- [x] Cut `DropItem`/`DropAllItems` over to the returned inventory and remove the
  direct production spawn helper.
- [x] Run focused owner/TCP tests, checked replay, short 4+1 soak, the existing
  two-real-client handoff gate when practical, and the full baseline.

## Evidence

- Six owner transaction tests cover one/all debit, stale slot/stack/session,
  duplicate command, and requester loss.
- The exact TCP drop test proves the selected-slot packet path.
- Checked replay and the short 4+1 pressure soak pass.
- An embedded-MCP two-client gate moved a stack from main inventory to hotbar,
  dropped it through the real 26.1.2 client, observed the same item entity from
  both clients, credited only the second player, and observed entity removal.
- Full workspace tests, clippy with warnings denied, formatting, code-health,
  and the full Gradle test suite pass.
