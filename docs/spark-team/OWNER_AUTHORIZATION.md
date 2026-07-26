# Owner authorization — autonomous Spark campaign

For this campaign the owner explicitly authorizes `quaka-whaka-zaka-du` parallel work under the repository cap.

The primary Codex thread may, without requesting another “continue” message:

- self-select the next finite checkpoint from `manifest.json` and the current task-card statuses;
- treat the selected card's `route` as `checkpoint.route` under `AGENTS.md`;
- create local `agent/spark-*` branches and isolated worktrees;
- spawn at most two `gpt-5.3-codex-spark` subagents with disjoint write sets;
- make local Conventional Commits, review them, and cherry-pick them into a dedicated campaign branch;
- update task cards, checkboxes, evidence indexes, ignored run artifacts, and campaign state;
- continue through successive batches until the campaign completion contract is proven.

This authorization does **not** permit pushing, merging to `main`, tagging, rewriting history, resetting/cleaning owner files, committing secrets/Mojang bytes/local artifacts, bypassing validation, or claiming unrun evidence.
