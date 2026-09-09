# Playable acceptance

## Outcome

Keep the normal Minecraft Java 26.1.2 client stable through:

```text
join -> move -> gather -> craft -> build -> fight/farm -> save/rejoin
```

Common gameplay, multiplayer authority and ordinary save integrity take priority
over uncommon parity edges. The current owner request and source cursor live
only in [MEMORY.md](../MEMORY.md); the replacement design is
[ARCHITECTURE.md](../ARCHITECTURE.md). This file owns the playable acceptance
scenario, not a second implementation queue or chronological work log.

## Gate

Use the documented debug `playable.toml` profile and a real graphical 26.1.2
client through the [QA protocol](../QA_AGENT_PROTOCOL.md). Focused probes locate
failures; a complete 20-minute survival loop closes the playable gate. A client
connect dispatch is not evidence of entering Play.

Verify movement without false correction, interactions and item conservation,
container/crafting publication, combat/death/respawn, disconnect/reconnect and
save/restart. Multiplayer changes include another real client observing the
same committed outcome. Privileged diagnostics and raw-TCP tests supplement,
but never replace, the normal client scenario.

Record the exact tree/config/scenario, agent-run versus owner-run execution,
client and server evidence, and unresolved failures. A passing scoped scenario
is not full vanilla parity, a performance result or release readiness.

## Current evidence boundary

- Prior graphical smoke: `.analysis/real-client-runs/20260905T022740Z-real-client-playable-loop-4rRzBN`.
  It predates the new core rewrite and is not a fresh acceptance run.
- Current automated startup/persistence component checks pass; their exact scope
  and receipts are in [MEMORY.md](../MEMORY.md). They use real server processes
  and protocol clients, not graphical-client acceptance.
- The previous frozen load matrix has 20 passing and 22 failing rows; eleven
  other workspace failures remain unresolved. Details and identities are in
  [MEMORY.md](../MEMORY.md). Do not relabel those gates through documentation.
- Owner terrain `ACCEPT` is still absent. The alpha acceptance contract remains
  [PUBLIC_ALPHA3_PLAN.md](../PUBLIC_ALPHA3_PLAN.md); no alpha closure is claimed.

Historical evidence remains in `docs/evidence/` and version history. Superseded
memory archives and execution prompts are not current routing authorities.
