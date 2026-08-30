# Running the Solaris whole-core audit with a Pro planning model

Run this only after currently active implementation agents have reached terminal checkpoints, so the audit sees one coherent tree.

## Recommended — fresh Pro context bundle

`codexpro --mode pro` is the context-export/planning path for model surfaces that do not call MCP tools directly. The reproducible repository entrypoint is:

```bash
cd /home/kaiserroman/solaris
just pro-audit-context
```

That regenerates `.ai-bridge/pro-context.md` from the current tree with a size bounded below this CodexPro profile's 1 MB write cap. The helper also refreshes ignored convenience copies of the canonical versioned prompt/runbook from `docs/agents/`.

Give the Pro model both:

1. `.ai-bridge/pro-context.md`
2. `docs/agents/PRO_CORE_AUDIT_PROMPT.md`

The second file is the exact audit/decomposition prompt. Do not replace it with a short paraphrase.

Save the model's complete response as:

```text
.ai-bridge/pro-core-audit-response.md
```

Do not use an old bundle after source changes; rerun `just pro-audit-context` immediately before the audit.

## If the selected Pro surface can call CodexPro tools

It may use the already-running ordinary CodexPro connector for supplemental read-only inspection of files truncated from the bundle. The prompt still makes the audit planning-only: do not edit production code, commit, push, tag or launch implementation agents during the audit. `codexpro start --mode pro` is not a special tool-enabled connector; its documented purpose is context export for non-tool model surfaces.

## Install the first execution handoff

Review the Pro response first. Keep the complete audit/backlog in its own file and install only the model's exact `FIRST HANDOFF` section into the normal execution handoff:

```bash
python3 tools/apply-pro-first-handoff.py \
  .ai-bridge/pro-core-audit-response.md
```

The helper extracts `FIRST HANDOFF` up to `DEFERRED / DO NOT DO` and passes only that finite wave through `codexpro pro-apply`. Do not put the complete multi-wave audit into `current-plan.md`.

Future Pi execution should use the CodexPro one-call lifecycle:

```text
spawn_pi_detached      # Sol by default, implementation/planning
spawn_pi_qa_detached   # Luna by default, mandatory graphical Xvfb QA
wait_pi_agent          # event-driven state/result/receipt, no tmux/ps/tail plumbing
```

## Audit output retention

Keep the full Pro audit separately even after installing the first handoff:

```text
.ai-bridge/pro-core-audit-response.md
```

Only the first execution wave belongs in `.ai-bridge/current-plan.md`. Promote later waves as earlier waves close; this keeps agents from trying to execute the entire audit in one session.
