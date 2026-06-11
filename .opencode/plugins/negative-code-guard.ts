import type { Plugin } from "@opencode-ai/plugin"

const POLICY = `Solaris Negative Code Review Gate

Before finalizing non-trivial edits in this repository, run a negative-code review. Prefer harness-slop-reviewer or /negative-code-review; self-review is acceptable for very small diffs. Final reports must state whether the gate ran and what it found. See AGENTS.md for the full policy.`

const COMPACTION_CONTEXT =
  "Solaris negative-code gate remains active after compaction: before finalizing non-trivial edits, review the diff for deletion opportunities, fake abstractions, one-use helpers, over-broad config, unrelated dirty worktree changes, and unsupported validation claims."

export default (async () => {
  return {
    "experimental.chat.system.transform": async (_input, output) => {
      if (!output.system.some((entry) => entry.includes("Solaris Negative Code Review Gate"))) {
        output.system.push(POLICY)
      }
    },
    "experimental.session.compacting": async (_input, output) => {
      if (!output.context.includes(COMPACTION_CONTEXT)) {
        output.context.push(COMPACTION_CONTEXT)
      }
    },
  }
}) satisfies Plugin
