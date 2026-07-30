# `/goal` Wrapper V2

The persistent objective should be stored once, outside the per-turn user
message. The runtime injects a compact reference plus one finite checkpoint.
Volatile counters are placed last so the stable prefix remains cacheable.

## CodexPro Launch Contract

Before starting `/goal`, make the active route document name one complete
active-plan item or one observable vertical capability and its acceptance
evidence. Do not leave an individual function, test, file move, or documentation
update in `resume.next` or the active queue. If the active cursor is mechanical,
fix the cursor before starting CodexPro.

For the current public-alpha campaign, start CodexPro with:

```text
/goal
Use the explicit checkpoint route and the current active route document.
Complete one feature-sized outcome: one full active-plan item or one observable
gameplay, multiplayer, plugin, persistence, performance, or tooling capability.

Treat test extraction, file movement, refactoring, evidence writing, and other
mechanical work as internal edits of that checkpoint. Do not close or advance
the checkpoint for any one of them. Run L2, one independent review, update all
owning documentation, and create one local commit only after the complete
outcome is satisfied.

If the hard continuation budget is reached first, close as partial with the same
unfinished feature-sized outcome in resume.next. Do not replace it with a
smaller mechanical checkpoint.
```

This launch prompt selects granularity, not route. `checkpoint.route` remains
the only routing authority. The CodexPro wrapper must retain the same unfinished
outcome across fresh continuations until `done_when` is satisfied or a concrete
blocker is recorded. `resume.next` names the next action inside that outcome,
not a new checkpoint.

To authorize parallel execution, prefix the launch with
`quaka-whaka-zaka-du`; this permits at most two disjoint workers under
`AGENTS.md` and does not make the checkpoint smaller.

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
id: PLAY-P1-03
route: playable
outcome: Close Phase 1 item 3 of docs/PUBLIC_ALPHA_PLAN.md as one
  feature-sized test-ownership checkpoint.

done_when:
  - All substantial remaining aggregate or inline production tests covered by
    the plan item live beside their focused domains.
  - The exact moved test set and before/after concentration are recorded once.
  - No new substantial aggregate or inline production test is introduced.
  - One clean L2 run and one independent review pass on the complete tree.
  - Owning plan and active-route documentation are updated.
  - The whole checkpoint is recorded in one local commit.

scope:
  include:
    - remaining aggregate test roots named by Phase 1 item 3
    - their focused sibling test modules
    - consolidated evidence and owning status documents
  exclude:
    - production gameplay changes
    - unrelated test cleanup
    - one-test or one-file checkpoint closes

validation: L2
primary_context: docs/playable/ACTIVE.md

resume:
  base_tree: <git-sha>
  changed_files: []
  evidence: []
  next: Inventory the remaining aggregate domain classes once, then apply the
    complete extraction batch for this plan item.

rules:
  - Use route above; never route from words in the persistent objective.
  - Use current tree and resume cursor; do not restart a full repo survey.
  - Batch independent tools and bound every output.
  - Do not repeat a command that is already green on the same tree hash.
  - Do not close for an individual test, function, file, evidence note, or
    mechanical edit.
  - Run L2, review, documentation close, and commit once for the complete
    outcome.
  - At hard budget, return partial and preserve this same unfinished outcome
    for the fresh continuation.

runtime_budget:
  model_roundtrips_remaining: 80
  shell_batches_remaining: 24
  subagents_remaining: 1
  l2_validation_runs_remaining: 1
</goal_checkpoint>
```

`runtime_budget` is deliberately last. Better still, keep it in tool/runtime
metadata rather than natural-language prompt content.

## 3. Checkpoint Close Transition - Inject Only On Close Candidate

```md
<goal_transition type="checkpoint_close" checkpoint="PLAY-P1-03">
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
