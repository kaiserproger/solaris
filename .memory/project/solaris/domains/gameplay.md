# Gameplay Route

Start with `docs/playable/README.md`, then `docs/playable/ACTIVE.md`. The active
queue, not old milestone prose, chooses the next gameplay slice.

Ownership:

- `crates/mc-net/src/play/` owns protocol adaptation, session orchestration,
  publication, containers, survival actions, and simulation commands.
- `crates/mc-world/`, `mc-entity`, `mc-physics`, and `mc-data` own their domain
  facts and mutations; do not copy their rules into `mc-net` helpers.
- `crates/mc-test-harness/tests/` is the wire/integration surface. The client
  MCP route is documented in `docs/AGENT_TOOLING.md`.

Evidence rules:

- Reproduce a reported bug through the same player-visible path, fix the
  authority or adapter that caused it, and rerun the shortest exact scenario.
- A unit test proves its local contract only. It does not prove vanilla parity,
  a real client, multiplayer behavior, performance, or a 20-minute session.
- A fake client may prove orchestration, but protocol/data ids and expected
  facts must come from an independent registry, vanilla oracle, or named
  real-client scenario. Do not copy the implementation's constant into the
  fake and call the agreement parity.
- When the root cause is a shared classifier, data table, or physics path,
  enumerate every supported member of that exact family in an oracle-backed
  table test. Keep real-client proof focused on representative reported paths.
- A timeout may fail a stuck gate but never prove success. Wait for the exact
  packet, owner result, world transition, or simulation event.
- A client-MCP `connect` result proves dispatch only. Await the pushed
  `in_play = true` state before gameplay actions. `playable.toml` intentionally
  denies admin commands; privileged diagnostics require a separate ignored
  config and are diagnostic evidence, not survival or parity evidence.
- A manually interrupted client run remains incomplete evidence even if its
  artifact receives plausible late output. It needs a new completed run.
- Record whether evidence was focused harness, automated client, owner-played,
  vanilla oracle, performance, or concurrency. Do not merge those labels.

Prioritize the common survival loop and multiplayer-visible failures. Defer
rare parity edges while a common blocker or production plugin path is missing.
For client-visible fixes, use a focused one-to-three-minute probe of the
affected phase, then one complete 20-minute gate after the feature is finished.
