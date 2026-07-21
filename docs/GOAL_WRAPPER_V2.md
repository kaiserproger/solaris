# `/goal` Wrapper V2

The persistent objective should be stored once, outside the per-turn user
message. The runtime injects a compact reference plus one finite checkpoint.
Volatile counters are placed last so the stable prefix remains cacheable.

## 1. Session-Level North Star - Inject Once

```md
<goal_north_star id="solaris-v1">
The owner wants a genuinely playable Minecraft server first, then multiplayer
quality, a usable plugin ecosystem, progressive vanilla parity, and scaling
from weak machines to server-class CPUs.

This is a persistent direction, not a single-turn completion contract.
Checkpoint completion must advance at least one success dimension without
redefining north-star completion.
</goal_north_star>
```

The full original owner wording can remain in durable goal storage. It does not
need to be repeated in every model turn.

## 2. Active Continuation Wrapper - Inject Every Checkpoint

```md
<goal_checkpoint version="2" north_star_ref="solaris-v1">
id: PLAY-042
route: playable
outcome: A real 26.1 client can complete one bounded survival action that is
  currently missing, with server-authoritative behavior and focused evidence.

done_when:
  - The named client-visible behavior works on the authoritative path.
  - Focused regression covers success and the dominant failure boundary.
  - L1 validation is green for the affected scope.

scope:
  include:
    - crates/mc-net/src/play/...
    - one focused test module
  exclude:
    - plugin API
    - broad vanilla parity cleanup
    - unrelated refactors

validation: L1
primary_context: docs/playable/ACTIVE.md

resume:
  base_tree: <git-sha>
  changed_files: []
  evidence: []
  next: Inspect the named packet-to-world mutation path once, then implement.

rules:
  - Use route above; never route from words in the persistent objective.
  - Use current tree and resume cursor; do not restart a full repo survey.
  - Batch independent tools and bound every output.
  - Do not repeat a command that is already green on the same tree hash.
  - Close this checkpoint at done_when or hard budget; a fresh continuation
    handles the next checkpoint.

runtime_budget:
  model_roundtrips_remaining: 80
  shell_batches_remaining: 24
  subagents_remaining: 2
  l2_validation_runs_remaining: 1
</goal_checkpoint>
```

`runtime_budget` is deliberately last. Better still, keep it in tool/runtime
metadata rather than natural-language prompt content.

## 3. Checkpoint Close Transition - Inject Only On Close Candidate

```md
<goal_transition type="checkpoint_close" checkpoint="PLAY-042">
Audit only this checkpoint's `done_when` against current authoritative evidence.
Return:

status: complete | partial | checkpoint-blocked
evidence:
  - requirement: ...
    proof: command/file/runtime evidence
remaining: []
resume:
  base_tree: ...
  diff_hash: ...
  changed_files: [...]
  next: ...

Do not audit or claim completion of the persistent north star here.
</goal_transition>
```

## 4. North-Star Completion Audit - Rare, Separate Transition

The current completion audit belongs only here, after runtime or owner
explicitly requests `north_star_complete_candidate`. It should never be
injected during ordinary active work.

## 5. Blocked Handling

Track repeated blockers in runtime state:

```yaml
blocker:
  fingerprint: <stable hash>
  consecutive_checkpoints: 2
  last_evidence: ...
```

Only inject a short blocked-transition prompt when the configured threshold is
reached. Repeating blocked policy in every active turn is unnecessary.

## 6. Runtime Work Required

Repository instructions cannot implement these mechanisms:

- command/subagent completion events without polling;
- batch executor for independent calls;
- validation cache keyed by command/tree/environment/scope;
- hard checkpoint counters and automatic fresh continuation;
- compact subagent results deduplicated by agent/revision;
- full stdout persisted outside model context with structured summaries.
