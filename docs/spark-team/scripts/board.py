#!/usr/bin/env python3
"""Validate and query the Solaris Spark Markdown task DAG."""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict, deque
from pathlib import Path

PLAN = Path(__file__).resolve().parents[1]
MANIFEST = PLAN / "manifest.json"
STATUS_RE = re.compile(r"^Status: `([A-Z-]+)`$", re.MULTILINE)
ALLOWED = {"QUEUED", "CLAIMED", "IMPLEMENTING", "TESTING", "REVIEW", "DONE", "BLOCKED"}
PLACEHOLDER_RE = re.compile(r"<[^>]+>|^(?:exact |focused |task cards|.*\+ evidence artifacts)", re.I)


def load():
    data = json.loads(MANIFEST.read_text())
    tasks = {t["id"]: t for t in data["tasks"]}
    return data, tasks


def status(tid: str) -> str:
    path = PLAN / "tasks" / f"{tid}.md"
    if not path.exists():
        return "MISSING"
    match = STATUS_RE.search(path.read_text())
    return match.group(1) if match else "INVALID"


def has_rust_lock(t):
    return any(lock.startswith("RUST-") for lock in t.get("locks", []))


def conflict(a, b):
    la, lb = set(a.get("locks", [])), set(b.get("locks", []))
    reasons = []
    if la & lb:
        reasons.append("shared locks=" + ",".join(sorted(la & lb)))
    if "VALIDATION" in la or "VALIDATION" in lb:
        reasons.append("VALIDATION singleton")
    if "PERF" in la or "PERF" in lb:
        reasons.append("PERF singleton")
    if "RUST-NET-ROOT" in la and has_rust_lock(b):
        reasons.append("RUST-NET-ROOT vs Rust runtime")
    if "RUST-NET-ROOT" in lb and has_rust_lock(a):
        reasons.append("RUST-NET-ROOT vs Rust runtime")
    leases_a, leases_b = set(a.get("leases", [])), set(b.get("leases", []))
    if leases_a & leases_b:
        reasons.append("shared leases=" + ",".join(sorted(leases_a & leases_b)))
    if {"CLEAN-HOST", "TREE-FROZEN"} & leases_a:
        reasons.append("first task requires exclusive host/tree")
    if {"CLEAN-HOST", "TREE-FROZEN"} & leases_b:
        reasons.append("second task requires exclusive host/tree")
    if a.get("dispatch") == "coordinator-only" or b.get("dispatch") == "coordinator-only":
        reasons.append("coordinator-only singleton")
    for p in a.get("write", []):
        for q in b.get("write", []):
            if PLACEHOLDER_RE.search(p) or PLACEHOLDER_RE.search(q) or "<" in p or "<" in q:
                continue
            pp, qq = p.rstrip("/"), q.rstrip("/")
            if pp == qq or pp.startswith(qq + "/") or qq.startswith(pp + "/"):
                reasons.append(f"overlapping writes={p}|{q}")
    return reasons


def validate() -> int:
    data, tasks = load()
    errors = []
    warnings = []
    if data.get("task_count") != len(tasks):
        errors.append(f"task_count={data.get('task_count')} but unique tasks={len(tasks)}")
    ids = [t["id"] for t in data["tasks"]]
    if len(ids) != len(set(ids)):
        errors.append("duplicate task IDs")

    indegree = {tid: 0 for tid in tasks}
    reverse = defaultdict(list)
    for tid, task in tasks.items():
        for dep in task.get("depends", []):
            if dep not in tasks:
                errors.append(f"{tid}: missing dependency {dep}")
            else:
                indegree[tid] += 1
                reverse[dep].append(tid)
        dispatch = task.get("dispatch")
        if dispatch not in {"ready", "template", "coordinator-only"}:
            errors.append(f"{tid}: invalid dispatch {dispatch!r}")
        placeholders = [p for p in task.get("read", []) + task.get("write", []) if PLACEHOLDER_RE.search(p)]
        if placeholders and dispatch == "ready":
            errors.append(f"{tid}: READY card contains non-exact paths {placeholders}")
        legs = set(task.get("evidence_legs", []))
        if "Q1" in task.get("rows", []) and "oracle" not in legs:
            errors.append(f"{tid}: Q1 missing oracle evidence leg")
        if "Q2" in task.get("rows", []) and "real-client-agent" not in legs:
            errors.append(f"{tid}: Q2 missing real-client evidence leg")
        if any(r in task.get("rows", []) for r in ("O1","O2","O3")) and "performance" not in legs:
            errors.append(f"{tid}: O-row missing performance evidence leg")

        card = PLAN / "tasks" / f"{tid}.md"
        if not card.exists():
            errors.append(f"{tid}: missing card")
        else:
            text = card.read_text()
            st = status(tid)
            if st not in ALLOWED:
                errors.append(f"{tid}: invalid/missing status {st}")
            if st == "DONE":
                for label in ("CLAIMED", "BASELINE / RED", "IMPLEMENTED", "TESTING", "SELF-REVIEW", "INDEPENDENT REVIEW", "DONE"):
                    if not re.search(rf"^- \[x\] {re.escape(label)}(?: |$)", text, re.MULTILINE | re.IGNORECASE):
                        errors.append(f"{tid}: DONE status but checklist item not checked: {label}")
                if any(token in text for token in ("base_tree: UNSET", "diff_hash: UNSET", "verdict: pending", "next: claim this task")):
                    errors.append(f"{tid}: DONE status but closeout still has placeholders")
            for section in ("## Dispatch gate", "## Live checklist", "## Outcome", "## Owned write paths", "## Required evidence legs", "## Required validation", "## Closeout"):
                if section not in text:
                    errors.append(f"{tid}: missing section {section}")

    queue = deque([tid for tid, n in indegree.items() if n == 0])
    seen = 0
    while queue:
        tid = queue.popleft(); seen += 1
        for nxt in reverse[tid]:
            indegree[nxt] -= 1
            if indegree[nxt] == 0:
                queue.append(nxt)
    if seen != len(tasks):
        errors.append("dependency cycle detected")

    scheduled = []
    done_before = set()
    for batch in data.get("batches", []):
        members = batch.get("tasks", [])
        if not (1 <= len(members) <= 2):
            errors.append(f"{batch.get('id')}: must contain one or two tasks")
            continue
        for tid in members:
            if tid not in tasks:
                errors.append(f"{batch.get('id')}: unknown task {tid}")
                continue
            missing = set(tasks[tid].get("depends", [])) - done_before
            if missing:
                errors.append(f"{batch.get('id')} {tid}: prerequisites not in earlier batches {sorted(missing)}")
            scheduled.append(tid)
        if len(members) == 2 and all(tid in tasks for tid in members):
            reasons = conflict(tasks[members[0]], tasks[members[1]])
            if reasons:
                errors.append(f"{batch.get('id')}: incompatible pair {members}: {'; '.join(reasons)}")
        done_before.update(members)
    if sorted(scheduled) != sorted(tasks):
        missing = sorted(set(tasks) - set(scheduled))
        dupes = sorted({x for x in scheduled if scheduled.count(x) > 1})
        errors.append(f"batch coverage mismatch: missing={missing}, duplicates={dupes}")

    for md in PLAN.rglob("*.md"):
        text = md.read_text()
        for link in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
            if "://" in link or link.startswith("#"):
                continue
            if not (md.parent / link).resolve().exists():
                errors.append(f"broken link {md.relative_to(PLAN)} -> {link}")

    if errors:
        print("INVALID")
        for err in errors:
            print("ERROR:", err)
        return 1
    print(f"VALID: {len(tasks)} tasks, {len(data.get('batches', []))} batches, DAG/links/dispatch/locks pass")
    for warning in warnings:
        print("WARN:", warning)
    return 0


def ready() -> int:
    _, tasks = load()
    states = {tid: status(tid) for tid in tasks}
    active = [tid for tid, st in states.items() if st in {"CLAIMED", "IMPLEMENTING", "TESTING", "REVIEW"}]
    candidates = []
    for tid, task in tasks.items():
        if states[tid] != "QUEUED" or task.get("dispatch") != "ready":
            continue
        if all(states.get(dep) == "DONE" for dep in task.get("depends", [])):
            if any(conflict(task, tasks[a]) for a in active):
                continue
            candidates.append(tid)
    candidates.sort(key=lambda tid: (0 if tasks[tid]["priority"] == "P0" else 1, tasks[tid]["wave"], tid))
    print("Active:", ", ".join(f"{tid}:{states[tid]}" for tid in active) or "none")
    print("Ready:", ", ".join(candidates) or "none")
    pairs = []
    for i, a in enumerate(candidates):
        for b in candidates[i+1:]:
            if not conflict(tasks[a], tasks[b]):
                pairs.append((a, b))
    print("Compatible pairs:")
    for a, b in pairs[:12]:
        print(f"  {a} + {b}")
    if not pairs:
        print("  none")
    templates = [tid for tid,t in tasks.items() if states[tid]=="QUEUED" and t.get("dispatch")=="template" and all(states.get(d)=="DONE" for d in t.get("materialize_after",[]))]
    if templates:
        print("Templates ready for coordinator materialization:", ", ".join(sorted(templates)))
    return 0


def summary() -> int:
    data, tasks = load()
    counts = defaultdict(int)
    for tid in tasks:
        counts[status(tid)] += 1
    print(f"Tasks={len(tasks)} batches={len(data.get('batches', []))}")
    for key in sorted(counts):
        print(f"{key:14} {counts[key]}")
    blocked = [tid for tid in tasks if status(tid)=="BLOCKED"]
    if blocked:
        print("Blocked:", ", ".join(blocked))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["validate", "ready", "summary"])
    args = parser.parse_args()
    return {"validate": validate, "ready": ready, "summary": summary}[args.command]()

if __name__ == "__main__":
    sys.exit(main())
