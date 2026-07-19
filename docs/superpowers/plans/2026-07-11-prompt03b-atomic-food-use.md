# Prompt 03B Atomic Food Use Plan

Quality label: `stabilization`.

## Scope

Use simulation ticks for item-use duration. Commit one food item debit and the
resulting hunger/saturation state in one session-fenced simulation-owner turn.
The connection keeps socket ownership and mirrors the returned snapshots only.

Bow release, shield state, generic damage/death, cursor/container state, pose,
and save barriers remain separate Prompt 03B work.

## Tasks

- [x] Add RED tick-duration and owner conservation tests.
- [x] Replace `PendingUse` wall-clock fields with simulation ticks.
- [x] Add a bounded owner command for exact stack plus survival-state commit.
- [x] Cut food completion over to the owner and remove local debit/feed.
- [x] Prove cancellation, stale session/state/stack, requester loss, and exact
  TCP slot/health output without guessed-time waits.
- [x] Run focused gates, replay/soak, embedded MCP, and the full baseline.

## Evidence

- `PendingUse` duration, completion, and bow draw sampling use simulation tick
  deltas. A `watch` notification wakes the connection on owner tick advance;
  the local furnace timer no longer polls item use.
- `CommitFoodUse` validates the active `SessionId`, selected slot, exact held
  stack, and exact survival snapshot, then writes inventory and hunger under
  one owner-held player lock. Requester loss after apply preserves both.
- Focused owner tests cover commit, stale stack/state/session, requester loss,
  and tick wakeup. `mc-net` passes 518 tests and `block_edit` passes 66 tests.
- Checked deterministic replay passed on two fresh Solaris servers. The short
  Prompt 02 4+1 soak passed with five transaction samples and no stuck retry.
- The embedded 26.1.2 client MCP passed
  `playable-11-eat-passive-food` on a new player: earned beef, natural sprint
  exhaustion changed food `20 -> 19`, eating changed food `19 -> 20`, and the
  earned stack changed `1 -> 0`.
- MCP waits are push-driven. A mixin publishes after an inbound packet is
  applied; client tick/login/logout events cover local progress. The smoke and
  production scenario client contain no `Thread.sleep` or retry polling.
- Final gates passed: Gradle `test`, `xtask code-health`, workspace tests,
  workspace clippy with warnings denied, and Rust formatting. One earlier
  workspace run hit the shield test once; the unchanged shield gate then
  passed 8 isolated runs, 5 complete `mob_presence` runs, and the repeated
  full workspace run.

Food-use start/cancel remains connection-local. This slice makes completion
atomic; it does not claim the complete player aggregate or Prompt 03B is done.
