#!/usr/bin/env python3
"""Machine control plane for the autonomous Solaris Spark campaign.

The coordinator reads compact JSON from this script instead of loading the full
board.  It can bootstrap an isolated campaign worktree, select up to two
compatible cards, create task worktrees and dispatch packets, verify candidate
branches, record an independent review, and squash-integrate accepted work.

Only Python's standard library and Git are required.
"""
from __future__ import annotations

import argparse
import contextlib
import datetime as dt
try:
    import fcntl  # type: ignore[import-not-found]
except ImportError:  # native Windows
    fcntl = None

try:
    import msvcrt  # type: ignore[import-not-found]
except ImportError:  # POSIX
    msvcrt = None
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable

PLAN_REL = Path("docs/spark-team")
STATUS_RE = re.compile(r"^Status: `([A-Z-]+)`$", re.MULTILINE)
PLACEHOLDER_RE = re.compile(r"<[^>]+>|^(?:exact |focused |task cards|.*\+ evidence artifacts)", re.I)
PRIORITY_RANK = {"P0": 0, "P1": 1, "P2": 2, "P3": 3}
ACTIVE_STATES = {"claimed", "candidate", "review-changes", "review-pass", "blocked"}
PRE_REVIEW_CHECKS = ("CLAIMED", "BASELINE / RED", "IMPLEMENTED", "TESTING", "SELF-REVIEW")
DISPATCH_CHECKS = (
    "All dependencies are `DONE` on the integrated tree.",
    "Coordinator confirmed no active write-lock or runtime-lease conflict.",
    "Exact base SHA, worktree, port range and run directory are assigned.",
    "Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.",
)
EVIDENCE_KINDS = {
    "audit", "validation", "docs", "docs-and-config", "campaign", "release-gate",
    "evidence-or-fix", "performance-run", "integration-gate", "external-gate",
}
IGNORED_OWNER_PREFIXES = ("docs/spark-team/", ".codex/", ".analysis/")


class AutopilotError(RuntimeError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()


def run(cmd: list[str], *, cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        cmd,
        cwd=str(cwd) if cwd else None,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if check and proc.returncode != 0:
        raise AutopilotError(
            f"command failed ({proc.returncode}): {' '.join(cmd)}\n"
            f"stdout: {(proc.stdout or '')[-2000:]}\n"
            f"stderr: {(proc.stderr or '')[-2000:]}"
        )
    return proc


def git(cwd: Path, *args: str, check: bool = True) -> str:
    return (run(["git", *args], cwd=cwd, check=check).stdout or "").strip()


def git_raw(cwd: Path, *args: str, check: bool = True) -> str:
    """Return Git output without destroying leading porcelain status columns."""
    return (run(["git", *args], cwd=cwd, check=check).stdout or "").rstrip("\n")


def repo_root(path: Path | None = None) -> Path:
    base = (path or Path.cwd()).resolve()
    try:
        return Path(git(base, "rev-parse", "--show-toplevel")).resolve()
    except AutopilotError as exc:
        raise AutopilotError(f"not inside a Git repository: {base}") from exc


def common_git_dir(root: Path) -> Path:
    proc = run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"], cwd=root, check=False)
    if proc.returncode == 0 and (proc.stdout or "").strip():
        return Path(proc.stdout.strip()).resolve()
    raw = Path(git(root, "rev-parse", "--git-common-dir"))
    return raw.resolve() if raw.is_absolute() else (root / raw).resolve()


def default_state_path(root: Path) -> Path:
    override = os.environ.get("SPARK_AUTOPILOT_STATE")
    if override:
        return Path(override).expanduser().resolve()
    return common_git_dir(root) / "spark-autopilot" / "state.json"


def acquire_file_lock(handle: Any) -> None:
    if fcntl is not None:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        return
    if msvcrt is not None:
        handle.seek(0, os.SEEK_END)
        if handle.tell() == 0:
            handle.write("\0")
            handle.flush()
        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_LOCK, 1)
        return
    raise AutopilotError("this Python build has neither fcntl nor msvcrt file locking")


def release_file_lock(handle: Any) -> None:
    if fcntl is not None:
        fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        return
    if msvcrt is not None:
        handle.seek(0)
        msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)


@contextlib.contextmanager
def state_lock(root: Path, *, create: bool = False):
    path = default_state_path(root)
    path.parent.mkdir(parents=True, exist_ok=True)
    lock_path = path.with_suffix(".lock")
    with lock_path.open("a+", encoding="utf-8") as lock:
        acquire_file_lock(lock)
        if path.exists():
            state = json.loads(path.read_text(encoding="utf-8"))
        elif create:
            state = None
        else:
            raise AutopilotError("autopilot state is missing; run `autopilot.py bootstrap` or `doctor --init-state`")
        yield state, path
        if state is not None:
            tmp = path.with_suffix(".tmp")
            tmp.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            os.replace(tmp, path)
        release_file_lock(lock)


def plan_root(root: Path) -> Path:
    return root / PLAN_REL


def load_manifest(root: Path) -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    path = plan_root(root) / "manifest.json"
    if not path.exists():
        raise AutopilotError(f"missing manifest: {path}")
    data = json.loads(path.read_text(encoding="utf-8"))
    tasks = {task["id"]: task for task in data.get("tasks", [])}
    return data, tasks


def card_path(root: Path, task_id: str) -> Path:
    return plan_root(root) / "tasks" / f"{task_id}.md"


def card_status(root: Path, task_id: str) -> str:
    path = card_path(root, task_id)
    if not path.exists():
        return "MISSING"
    match = STATUS_RE.search(path.read_text(encoding="utf-8"))
    return match.group(1) if match else "INVALID"


def normalized(value: str) -> str:
    return value.replace("\\", "/").strip().strip("/")


def overlaps(left: str, right: str) -> bool:
    a, b = normalized(left), normalized(right)
    return bool(a and b and (a == b or a.startswith(b + "/") or b.startswith(a + "/")))


def rust_locked(task: dict[str, Any]) -> bool:
    return any(lock.startswith("RUST-") for lock in task.get("locks", []))


def conflicts(a: dict[str, Any], b: dict[str, Any]) -> list[str]:
    la, lb = set(a.get("locks", [])), set(b.get("locks", []))
    reasons: list[str] = []
    if la & lb:
        reasons.append("shared locks=" + ",".join(sorted(la & lb)))
    if "VALIDATION" in la or "VALIDATION" in lb:
        reasons.append("VALIDATION singleton")
    if "PERF" in la or "PERF" in lb:
        reasons.append("PERF singleton")
    if "RUST-NET-ROOT" in la and rust_locked(b):
        reasons.append("RUST-NET-ROOT vs Rust runtime")
    if "RUST-NET-ROOT" in lb and rust_locked(a):
        reasons.append("RUST-NET-ROOT vs Rust runtime")
    leases_a, leases_b = set(a.get("leases", [])), set(b.get("leases", []))
    if leases_a & leases_b:
        reasons.append("shared leases=" + ",".join(sorted(leases_a & leases_b)))
    if {"CLEAN-HOST", "TREE-FROZEN"} & leases_a:
        reasons.append("first task has exclusive host/tree lease")
    if {"CLEAN-HOST", "TREE-FROZEN"} & leases_b:
        reasons.append("second task has exclusive host/tree lease")
    if a.get("dispatch") == "coordinator-only" or b.get("dispatch") == "coordinator-only":
        reasons.append("coordinator-only singleton")
    for left in a.get("write", []):
        for right in b.get("write", []):
            if PLACEHOLDER_RE.search(left) or PLACEHOLDER_RE.search(right):
                continue
            if overlaps(left, right):
                reasons.append(f"overlapping writes={left}|{right}")
    return reasons


def owner_dirty_paths(root: Path) -> list[str]:
    output = git_raw(root, "status", "--porcelain=v1", "--untracked-files=all")
    result: list[str] = []
    for line in output.splitlines():
        if len(line) < 4:
            continue
        value = line[3:].split(" -> ")[-1].strip('"')
        if value == "AGENTS.md" or value.startswith(IGNORED_OWNER_PREFIXES):
            continue
        result.append(value)
    return sorted(set(result))


def owner_conflicts(task: dict[str, Any], dirty: Iterable[str]) -> list[str]:
    if task["id"] == "T00-01":
        return []
    hits: list[str] = []
    for write in task.get("write", []):
        if PLACEHOLDER_RE.search(write):
            continue
        for path in dirty:
            if overlaps(write, path):
                hits.append(path)
    return sorted(set(hits))


def validate_board(root: Path) -> str:
    script = plan_root(root) / "scripts" / "board.py"
    proc = run([sys.executable, str(script), "validate"], cwd=root, check=False)
    if proc.returncode != 0:
        raise AutopilotError((proc.stdout or "") + "\n" + (proc.stderr or ""))
    return (proc.stdout or "").strip()


def campaign_root_from_state(state: dict[str, Any], fallback: Path) -> Path:
    value = state.get("campaign_root")
    return Path(value).resolve() if value else fallback.resolve()


def refresh_claims(state: dict[str, Any], campaign_root: Path) -> None:
    """Release claims already integrated as DONE/BLOCKED in the campaign tree."""
    for task_id, info in state.get("tasks", {}).items():
        status = card_status(campaign_root, task_id)
        if status == "DONE" and info.get("state") != "integrated":
            info["state"] = "integrated"
            info["integrated_at"] = utc_now()
        elif status == "BLOCKED" and info.get("state") in ACTIVE_STATES:
            info["state"] = "blocked-integrated"


def task_rank(task: dict[str, Any]) -> tuple[int, str, str]:
    return (
        PRIORITY_RANK.get(task.get("priority", "P9"), 9),
        task.get("wave", "W99"),
        task["id"],
    )


def active_action(state: dict[str, Any], root: Path, task_id: str) -> dict[str, Any]:
    info = state.get("tasks", {}).get(task_id, {})
    current_state = info.get("state", "unknown")
    action = {
        "claimed": "run-worker",
        "review-changes": "repair-worker",
        "candidate": "run-reviewer",
        "review-pass": "integrate",
        "blocked": "checkpoint",
    }.get(current_state, "inspect")
    packet = info.get("packet")
    reviewer_packet = info.get("reviewer_packet") or str(
        common_git_dir(root) / "spark-autopilot" / "packets" / f"{task_id}-review.md"
    )
    return {
        "task_id": task_id,
        "state": current_state,
        "action": action,
        "role": "solaris_reviewer" if action == "run-reviewer" else info.get("role"),
        "packet": reviewer_packet if action == "run-reviewer" else packet,
        "worktree": info.get("worktree"),
        "branch": info.get("branch"),
        "base_sha": info.get("base_sha"),
        "review_findings": info.get("review_summary") if action == "repair-worker" else None,
        "next_command": (
            f"integrate --task {task_id}" if action == "integrate"
            else f"checkpoint --task {task_id}" if action == "checkpoint"
            else f"candidate --task {task_id}" if action in {"run-worker", "repair-worker"}
            else f"review --task {task_id} --verdict <pass|changes|blocked>"
            if action == "run-reviewer"
            else f"candidate --task {task_id}"
        ),
    }


def ready_snapshot(state: dict[str, Any], root: Path) -> dict[str, Any]:
    _, tasks = load_manifest(root)
    statuses = {task_id: card_status(root, task_id) for task_id in tasks}
    refresh_claims(state, root)
    active_ids = [
        task_id for task_id, info in state.get("tasks", {}).items() if info.get("state") in ACTIVE_STATES
    ]
    active_tasks = [tasks[task_id] for task_id in active_ids if task_id in tasks]
    dirty = state.get("owner_dirty_paths", [])
    candidates: list[dict[str, Any]] = []
    dirty_blocked: dict[str, list[str]] = {}

    def available(task_id: str, task: dict[str, Any]) -> bool:
        if statuses[task_id] != "QUEUED":
            return False
        if not all(statuses.get(dep) == "DONE" for dep in task.get("depends", [])):
            return False
        if any(conflicts(task, active) for active in active_tasks):
            return False
        hits = owner_conflicts(task, dirty)
        if hits:
            dirty_blocked[task_id] = hits
            return False
        return True

    for task_id, task in tasks.items():
        if task.get("dispatch") == "ready" and available(task_id, task):
            candidates.append(task)

    candidates.sort(key=task_rank)
    pairs: list[list[str]] = []
    for i, first in enumerate(candidates):
        for second in candidates[i + 1 :]:
            if not conflicts(first, second):
                pairs.append([first["id"], second["id"]])

    templates = [
        task_id
        for task_id, task in tasks.items()
        if statuses[task_id] == "QUEUED"
        and task.get("dispatch") == "template"
        and all(statuses.get(dep) == "DONE" for dep in task.get("materialize_after", []))
        and not any(conflicts(task, active) for active in active_tasks)
        and not owner_conflicts(task, dirty)
    ]
    coordinator = [
        task_id
        for task_id, task in tasks.items()
        if task.get("dispatch") == "coordinator-only" and available(task_id, task)
    ]
    coordinator.sort(key=lambda task_id: task_rank(tasks[task_id]))
    templates.sort(key=lambda task_id: task_rank(tasks[task_id]))
    choices: list[tuple[tuple[int, str, str], str, str]] = []
    if candidates:
        choices.append((task_rank(candidates[0]), "ready", candidates[0]["id"]))
    if coordinator:
        choices.append((task_rank(tasks[coordinator[0]]), "coordinator", coordinator[0]))
    if templates:
        choices.append((task_rank(tasks[templates[0]]), "template", templates[0]))
    recommended = None
    if choices:
        _, kind, task_id = min(choices, key=lambda item: item[0])
        recommended = {"kind": kind, "task_id": task_id}

    counts: dict[str, int] = defaultdict(int)
    for status in statuses.values():
        counts[status] += 1
    return {
        "campaign_root": str(root),
        "campaign_branch": git(root, "branch", "--show-current"),
        "campaign_head": git(root, "rev-parse", "HEAD"),
        "counts": dict(sorted(counts.items())),
        "active": sorted(active_ids),
        "active_actions": [active_action(state, root, task_id) for task_id in sorted(active_ids)],
        "ready": [task["id"] for task in candidates],
        "compatible_pairs": pairs[:20],
        "templates_ready_for_materialization": templates,
        "coordinator_ready": coordinator,
        "recommended_action": recommended,
        "blocked": sorted(task_id for task_id, status in statuses.items() if status == "BLOCKED"),
        "completion_gate_done": statuses.get("T10-04") == "DONE",
        "blocked_by_owner_dirty_paths": dirty_blocked,
        "owner_dirty_paths": dirty,
    }

def role_for(task: dict[str, Any]) -> str:
    if task.get("dispatch") == "coordinator-only":
        return "primary-coordinator"
    if task.get("kind") in EVIDENCE_KINDS:
        return "solaris_evidence"
    return "solaris_worker"

def set_table_field(text: str, field: str, value: str) -> str:
    pattern = re.compile(rf"^\| {re.escape(field)} \| .* \|$", re.MULTILINE)
    if not pattern.search(text):
        raise AutopilotError(f"task card is missing field: {field}")
    return pattern.sub(f"| {field} | `{value}` |", text, count=1)


def check_box(text: str, label: str) -> str:
    pattern = re.compile(rf"^- \[ \] ({re.escape(label)}(?:\s|—|$).*)$", re.MULTILINE | re.IGNORECASE)
    return pattern.sub(r"- [x] \1", text, count=1)


def is_checked(text: str, label: str) -> bool:
    return bool(re.search(rf"^- \[x\] {re.escape(label)}(?:\s|—|$)", text, re.MULTILINE | re.IGNORECASE))


def markdown_section(text: str, heading: str) -> str:
    match = re.search(rf"^{re.escape(heading)}\n(?P<body>.*?)(?=^## |\Z)", text, re.MULTILINE | re.DOTALL)
    return match.group("body") if match else ""


def unchecked_boxes(text: str, heading: str) -> list[str]:
    body = markdown_section(text, heading)
    return [label.strip() for mark, label in re.findall(r"^- \[([ xX])\] (.+)$", body, re.MULTILINE) if mark.lower() != "x"]


def closeout_scalar(text: str, key: str) -> str | None:
    body = markdown_section(text, "## Closeout")
    matches = re.findall(rf"^{re.escape(key)}:\s*(.*?)\s*$", body, re.MULTILINE)
    return matches[-1] if matches else None


def closeout_list_is_empty(text: str, key: str) -> bool:
    value = closeout_scalar(text, key)
    return value is None or value.strip() in {"[]", "", "null", "UNSET"}


def replace_markdown_section(text: str, heading: str, body: str) -> str:
    pattern = re.compile(rf"^{re.escape(heading)}\n.*?(?=^## |\Z)", re.MULTILINE | re.DOTALL)
    replacement = heading + "\n\n" + body.strip() + "\n\n"
    updated, count = pattern.subn(replacement, text, count=1)
    if count != 1:
        raise AutopilotError(f"missing Markdown section: {heading}")
    return updated


def validation_seed(task: dict[str, Any], payload: dict[str, Any]) -> str:
    lines = [
        f"- Dispatch packet: `{payload['packet']}`",
        f"- Evidence legs: `{', '.join(task.get('evidence_legs', [])) or 'NONE'}`",
        f"- Assigned artifact root: `{payload['run_dir']}`",
        "- Before implementation, replace/extend this seed with exact current-tree commands and artifact paths, then check the fourth Dispatch gate item.",
        "- Card-required validation:",
    ]
    lines.extend(f"  - `{item}`" for item in task.get("validation", []))
    return "\n".join(lines)


def insert_dispatch(text: str, payload: dict[str, Any]) -> str:
    block = f"""
## Autonomous dispatch

- Packet: `{payload['packet']}`
- Task: `{payload['task_id']}`
- Agent role: `{payload['role']}`
- Worktree: `{payload['worktree']}`
- Branch: `{payload['branch']}`
- Base SHA: `{payload['base_sha']}`
- Slot / ports: `{payload['slot']}` / `{payload['ports']}`
- Run directory: `{payload['run_dir']}`
- Owner checkout (read-only): `{payload['owner_checkout']}`

"""
    marker = "\n## Validation log\n"
    if marker not in text:
        raise AutopilotError("task card is missing Validation log")
    if "## Autonomous dispatch" in text:
        return re.sub(r"\n## Autonomous dispatch\n.*?(?=\n## )", "\n" + block.strip() + "\n", text, count=1, flags=re.DOTALL)
    return text.replace(marker, "\n" + block + "## Validation log\n", 1)


def task_packet_text(task: dict[str, Any], payload: dict[str, Any]) -> str:
    reads = "\n".join(f"- `{path}`" for path in task.get("read", [])) or "- none"
    writes = "\n".join(f"- `{path}`" for path in task.get("write", [])) or "- none"
    validation = "\n".join(f"- `{item}`" for item in task.get("validation", [])) or "- card-defined"
    checkpoint = f"""<goal_checkpoint>
id: {task['id']}
route: {task.get('route', 'architecture')}
base_tree: {payload['base_sha']}
owned_changed_files: []
outcome: {task['outcome']}
resume.next: complete exactly {task['id']} from its task card, then return one compact closeout
</goal_checkpoint>"""
    return f"""# Solaris autonomous packet — {task['id']}

Execution role: `{payload['role']}`. Complete exactly this card; do not plan the campaign. If the role is `primary-coordinator`, the primary thread executes this singleton itself and may delegate only bounded read-only evidence slices.

{checkpoint}

## Coordinates

- Worktree: `{payload['worktree']}`
- Branch: `{payload['branch']}`
- Base SHA: `{payload['base_sha']}`
- Card: `{payload['worktree']}/{PLAN_REL}/tasks/{task['id']}.md`
- Owner checkout: `{payload['owner_checkout']}` (read-only)
- Slot / ports / run dir: `{payload['slot']}` / `{payload['ports']}` / `{payload['run_dir']}`
- Locks: `{', '.join(task.get('locks', [])) or 'NONE'}`
- Leases: `{', '.join(task.get('leases', [])) or 'NONE'}`

## Outcome

{task['outcome']}

## Read only

{reads}

## Owned writes

{writes}
- `{PLAN_REL}/tasks/{task['id']}.md` for status, evidence, and closeout.

## Required validation

{validation}

## Hard contract

1. Every shell command starts from the absolute worktree above. First verify `pwd`, Git root, branch, and base ancestry.
2. Rely on the automatically loaded root `AGENTS.md`; do not reopen it. Read only the task card and declared paths. Never read `ALL_IN_ONE.md`, full `BOARD.md`, parent history, whole roadmap, old sessions, or broad source trees.
3. Files over 400 lines: one `rg -n` anchor batch, at most three windows of at most 160 lines.
4. Put exact baseline/RED commands and artifacts into the card before editing. Update status/checks through IMPLEMENTING and TESTING.
5. One observable gap, one bounded edit batch, focused validation, self-review. No unrelated cleanup, speculative abstraction, sleeps, polling, or timeout-as-success.
6. Success: set card to `REVIEW`, check CLAIMED through SELF-REVIEW, fill Validation log and Closeout, make one coherent local commit. Do not check INDEPENDENT REVIEW or DONE.
7. Hard blocker: set `BLOCKED`, record one fingerprint/proof/unlock command, commit useful scoped evidence, and stop without widening scope.
8. Return compact YAML only (≤1000 chars), with commit, changed files, validation, report path, gaps, and one next action.
"""


def ensure_clean(root: Path, *, allow_analysis: bool = True) -> None:
    output = git_raw(root, "status", "--porcelain=v1", "--untracked-files=all")
    bad: list[str] = []
    for line in output.splitlines():
        path = line[3:].split(" -> ")[-1].strip('"') if len(line) >= 4 else line
        if allow_analysis and path.startswith(".analysis/"):
            continue
        bad.append(line)
    if bad:
        raise AutopilotError("worktree is not clean:\n" + "\n".join(bad[:40]))


def selected_ids(snapshot: dict[str, Any], tasks: dict[str, dict[str, Any]], limit: int) -> list[str]:
    ready = snapshot["ready"]
    if ready:
        first = ready[0]
        result = [first]
        if limit > 1:
            for candidate in ready[1:]:
                if not conflicts(tasks[first], tasks[candidate]):
                    result.append(candidate)
                    break
        return result
    coordinator = snapshot.get("coordinator_ready", [])
    return coordinator[:1]

def write_claim(root: Path, task_id: str, payload: dict[str, Any]) -> None:
    _, tasks = load_manifest(root)
    task = tasks[task_id]
    path = card_path(root, task_id)
    text = path.read_text(encoding="utf-8")
    if card_status(root, task_id) != "QUEUED":
        raise AutopilotError(f"{task_id} is not QUEUED in {root}")
    text = STATUS_RE.sub("Status: `CLAIMED`", text, count=1)
    text = set_table_field(text, "Agent", f"{payload['role']}:{task_id}")
    text = set_table_field(text, "Worktree / branch", f"{payload['worktree']} / {payload['branch']}")
    text = set_table_field(text, "Base SHA", payload["base_sha"])
    text = set_table_field(text, "Started", utc_now())
    for label in DISPATCH_CHECKS[:3]:
        text = check_box(text, label)
    text = check_box(text, "CLAIMED")
    text = insert_dispatch(text, payload)
    text = replace_markdown_section(text, "## Validation log", validation_seed(task, payload))
    path.write_text(text, encoding="utf-8")

def prepare_one(state: dict[str, Any], campaign_root: Path, task: dict[str, Any], slot: str, worktrees_root: Path) -> dict[str, Any]:
    base = git(campaign_root, "rev-parse", "HEAD")
    branch = f"agent/{task['id'].lower()}-{base[:8]}"
    worktree = worktrees_root / task["id"]
    if worktree.exists():
        raise AutopilotError(f"worktree already exists: {worktree}")
    if run(["git", "show-ref", "--verify", "--quiet", f"refs/heads/{branch}"], cwd=campaign_root, check=False).returncode == 0:
        raise AutopilotError(f"branch already exists: {branch}")
    worktree.parent.mkdir(parents=True, exist_ok=True)
    run(["git", "worktree", "add", "-b", branch, str(worktree), base], cwd=campaign_root)
    role = role_for(task)
    ports = "25570-25579" if slot == "A" else "25580-25589"
    run_dir = f".analysis/runs/{task['id'].lower()}-{slot.lower()}"
    packet_dir = common_git_dir(campaign_root) / "spark-autopilot" / "packets"
    packet_dir.mkdir(parents=True, exist_ok=True)
    packet_path = packet_dir / f"{task['id']}-dispatch.md"
    payload = {
        "task_id": task["id"],
        "title": task["title"],
        "role": role,
        "execution": "primary" if role == "primary-coordinator" else "subagent",
        "slot": slot,
        "ports": ports,
        "run_dir": run_dir,
        "base_sha": base,
        "branch": branch,
        "worktree": str(worktree.resolve()),
        "owner_checkout": state.get("owner_checkout", str(campaign_root)),
        "packet": str(packet_path.resolve()),
        "claimed_at": utc_now(),
        "state": "claimed",
    }
    try:
        packet_path.write_text(task_packet_text(task, payload), encoding="utf-8")
        write_claim(worktree, task["id"], payload)
    except Exception:
        run(["git", "worktree", "remove", "--force", str(worktree)], cwd=campaign_root, check=False)
        run(["git", "branch", "-D", branch], cwd=campaign_root, check=False)
        raise
    state.setdefault("tasks", {})[task["id"]] = payload
    state.setdefault("events", []).append({"at": utc_now(), "kind": "claim", "task": task["id"], "base": base})
    return payload


def changed_paths(worktree: Path, base: str) -> list[str]:
    return [line for line in git(worktree, "diff", "--name-only", f"{base}..HEAD").splitlines() if line]


def path_allowed(path: str, allowed: list[str]) -> bool:
    value = normalized(path)
    for candidate in allowed:
        if PLACEHOLDER_RE.search(candidate):
            continue
        permitted = normalized(candidate)
        if value == permitted or value.startswith(permitted + "/"):
            return True
    return False


def candidate_report(state: dict[str, Any], campaign_root: Path, task_id: str) -> dict[str, Any]:
    _, tasks = load_manifest(campaign_root)
    task = tasks.get(task_id)
    info = state.get("tasks", {}).get(task_id)
    if not task or not info:
        raise AutopilotError(f"unknown/unclaimed task: {task_id}")
    worktree = Path(info["worktree"])
    if not worktree.exists():
        raise AutopilotError(f"missing task worktree: {worktree}")
    errors: list[str] = []
    status_output = git_raw(worktree, "status", "--porcelain=v1", "--untracked-files=all")
    for line in status_output.splitlines():
        path = line[3:].split(" -> ")[-1].strip('"') if len(line) >= 4 else line
        if path.startswith(".analysis/") or path.startswith("data/vanilla/"):
            continue
        errors.append("uncommitted: " + line)
    base = info["base_sha"]
    head = git(worktree, "rev-parse", "HEAD")
    if run(["git", "merge-base", "--is-ancestor", base, head], cwd=worktree, check=False).returncode != 0:
        errors.append("assigned base is not an ancestor of candidate HEAD")
    commits = int(git(worktree, "rev-list", "--count", f"{base}..HEAD") or "0")
    if commits < 1:
        errors.append("candidate has no commit above assigned base")
    changed = changed_paths(worktree, base)
    allowed = list(task.get("write", [])) + [f"{PLAN_REL}/tasks/{task_id}.md"]
    outside = [path for path in changed if not path_allowed(path, allowed)]
    if outside:
        errors.append("outside owned paths: " + ", ".join(outside))
    status = card_status(worktree, task_id)
    if status not in {"REVIEW", "DONE", "BLOCKED"}:
        errors.append(f"card status is {status}, expected REVIEW, DONE, or BLOCKED")
    text = card_path(worktree, task_id).read_text(encoding="utf-8")
    closeout_base = closeout_scalar(text, "base_tree")
    if closeout_base != base:
        errors.append(f"closeout base_tree is {closeout_base!r}, expected assigned base {base}")
    closeout_hash = closeout_scalar(text, "diff_hash")
    if not closeout_hash or closeout_hash in {"UNSET", "null", "[]"}:
        errors.append("closeout diff_hash is empty")
    if status in {"REVIEW", "DONE"}:
        validation_log = markdown_section(text, "## Validation log")
        if not validation_log.strip():
            errors.append("Validation log is empty")
        if "Before implementation, replace/extend this seed" in validation_log:
            errors.append("Validation log still contains the dispatch seed instruction")
        for label in DISPATCH_CHECKS:
            if not is_checked(text, label):
                errors.append(f"unchecked dispatch gate: {label}")
        for label in PRE_REVIEW_CHECKS:
            if not is_checked(text, label):
                errors.append(f"unchecked before review: {label}")
        for label in unchecked_boxes(text, "## Done when"):
            errors.append(f"unchecked Done when: {label}")
        if status == "REVIEW" and (is_checked(text, "INDEPENDENT REVIEW") or is_checked(text, "DONE")):
            errors.append("worker self-checked reviewer/DONE")
        if status == "DONE":
            if not is_checked(text, "INDEPENDENT REVIEW") or not is_checked(text, "DONE"):
                errors.append("DONE card lacks independent-review/DONE checks")
            if closeout_scalar(text, "verdict") != "pass":
                errors.append("DONE closeout verdict is not pass")
            if not re.search(r"^- Verdict: `PASS`$", text, re.MULTILINE):
                errors.append("DONE review section lacks PASS verdict")
    if status == "REVIEW":
        if closeout_scalar(text, "verdict") != "pass":
            errors.append("REVIEW closeout verdict must be pass")
        if closeout_scalar(text, "status") != "complete":
            errors.append("REVIEW closeout status must be complete")
        for key in ("changed_files", "validation", "evidence"):
            if closeout_list_is_empty(text, key):
                errors.append(f"REVIEW closeout {key} is empty")
    if status == "BLOCKED":
        if closeout_scalar(text, "verdict") != "blocked":
            errors.append("BLOCKED closeout verdict must be blocked")
        if closeout_scalar(text, "status") not in {"partial", "checkpoint-blocked"}:
            errors.append("BLOCKED closeout status must be partial/checkpoint-blocked")
        if closeout_list_is_empty(text, "known_gaps"):
            errors.append("BLOCKED closeout known_gaps is empty")
    if any(token in text for token in ("base_tree: UNSET", "diff_hash: UNSET", "next: claim this task")):
        errors.append("closeout still contains required placeholders")
    next_value = closeout_scalar(text, "next")
    if not next_value or next_value in {"null", "[]", "UNSET"}:
        errors.append("closeout next action is empty")
    diff = git_raw(worktree, "diff", "--binary", f"{base}..HEAD")
    diff_hash = hashlib.sha256(diff.encode("utf-8", errors="replace")).hexdigest()
    reviewer_dir = common_git_dir(campaign_root) / "spark-autopilot" / "packets"
    reviewer_dir.mkdir(parents=True, exist_ok=True)
    reviewer_packet = reviewer_dir / f"{task_id}-review.md"
    reviewer_packet.write_text(
        f"# Solaris reviewer packet — {task_id}\n\n"
        f"Worktree: `{worktree}`\nBase: `{base}`\nCandidate: `{head}`\n"
        f"Card: `{card_path(worktree, task_id)}`\nOutcome: {task['outcome']}\n"
        f"Changed files: `{changed}`\nDiff hash: `{diff_hash}`\n\n"
        "Review only this card and `git diff <base>..HEAD`. Do not edit or spawn agents. "
        "Return YAML exactly as required by the solaris_reviewer role.\n",
        encoding="utf-8",
    )
    return {
        "ok": not errors,
        "task_id": task_id,
        "card_status": status,
        "errors": errors,
        "base_sha": base,
        "head_sha": head,
        "commit_count": commits,
        "changed_files": changed,
        "outside_owned_paths": outside,
        "diff_hash": diff_hash,
        "reviewer_packet": str(reviewer_packet.resolve()),
        "worktree": str(worktree),
    }

def replace_review(text: str, reviewer: str, verdict: str, summary: str) -> str:
    summary = summary.replace("`", "'").replace("\n", " ").strip()[:1200] or "[]"
    text = re.sub(r"^- Reviewer: `.*`$", f"- Reviewer: `{reviewer}`", text, count=1, flags=re.MULTILINE)
    text = re.sub(r"^- Verdict: `.*`$", f"- Verdict: `{verdict.upper()}`", text, count=1, flags=re.MULTILINE)
    text = re.sub(r"^- Findings: `.*`$", f"- Findings: `{summary}`", text, count=1, flags=re.MULTILINE)
    return text


def replace_last_scalar(text: str, key: str, value: str) -> str:
    matches = list(re.finditer(rf"^{re.escape(key)}: .*?$", text, re.MULTILINE))
    if not matches:
        return text
    match = matches[-1]
    return text[: match.start()] + f"{key}: {value}" + text[match.end() :]


def mark_board_done(root: Path, task_id: str) -> None:
    path = plan_root(root) / "BOARD.md"
    text = path.read_text(encoding="utf-8")
    pattern = re.compile(rf"^- \[ \] (\[`{re.escape(task_id)}`[^\n]+)$", re.MULTILINE)
    updated, count = pattern.subn(r"- [x] \1", text, count=1)
    if count != 1:
        raise AutopilotError(f"could not mark {task_id} done in BOARD.md")
    path.write_text(updated, encoding="utf-8")


def archive_artifacts(campaign_root: Path, task_id: str, worktree: Path) -> str | None:
    source = worktree / ".analysis"
    if not source.exists():
        return None
    target = campaign_root / ".analysis" / "spark-autopilot" / "artifacts" / task_id
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, target, dirs_exist_ok=True)
    return str(target.resolve())


def command_bootstrap(args: argparse.Namespace) -> int:
    owner = repo_root(Path(args.repo).resolve() if args.repo else None)
    source_plan = plan_root(owner)
    if not source_plan.exists():
        raise AutopilotError(f"install the pack first; missing {source_plan}")
    with state_lock(owner, create=True) as (state, state_path):
        if state is not None:
            existing = Path(state.get("campaign_root", ""))
            if existing.exists():
                print(json.dumps({"status": "already-bootstrapped", **state}, ensure_ascii=False, indent=2))
                return 0
            raise AutopilotError(f"durable state points to a missing campaign: {existing}; inspect/remove {state_path} explicitly")
        timestamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
        short = git(owner, "rev-parse", "--short=8", "HEAD")
        branch = args.branch or f"agent/spark-campaign-{short}-{timestamp}"
        worktrees_root = Path(args.worktrees_root).expanduser().resolve() if args.worktrees_root else owner.parent / f"{owner.name}-spark-worktrees"
        campaign = worktrees_root / "campaign"
        if campaign.exists() and any(campaign.iterdir()):
            raise AutopilotError(f"campaign path is not empty: {campaign}")
        campaign.parent.mkdir(parents=True, exist_ok=True)
        base = git(owner, "rev-parse", "HEAD")
        run(["git", "worktree", "add", "-b", branch, str(campaign), base], cwd=owner)
        try:
            shutil.copytree(source_plan, campaign / PLAN_REL, dirs_exist_ok=True)
            agents = owner / ".codex" / "agents"
            if agents.exists():
                shutil.copytree(agents, campaign / ".codex" / "agents", dirs_exist_ok=True)
            git(campaign, "add", str(PLAN_REL), ".codex/agents")
            if git(campaign, "diff", "--cached", "--name-only"):
                git(campaign, "commit", "-m", "chore(spark): bootstrap autonomous campaign")
            validate_board(campaign)
        except Exception:
            run(["git", "worktree", "remove", "--force", str(campaign)], cwd=owner, check=False)
            run(["git", "branch", "-D", branch], cwd=owner, check=False)
            raise
        state = {
            "schema": 2,
            "created_at": utc_now(),
            "owner_checkout": str(owner),
            "owner_head": base,
            "owner_branch": git(owner, "branch", "--show-current"),
            "owner_dirty_paths": owner_dirty_paths(owner),
            "campaign_root": str(campaign.resolve()),
            "campaign_branch": branch,
            "campaign_head": git(campaign, "rev-parse", "HEAD"),
            "worktrees_root": str(worktrees_root.resolve()),
            "tasks": {},
            "events": [{"at": utc_now(), "kind": "bootstrap", "base": base}],
        }
        state_path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"status": "bootstrapped", "state_file": str(state_path), **state}, ensure_ascii=False, indent=2))
    return 0


def command_doctor(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.repo).resolve() if args.repo else None)
    if args.init_state and not default_state_path(root).exists():
        with state_lock(root, create=True) as (state, path):
            state = {
                "schema": 2,
                "created_at": utc_now(),
                "owner_checkout": str(root),
                "owner_head": git(root, "rev-parse", "HEAD"),
                "owner_branch": git(root, "branch", "--show-current"),
                "owner_dirty_paths": owner_dirty_paths(root),
                "campaign_root": str(root),
                "campaign_branch": git(root, "branch", "--show-current"),
                "campaign_head": git(root, "rev-parse", "HEAD"),
                "worktrees_root": str((root.parent / f"{root.name}-spark-worktrees").resolve()),
                "tasks": {},
                "events": [{"at": utc_now(), "kind": "init-state"}],
            }
            path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    issues: list[str] = []
    target = root
    state_payload: dict[str, Any] | None = None
    if default_state_path(root).exists():
        with state_lock(root) as (state, _):
            assert state is not None
            state_payload = state
            campaign = campaign_root_from_state(state, root)
            if campaign.exists():
                target = campaign
            else:
                issues.append(f"campaign_root is missing: {campaign}")
    try:
        validation = validate_board(target)
    except AutopilotError as exc:
        validation = "INVALID"
        issues.append(str(exc))
    for name in ("solaris-worker.toml", "solaris-evidence.toml", "solaris-reviewer.toml", "solaris-explorer.toml"):
        if not (target / ".codex" / "agents" / name).exists():
            issues.append(f"missing .codex/agents/{name}")
    result: dict[str, Any] = {
        "repo": str(root),
        "authoritative_root": str(target),
        "branch": git(target, "branch", "--show-current"),
        "head": git(target, "rev-parse", "HEAD"),
        "board": validation,
        "issues": issues,
    }
    if state_payload is not None and target.exists():
        result["state_file"] = str(default_state_path(root))
        result["campaign_root"] = str(target)
        result["owner_dirty_paths"] = state_payload.get("owner_dirty_paths", [])
        result["snapshot"] = ready_snapshot(state_payload, target)
    result["ok"] = not issues
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if not issues else 1

def command_next(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        root = campaign_root_from_state(state, current)
        snapshot = ready_snapshot(state, root)
        _, tasks = load_manifest(root)
        snapshot["selected"] = selected_ids(snapshot, tasks, min(max(args.limit, 1), 2))
        print(json.dumps(snapshot, ensure_ascii=False, indent=2))
    return 0


def command_dispatch(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        root = campaign_root_from_state(state, current)
        ensure_clean(root)
        validate_board(root)
        snapshot = ready_snapshot(state, root)
        _, tasks = load_manifest(root)
        ids = selected_ids(snapshot, tasks, min(max(args.limit, 1), 2))
        if not ids:
            status = "template-required" if snapshot.get("templates_ready_for_materialization") else "no-ready-work"
            print(json.dumps({"status": status, **snapshot}, ensure_ascii=False, indent=2))
            return 0 if status == "template-required" else 2
        worktrees_root = Path(state["worktrees_root"]) / "tasks"
        dispatches = [prepare_one(state, root, tasks[task_id], slot, worktrees_root) for task_id, slot in zip(ids, ("A", "B"))]
        status = "coordinator-dispatched" if dispatches[0].get("execution") == "primary" else "dispatched"
        print(json.dumps({"status": status, "dispatches": dispatches}, ensure_ascii=False, indent=2))
    return 0

def command_packet(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        _, tasks = load_manifest(campaign)
        task = tasks.get(args.task)
        if not task:
            raise AutopilotError(f"unknown task: {args.task}")
        slot = args.slot.upper()
        packet_dir = common_git_dir(current) / "spark-autopilot" / "packets"
        packet_dir.mkdir(parents=True, exist_ok=True)
        packet_path = packet_dir / f"{args.task}-dispatch.md"
        payload = {
            "task_id": args.task,
            "title": task["title"],
            "role": role_for(task),
            "slot": slot,
            "ports": "25570-25579" if slot == "A" else "25580-25589",
            "run_dir": f".analysis/runs/{args.task.lower()}-{slot.lower()}",
            "base_sha": args.base,
            "branch": args.branch,
            "worktree": str(Path(args.worktree).resolve()),
            "owner_checkout": state.get("owner_checkout", str(current)),
            "packet": str(packet_path.resolve()),
            "state": "prepared",
        }
        packet_path.write_text(task_packet_text(task, payload), encoding="utf-8")
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0


def command_claim(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(root) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, root)
        _, tasks = load_manifest(campaign)
        task = tasks.get(args.task)
        if not task:
            raise AutopilotError(f"unknown task: {args.task}")
        slot = args.slot.upper()
        packet_dir = common_git_dir(root) / "spark-autopilot" / "packets"
        packet_path = packet_dir / f"{args.task}-dispatch.md"
        payload = {
            "task_id": args.task,
            "title": task["title"],
            "role": args.agent,
            "slot": slot,
            "ports": "25570-25579" if slot == "A" else "25580-25589",
            "run_dir": f".analysis/runs/{args.task.lower()}-{slot.lower()}",
            "base_sha": args.base,
            "branch": args.branch,
            "worktree": str(Path(args.worktree).resolve()),
            "owner_checkout": state.get("owner_checkout", str(root)),
            "packet": str(packet_path.resolve()),
            "claimed_at": utc_now(),
            "state": "claimed",
        }
        if not packet_path.exists():
            packet_path.write_text(task_packet_text(task, payload), encoding="utf-8")
        write_claim(root, args.task, payload)
        state.setdefault("tasks", {})[args.task] = payload
        state.setdefault("events", []).append({"at": utc_now(), "kind": "claim", "task": args.task})
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    return 0


def command_candidate(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        report = candidate_report(state, campaign, args.task)
        if report["ok"]:
            if report["card_status"] == "REVIEW":
                state["tasks"][args.task]["state"] = "candidate"
            elif report["card_status"] == "BLOCKED":
                state["tasks"][args.task]["state"] = "blocked"
            state["tasks"][args.task]["candidate_head"] = report["head_sha"]
            state["tasks"][args.task]["diff_hash"] = report["diff_hash"]
            state.setdefault("events", []).append({"at": utc_now(), "kind": "candidate", "task": args.task})
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0 if report["ok"] else 1


def command_review(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        info = state.get("tasks", {}).get(args.task)
        if not info:
            raise AutopilotError(f"unknown task: {args.task}")
        worktree = Path(info["worktree"])
        campaign = campaign_root_from_state(state, current)
        report = candidate_report(state, campaign, args.task)
        if args.verdict in {"pass", "changes"}:
            if info.get("state") != "candidate":
                raise AutopilotError(f"{args.verdict} requires candidate state, got {info.get('state')}")
            if not report["ok"] or report["card_status"] != "REVIEW":
                raise AutopilotError("review verdict requires an unchanged valid REVIEW candidate")
            if report["head_sha"] != info.get("candidate_head") or report["diff_hash"] != info.get("diff_hash"):
                raise AutopilotError("candidate changed after reviewer packet was issued; rerun candidate and review")
        elif args.verdict == "blocked" and not report["ok"]:
            raise AutopilotError("blocked review requires a valid candidate/checkpoint")
        card = card_path(worktree, args.task)
        text = card.read_text(encoding="utf-8")
        status = card_status(worktree, args.task)
        if args.verdict == "changes":
            info["state"] = "review-changes"
            info["review_summary"] = args.summary
        elif args.verdict == "pass":
            if status != "REVIEW":
                raise AutopilotError(f"PASS requires REVIEW card, got {status}")
            text = replace_review(text, args.reviewer, "pass", args.summary)
            text = STATUS_RE.sub("Status: `DONE`", text, count=1)
            text = check_box(text, "INDEPENDENT REVIEW")
            text = check_box(text, "DONE")
            text = replace_last_scalar(text, "verdict", "pass")
            text = replace_last_scalar(text, "status", "complete")
            text = replace_last_scalar(text, "next", f"integrate {args.task}")
            card.write_text(text, encoding="utf-8")
            git(worktree, "add", str(card.relative_to(worktree)))
            if git(worktree, "diff", "--cached", "--name-only"):
                git(worktree, "commit", "-m", f"chore({args.task}): record independent review")
            info["state"] = "review-pass"
            info["reviewed_candidate_head"] = report["head_sha"]
        else:
            text = replace_review(text, args.reviewer, "blocked", args.summary)
            text = STATUS_RE.sub("Status: `BLOCKED`", text, count=1)
            text = replace_last_scalar(text, "verdict", "blocked")
            text = replace_last_scalar(text, "status", "checkpoint-blocked")
            compact = args.summary.replace("`", "'").replace("\n", " ").strip()[:800] or "independent review blocker"
            text = replace_last_scalar(text, "known_gaps", json.dumps([compact], ensure_ascii=False))
            text = replace_last_scalar(text, "next", "resolve independent-review blocker, then redispatch this card")
            card.write_text(text, encoding="utf-8")
            git(worktree, "add", str(card.relative_to(worktree)))
            if git(worktree, "diff", "--cached", "--name-only"):
                git(worktree, "commit", "-m", f"chore({args.task}): record blocked review")
            info["state"] = "blocked"
        info["review_summary"] = args.summary
        state.setdefault("events", []).append({"at": utc_now(), "kind": "review", "task": args.task, "verdict": args.verdict})
        print(json.dumps({"status": "review-recorded", "task": args.task, "verdict": args.verdict, "next": "integrate" if args.verdict == "pass" else "return-to-worker" if args.verdict == "changes" else "checkpoint"}, ensure_ascii=False, indent=2))
    return 0

def cherry_pick_no_commit(root: Path, commits: list[str]) -> None:
    proc = run(["git", "cherry-pick", "--no-commit", *commits], cwd=root, check=False)
    if proc.returncode != 0:
        run(["git", "cherry-pick", "--abort"], cwd=root, check=False)
        raise AutopilotError("cherry-pick conflict; aborted safely\n" + (proc.stdout or "")[-1500:] + "\n" + (proc.stderr or "")[-1500:])


def command_integrate(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        ensure_clean(campaign)
        report = candidate_report(state, campaign, args.task)
        if not report["ok"]:
            raise AutopilotError("candidate failed inspection: " + "; ".join(report["errors"]))
        if report["card_status"] != "DONE" or state["tasks"][args.task].get("state") != "review-pass":
            raise AutopilotError("task is not independently reviewed DONE")
        info = state["tasks"][args.task]
        worktree = Path(info["worktree"])
        commits = [line for line in git(worktree, "rev-list", "--reverse", f"{info['base_sha']}..HEAD").splitlines() if line]
        if not commits:
            raise AutopilotError("no commits to integrate")
        if run(["git", "merge-base", "--is-ancestor", info["base_sha"], git(campaign, "rev-parse", "HEAD")], cwd=campaign, check=False).returncode != 0:
            raise AutopilotError("candidate base is not an ancestor of campaign HEAD")
        cherry_pick_no_commit(campaign, commits)
        mark_board_done(campaign, args.task)
        validate_board(campaign)
        git(campaign, "add", str(PLAN_REL / "BOARD.md"))
        _, tasks = load_manifest(campaign)
        git(campaign, "commit", "-m", f"task({args.task}): {tasks[args.task]['title']}")
        sha = git(campaign, "rev-parse", "HEAD")
        artifact_archive = archive_artifacts(campaign, args.task, worktree)
        info["state"] = "integrated"
        info["integrated_sha"] = sha
        info["integrated_at"] = utc_now()
        info["artifact_archive"] = artifact_archive
        state["campaign_head"] = sha
        state.setdefault("events", []).append({"at": utc_now(), "kind": "integrate", "task": args.task, "sha": sha})
        run(["git", "worktree", "remove", "--force", str(worktree)], cwd=campaign, check=False)
        run(["git", "branch", "-D", info["branch"]], cwd=campaign, check=False)
        print(json.dumps({"status": "integrated", "task": args.task, "campaign_sha": sha, "artifact_archive": artifact_archive, "next": "dashboard then dispatch"}, ensure_ascii=False, indent=2))
    return 0


def command_checkpoint(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        ensure_clean(campaign)
        report = candidate_report(state, campaign, args.task)
        if not report["ok"] or report["card_status"] != "BLOCKED":
            raise AutopilotError("blocked checkpoint failed inspection")
        info = state["tasks"][args.task]
        worktree = Path(info["worktree"])
        commits = [line for line in git(worktree, "rev-list", "--reverse", f"{info['base_sha']}..HEAD").splitlines() if line]
        if commits:
            cherry_pick_no_commit(campaign, commits)
            validate_board(campaign)
            git(campaign, "commit", "-m", f"chore({args.task}): checkpoint blocker")
        sha = git(campaign, "rev-parse", "HEAD")
        info["state"] = "blocked-integrated"
        info["integrated_sha"] = sha
        info["artifact_archive"] = archive_artifacts(campaign, args.task, worktree)
        state["campaign_head"] = sha
        state.setdefault("events", []).append({"at": utc_now(), "kind": "checkpoint", "task": args.task, "sha": sha})
        run(["git", "worktree", "remove", "--force", str(worktree)], cwd=campaign, check=False)
        run(["git", "branch", "-D", info["branch"]], cwd=campaign, check=False)
        print(json.dumps({"status": "blocked-checkpoint-integrated", "task": args.task, "campaign_sha": sha}, indent=2))
    return 0



def template_packet_text(task: dict[str, Any], campaign: Path, packet_path: Path) -> str:
    return f"""# Solaris template materialization packet — {task['id']}

Role: `solaris_explorer` (read-only). Materialize one measured task; do not implement it.
Campaign: `{campaign}`
Card: `{card_path(campaign, task['id'])}`
Outcome: {task['outcome']}

Read only the card, its completed materialization dependencies, and exact paths named there. Return compact YAML with one measured hotspot/failure, exact read/write paths, locks, leases, one RED command/artifact, focused validation commands, and evidence legs. No placeholders, broad directories, redesign, edits, or subagents.

The primary converts the result into this JSON schema and runs `materialize --task {task['id']} --spec <file>`:

```json
{{
  "read": ["exact/file"],
  "write": ["exact/file"],
  "locks": ["LOCK"],
  "leases": [],
  "discovery": ["exact RED command and artifact"],
  "validation": ["exact focused command", "exact workload rerun"],
  "evidence_legs": {json.dumps(task.get('evidence_legs', []), ensure_ascii=False)}
}}
```
"""


def command_template_packet(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        snapshot = ready_snapshot(state, campaign)
        if args.task not in snapshot.get("templates_ready_for_materialization", []):
            raise AutopilotError(f"template is not ready for materialization: {args.task}")
        _, tasks = load_manifest(campaign)
        task = tasks[args.task]
        packet_dir = common_git_dir(campaign) / "spark-autopilot" / "packets"
        packet_dir.mkdir(parents=True, exist_ok=True)
        packet = packet_dir / f"{args.task}-materialize.md"
        packet.write_text(template_packet_text(task, campaign, packet), encoding="utf-8")
        print(json.dumps({"status": "template-packet", "task": args.task, "role": "solaris_explorer", "packet": str(packet), "next": f"spawn solaris_explorer, write exact JSON spec, run materialize --task {args.task} --spec <file>"}, ensure_ascii=False, indent=2))
    return 0


def exact_value_list(
    values: Any,
    field: str,
    *,
    allow_empty: bool = False,
    forbid_angle: bool = True,
) -> list[str]:
    if not isinstance(values, list) or (not values and not allow_empty):
        raise AutopilotError(f"materialization field {field} must be a {'possibly empty ' if allow_empty else 'non-empty '}list")
    result: list[str] = []
    for value in values:
        invalid_angle = forbid_angle and isinstance(value, str) and ("<" in value or ">" in value)
        if not isinstance(value, str) or not value.strip() or PLACEHOLDER_RE.search(value) or invalid_angle:
            raise AutopilotError(f"materialization field {field} contains non-exact value: {value!r}")
        result.append(value.strip())
    return result


def exact_path_list(values: Any, field: str, *, allow_empty: bool = False) -> list[str]:
    return exact_value_list(values, field, allow_empty=allow_empty, forbid_angle=True)


def exact_command_list(values: Any, field: str) -> list[str]:
    return exact_value_list(values, field, forbid_angle=False)


def exact_repo_paths(values: Any, field: str) -> list[str]:
    result = exact_path_list(values, field)
    for value in result:
        normalized_value = value.replace("\\", "/")
        parts = Path(normalized_value).parts
        if (
            Path(normalized_value).is_absolute()
            or ".." in parts
            or normalized_value.endswith("/")
            or re.search(r"[\s*?\[\]{};$|`\n\r]", normalized_value)
        ):
            raise AutopilotError(f"materialization field {field} contains unsafe/non-file path: {value!r}")
    return result


def preserve_materialized_requirements(
    task: dict[str, Any],
    read: list[str],
    write: list[str],
    locks: list[str],
    leases: list[str],
    evidence: list[str],
) -> None:
    def exact_original(field: str) -> set[str]:
        return {
            value
            for value in task.get(field, [])
            if isinstance(value, str) and not PLACEHOLDER_RE.search(value)
        }

    supplied = {
        "read": set(read),
        "write": set(write),
        "locks": set(locks),
        "leases": set(leases),
        "evidence_legs": set(evidence),
    }
    for field, values in supplied.items():
        missing = sorted(exact_original(field) - values)
        if missing:
            raise AutopilotError(f"materialization may not weaken {field}; missing: {missing}")


def bullet_body(values: list[str], *, code: bool = True) -> str:
    if code:
        return "\n".join(f"- `{value}`" for value in values)
    return "\n".join(f"- {value}" for value in values)


def command_materialize(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    spec_path = Path(args.spec).expanduser().resolve()
    if not spec_path.exists():
        raise AutopilotError(f"missing materialization spec: {spec_path}")
    try:
        spec = json.loads(spec_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        raise AutopilotError(f"invalid materialization JSON: {exc}") from exc
    with state_lock(current) as (state, _):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        ensure_clean(campaign)
        snapshot = ready_snapshot(state, campaign)
        if args.task not in snapshot.get("templates_ready_for_materialization", []):
            raise AutopilotError(f"template is not ready for materialization: {args.task}")
        manifest, tasks = load_manifest(campaign)
        task = tasks[args.task]
        read = exact_repo_paths(spec.get("read"), "read")
        write = exact_repo_paths(spec.get("write"), "write")
        locks = exact_path_list(spec.get("locks"), "locks")
        leases = exact_path_list(spec.get("leases", []), "leases", allow_empty=True)
        discovery = exact_command_list(spec.get("discovery"), "discovery")
        validation = exact_command_list(spec.get("validation"), "validation")
        evidence = exact_path_list(spec.get("evidence_legs", task.get("evidence_legs", [])), "evidence_legs")
        preserve_materialized_requirements(task, read, write, locks, leases, evidence)
        task.update({
            "read": read,
            "write": write,
            "locks": locks,
            "leases": leases,
            "discovery": discovery,
            "validation": validation,
            "evidence_legs": evidence,
            "dispatch": "ready",
        })
        manifest_path = plan_root(campaign) / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        card = card_path(campaign, args.task)
        text = card.read_text(encoding="utf-8")
        text = set_table_field(text, "Dispatch", "READY after dependencies")
        text = set_table_field(text, "Write locks", ", ".join(locks) or "NONE")
        text = set_table_field(text, "Runtime leases", ", ".join(leases) or "NONE")
        text = set_table_field(text, "Required evidence", ", ".join(evidence) or "NONE")
        dispatch_gate = "\n".join(f"- [ ] {label}" for label in DISPATCH_CHECKS)
        text = replace_markdown_section(text, "## Dispatch gate", dispatch_gate)
        text = replace_markdown_section(text, "## Read-only context — do not broaden", bullet_body(read) + "\n\nFor any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.")
        text = replace_markdown_section(text, "## Owned write paths", bullet_body(write) + "\n\nAny additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.")
        text = replace_markdown_section(text, "## Bounded discovery / search anchors", bullet_body(discovery, code=False))
        text = replace_markdown_section(text, "## Required evidence legs", bullet_body(evidence))
        text = replace_markdown_section(text, "## Required validation", bullet_body(validation))
        card.write_text(text, encoding="utf-8")
        validate_board(campaign)
        git(campaign, "add", str(manifest_path.relative_to(campaign)), str(card.relative_to(campaign)))
        git(campaign, "commit", "-m", f"chore({args.task}): materialize measured task card")
        sha = git(campaign, "rev-parse", "HEAD")
        state["campaign_head"] = sha
        state.setdefault("events", []).append({"at": utc_now(), "kind": "materialize", "task": args.task, "sha": sha})
        print(json.dumps({"status": "materialized", "task": args.task, "campaign_sha": sha, "next": "dispatch --limit 2 --json"}, ensure_ascii=False, indent=2))
    return 0


def render_dashboard(state: dict[str, Any], campaign: Path, snapshot: dict[str, Any]) -> str:
    manifest, tasks = load_manifest(campaign)
    lines = [
        "# Solaris Spark Autopilot Status",
        "",
        "Generated from task cards plus durable Git-common-dir state. Edit task cards, not this file.",
        "",
        "## Counts",
        "",
    ]
    for key, value in sorted(snapshot.get("counts", {}).items()):
        lines.append(f"- `{key}`: **{value}**")
    lines += ["", "## Cursor", ""]
    lines.append(f"- Active: `{', '.join(snapshot.get('active', [])) or 'none'}`")
    lines.append(f"- Ready: `{', '.join(snapshot.get('ready', [])[:12]) or 'none'}`")
    lines.append(f"- Coordinator: `{', '.join(snapshot.get('coordinator_ready', [])) or 'none'}`")
    lines.append(f"- Templates: `{', '.join(snapshot.get('templates_ready_for_materialization', [])) or 'none'}`")
    lines += ["", "## Active details", ""]
    active = snapshot.get("active", [])
    if active:
        for task_id in active:
            info = state.get("tasks", {}).get(task_id, {})
            lines.append(f"- [ ] `{task_id}` · **{info.get('state', 'unknown').upper()}** · `{info.get('role', 'unknown')}` · `{info.get('worktree', 'missing')}`")
    else:
        lines.append("- none")
    waves: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for task in manifest.get("tasks", []):
        waves[task.get("wave", "W??")].append(task)
    for wave in sorted(waves):
        lines += ["", f"## {wave}", ""]
        for task in waves[wave]:
            task_id = task["id"]
            status = card_status(campaign, task_id)
            info = state.get("tasks", {}).get(task_id, {})
            display = "DONE" if status == "DONE" else str(info.get("state", status)).upper()
            mark = "x" if status == "DONE" else " "
            lines.append(f"- [{mark}] `{task_id}` · **{display}** · {task['title']}")
    lines += ["", "## Machine cursor", "", "Run `python3 docs/spark-team/scripts/autopilot.py dispatch --limit 2 --json` from the campaign worktree.", ""]
    return "\n".join(lines)


def command_event(args: argparse.Namespace) -> int:
    root = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(root) as (state, _):
        assert state is not None
        state.setdefault("events", []).append({"at": utc_now(), "kind": args.kind, "message": args.message})
        print(json.dumps({"status": "event-recorded", "kind": args.kind}, indent=2))
    return 0


def command_dashboard(args: argparse.Namespace) -> int:
    current = repo_root(Path(args.repo).resolve() if args.repo else None)
    with state_lock(current) as (state, path):
        assert state is not None
        campaign = campaign_root_from_state(state, current)
        snapshot = ready_snapshot(state, campaign)
        if args.write_md:
            ensure_clean(campaign)
            dashboard_path = plan_root(campaign) / "AUTOPILOT_STATUS.md"
            rendered = render_dashboard(state, campaign, snapshot)
            before = dashboard_path.read_text(encoding="utf-8") if dashboard_path.exists() else ""
            if rendered != before:
                dashboard_path.write_text(rendered, encoding="utf-8")
                git(campaign, "add", str(dashboard_path.relative_to(campaign)))
                git(campaign, "commit", "-m", "chore(spark): refresh autopilot dashboard")
                sha = git(campaign, "rev-parse", "HEAD")
                state["campaign_head"] = sha
                state.setdefault("events", []).append({"at": utc_now(), "kind": "dashboard", "sha": sha})
                snapshot = ready_snapshot(state, campaign)
            snapshot["dashboard_path"] = str(dashboard_path)
        snapshot["state_file"] = str(path)
        snapshot["task_states"] = {task_id: info.get("state") for task_id, info in sorted(state.get("tasks", {}).items())}
        snapshot["recent_events"] = state.get("events", [])[-12:]
        print(json.dumps(snapshot, ensure_ascii=False, indent=2))
    return 0

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="repository/worktree path; defaults to cwd")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("bootstrap", help="create isolated campaign worktree and initialize durable state")
    p.add_argument("--branch")
    p.add_argument("--worktrees-root")
    p.set_defaults(func=command_bootstrap)

    p = sub.add_parser("doctor", help="validate installation, board, state, and campaign snapshot")
    p.add_argument("--init-state", action="store_true")
    p.set_defaults(func=command_doctor)

    p = sub.add_parser("next", help="print ready tasks, compatible pairs, templates, and coordinator gates")
    p.add_argument("--limit", type=int, default=2)
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=command_next)

    p = sub.add_parser("dispatch", help="create/claim up to two compatible isolated task worktrees")
    p.add_argument("--limit", type=int, default=2)
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=command_dispatch)

    p = sub.add_parser("packet", help="generate a dispatch packet for an already-created worktree")
    p.add_argument("--task", required=True)
    p.add_argument("--worktree", required=True)
    p.add_argument("--branch", required=True)
    p.add_argument("--base", required=True)
    p.add_argument("--slot", choices=["A", "B", "a", "b"], required=True)
    p.set_defaults(func=command_packet)

    p = sub.add_parser("claim", help="mark a task card claimed in its worktree")
    p.add_argument("--task", required=True)
    p.add_argument("--agent", required=True)
    p.add_argument("--worktree", required=True)
    p.add_argument("--branch", required=True)
    p.add_argument("--base", required=True)
    p.add_argument("--slot", choices=["A", "B", "a", "b"], default="A")
    p.set_defaults(func=command_claim)

    p = sub.add_parser("candidate", help="verify worker branch and emit a reviewer packet")
    p.add_argument("--task", required=True)
    p.set_defaults(func=command_candidate)

    p = sub.add_parser("review", help="record reviewer verdict; PASS marks/commits task DONE")
    p.add_argument("--task", required=True)
    p.add_argument("--verdict", choices=["pass", "changes", "blocked"], required=True)
    p.add_argument("--reviewer", default="solaris_reviewer")
    p.add_argument("--summary", default="[]")
    p.set_defaults(func=command_review)

    p = sub.add_parser("integrate", help="squash-integrate a reviewed DONE candidate")
    p.add_argument("--task", required=True)
    p.set_defaults(func=command_integrate)

    p = sub.add_parser("checkpoint", help="integrate useful BLOCKED evidence without satisfying dependencies")
    p.add_argument("--task", required=True)
    p.set_defaults(func=command_checkpoint)

    p = sub.add_parser("template-packet", help="emit a bounded read-only packet for one ready TEMPLATE card")
    p.add_argument("--task", required=True)
    p.set_defaults(func=command_template_packet)

    p = sub.add_parser("materialize", help="turn one measured TEMPLATE into an exact READY card from JSON")
    p.add_argument("--task", required=True)
    p.add_argument("--spec", required=True)
    p.set_defaults(func=command_materialize)

    p = sub.add_parser("event", help="append a durable compact campaign event")
    p.add_argument("--kind", required=True)
    p.add_argument("--message", required=True)
    p.set_defaults(func=command_event)

    p = sub.add_parser("dashboard", help="print compact durable campaign dashboard")
    p.add_argument("--write-md", action="store_true", help="regenerate/commit docs/spark-team/AUTOPILOT_STATUS.md")
    p.set_defaults(func=command_dashboard)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    try:
        return int(args.func(args))
    except AutopilotError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("ERROR: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
