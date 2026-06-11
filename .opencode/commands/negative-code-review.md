---
description: Review the current diff for negative-code and LLM slop issues
disable-model-invocation: false
---

Run a read-only negative-code review of the current workspace diff.

Scope:

- Do not edit files.
- Do not commit, push, tag, merge, or stage changes.
- Preserve unrelated dirty worktree changes.
- Findings first, ordered by severity, with file/line references when available.

Review checklist:

- Can any added code/config/docs be deleted while preserving the requested behavior?
- Are there one-use helpers, fake abstractions, generic `utils`/`common`/`shared` additions, or speculative extension points?
- Did permissions, public contracts, validation claims, milestone wording, or docs become broader than the evidence supports?
- Are tests exercising real code paths and external facts rather than model-invented fixtures?
- Are there simpler existing repo patterns that should have been reused?

Suggested flow:

1. Inspect `git status --short` and the relevant diff.
2. If the diff is non-trivial, use the `harness-slop-reviewer` subagent with the checklist above.
3. If the diff is small, perform the checklist directly.
4. Return one verdict: `KEEP`, `CLEANUP_REQUIRED`, or `REJECT`, then list findings.
