#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

mkdir -p .ai-bridge
cp docs/agents/PRO_CORE_AUDIT_PROMPT.md .ai-bridge/PRO_CORE_AUDIT_PROMPT.md
cp docs/agents/PRO_CORE_AUDIT_RUNBOOK.md .ai-bridge/PRO_CORE_AUDIT_RUNBOOK.md

codexpro pro-bundle \
  --root "$ROOT" \
  --title "Solaris whole-core audit for v0.0.3-alpha.1" \
  --max-files 80 \
  --max-file-bytes 100000 \
  --max-total-bytes 900000

cat <<'EOF'

Pro audit context is ready:
  .ai-bridge/pro-context.md

Canonical planning prompt:
  docs/agents/PRO_CORE_AUDIT_PROMPT.md

Ignored convenience copies are refreshed under .ai-bridge/ before bundling.

Recommended Pro path:
  give the Pro model pro-context.md and docs/agents/PRO_CORE_AUDIT_PROMPT.md.

If that Pro surface can call the already-running CodexPro connector, it may use
read-only inspection to fill gaps from truncated bundle files; the audit prompt
still forbids production edits and implementation execution.

Save the complete response to:
  .ai-bridge/pro-core-audit-response.md

After human review only, install only FIRST HANDOFF:
  python3 tools/apply-pro-first-handoff.py .ai-bridge/pro-core-audit-response.md

Do not run Pro from a stale bundle after source changes; rerun this script first.
EOF
