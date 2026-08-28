#!/usr/bin/env python3
"""Fail-closed validation for runner-managed real-client restart evidence."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Callable

RESTART_PREREQUISITES: dict[str, tuple[str, ...]] = {
    "m94-06-save-restart-after": ("m94-06-save-restart-before",),
    "playable-03-save-restart-after": (
        "playable-03-save-restart-before",
        "playable-03-save-restart-rejoin",
        "playable-04-twenty-minute-survival-loop",
    ),
    "playable-06-stone-tool-save-restart-after": (
        "playable-06-stone-tool-save-restart-before",
    ),
    "playable-13-chest-storage-save-restart-after": (
        "playable-13-chest-storage-save-restart-before",
    ),
    "playable-25-iron-sword-save-restart-after": (
        "playable-25-iron-sword-save-restart-before",
    ),
    "playable-29-iron-chestplate-save-restart-mitigation-after": (
        "playable-29-iron-chestplate-save-restart-mitigation-before",
    ),
    "playable-45-two-client-shared-chest-save-restart-after": (
        "playable-45-two-client-shared-chest-save-restart-before",
    ),
    "playable-46-generated-ruin-cache-after": (
        "playable-46-generated-ruin-cache-before",
    ),
}

TWO_CLIENT_RESTART_AFTER = "playable-45-two-client-shared-chest-save-restart-after"

REQUIRED_REPORT_OBSERVATION_FRAGMENTS: dict[str, tuple[str, ...]] = {
    "playable-04-twenty-minute-survival-loop": (
        "natural spawn acceptance: passed",
        "passive_observed=true",
        "hostile_observed=true",
        "20-minute survival soak: passed",
    ),
    "playable-03-save-restart-after": (
        "restart marker persistence: passed",
        "inventory persistence: passed",
    ),
    "playable-06-stone-tool-save-restart-after": (
        "restart marker persistence: passed",
        "stone inventory persistence: passed",
    ),
    "playable-13-chest-storage-save-restart-after": (
        "chest block persistence: passed",
        "chest storage persistence: passed",
    ),
    "playable-25-iron-sword-save-restart-after": (
        "restart marker persistence: passed",
        "iron sword inventory persistence: passed",
    ),
    "playable-29-iron-chestplate-save-restart-mitigation-after": (
        "restart marker persistence: passed",
        "iron chestplate armor persistence: passed",
    ),
    "playable-46-generated-ruin-cache-before": (
        "generated ruin center approach: passed",
        "exact generated chest Y: passed",
        "generated ruin chest open client-state: passed",
        "generated ruin loot quick-move: passed",
        "generated ruin chest close client-state: passed",
    ),
    "playable-46-generated-ruin-cache-after": (
        "generated ruin chest persistence client-state: passed",
        "generated ruin cache persistence: passed",
    ),
}


def fail(message: str) -> None:
    raise ValueError(message)


def load_json_object(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"{label} is unreadable JSON: {exc}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def successful_commands(
    scenario: dict[str, Any], command: str, actor: str | None = None
) -> list[dict[str, Any]]:
    commands = scenario.get("commands")
    if not isinstance(commands, list):
        return []
    matches: list[dict[str, Any]] = []
    for entry in commands:
        if not isinstance(entry, dict) or entry.get("command") != command:
            continue
        if actor is not None and entry.get("client") != actor:
            continue
        if "error" in entry or not isinstance(entry.get("response"), dict):
            continue
        matches.append(entry)
    return matches


def require_agent_report(scenario_id: str, scenario: dict[str, Any]) -> dict[str, Any]:
    report = scenario.get("agent_report")
    if not isinstance(report, dict):
        fail(f"{scenario_id} must include a structured agent_report")
    if report.get("id") != scenario_id:
        fail(f"{scenario_id} agent_report id must match the observed scenario")
    if report.get("result") != "passed":
        fail(f"{scenario_id} agent_report result must be passed")
    observations = report.get("observations")
    if not isinstance(observations, list) or not observations:
        fail(f"{scenario_id} agent_report observations must be non-empty")
    if any(not isinstance(observation, str) or not observation.strip() for observation in observations):
        fail(f"{scenario_id} agent_report observations must be non-empty strings")
    required_fragments = REQUIRED_REPORT_OBSERVATION_FRAGMENTS.get(scenario_id, ())
    for fragment in required_fragments:
        if not any(fragment in observation for observation in observations):
            fail(
                f"{scenario_id} agent_report is missing required acceptance observation: {fragment}"
            )
    return report


def find_after(
    lines: list[str], start: int, predicate: Callable[[str], bool]
) -> int | None:
    for index in range(start + 1, len(lines)):
        if predicate(lines[index]):
            return index
    return None


def find_before(
    lines: list[str], end: int, predicate: Callable[[str], bool]
) -> int | None:
    for index in range(end - 1, -1, -1):
        if predicate(lines[index]):
            return index
    return None


def require_ready_server_before_phase(lines: list[str], before_status_index: int) -> None:
    start_index = find_before(
        lines, before_status_index, lambda line: line.startswith("server_start_phase=")
    )
    if start_index is None:
        fail("before-restart phase has no preceding runner server_start_phase")
    start_phase = lines[start_index].split("=", 1)[1]
    pid_prefix = f"server_pid_{start_phase}="
    pid_index = find_after(
        lines,
        start_index,
        lambda line: line.startswith(pid_prefix) and line[len(pid_prefix) :].isdigit(),
    )
    if pid_index is None or pid_index >= before_status_index:
        fail(f"before-restart server phase {start_phase} has no recorded process pid")
    ready_line = f"server_ready_phase={start_phase} status=ready"
    ready_index = find_after(lines, pid_index, lambda line: line == ready_line)
    if ready_index is None or ready_index >= before_status_index:
        fail(f"before-restart server phase {start_phase} was not ready before client evidence")


def require_restart_sequence(
    lines: list[str], before_scenario: str, after_scenario: str
) -> None:
    before_status = f"client_agent_phase_exit_status_{before_scenario}=0"
    after_status = f"client_agent_phase_exit_status_{after_scenario}=0"
    try:
        before_status_index = lines.index(before_status)
    except ValueError:
        fail(f"automation-driver.txt is missing passed before phase {before_status}")

    require_ready_server_before_phase(lines, before_status_index)

    stop_index = find_after(
        lines,
        before_status_index,
        lambda line: line.startswith("server_stop_phase=") and line.endswith(" signal=INT"),
    )
    if stop_index is None:
        fail(f"restart stop must occur after passed before phase {before_scenario}")
    exit_index = find_after(
        lines,
        stop_index,
        lambda line: line.startswith("server_exit_phase=") and line.endswith(" status=0"),
    )
    if exit_index is None:
        fail(f"clean server exit must occur after restart stop for {after_scenario}")
    restart_count_index = find_after(
        lines,
        exit_index,
        lambda line: line.startswith("server_restart_count=")
        and line.split("=", 1)[1].isdigit()
        and int(line.split("=", 1)[1]) >= 1,
    )
    if restart_count_index is None:
        fail(f"server_restart_count>=1 must follow the clean server exit for {after_scenario}")
    start_index = find_after(
        lines,
        restart_count_index,
        lambda line: line.startswith("server_start_phase=")
        and "after" in line.split("=", 1)[1],
    )
    if start_index is None:
        fail(f"after-restart server start must follow restart count for {after_scenario}")
    start_phase = lines[start_index].split("=", 1)[1]
    pid_prefix = f"server_pid_{start_phase}="
    pid_index = find_after(
        lines,
        start_index,
        lambda line: line.startswith(pid_prefix) and line[len(pid_prefix) :].isdigit(),
    )
    if pid_index is None:
        fail(f"after-restart server process pid is missing for phase {start_phase}")
    ready_line = f"server_ready_phase={start_phase} status=ready"
    ready_index = find_after(lines, pid_index, lambda line: line == ready_line)
    if ready_index is None:
        fail(f"after-restart server phase {start_phase} never recorded readiness")
    after_status_index = find_after(lines, ready_index, lambda line: line == after_status)
    if after_status_index is None:
        fail(f"passed after phase {after_scenario} must occur after restarted server readiness")


def validate(
    automation_driver_path: Path, observations_path: Path, manifest_path: Path
) -> None:
    try:
        lines = automation_driver_path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        fail(f"automation-driver.txt is unreadable: {exc}")
    observations = load_json_object(observations_path, "observations.json")
    manifest = load_json_object(manifest_path, "manifest.json")
    manifest_ids = {
        scenario.get("id")
        for scenario in manifest.get("scenarios", [])
        if isinstance(scenario, dict) and isinstance(scenario.get("id"), str)
    }

    passed_entries: dict[str, dict[str, Any]] = {}
    passed_order: list[str] = []
    scenarios = observations.get("scenarios")
    if not isinstance(scenarios, list):
        fail("observations.json scenarios must be a list")
    for scenario in scenarios:
        if not isinstance(scenario, dict) or scenario.get("result") != "passed":
            continue
        scenario_id = scenario.get("id")
        if not isinstance(scenario_id, str) or scenario_id not in manifest_ids:
            continue
        if scenario_id in passed_entries:
            fail(f"observations.json contains duplicate passed scenario id: {scenario_id}")
        passed_entries[scenario_id] = scenario
        passed_order.append(scenario_id)

    for after_scenario, before_candidates in RESTART_PREREQUISITES.items():
        if after_scenario not in passed_entries:
            continue
        matched_before = [
            scenario_id for scenario_id in passed_order if scenario_id in before_candidates
        ]
        if not matched_before:
            fail(
                f"observations.json must include a passed before-restart scenario for "
                f"{after_scenario}: one of {', '.join(sorted(before_candidates))}"
            )
        before_scenario = matched_before[-1]
        if passed_order.index(before_scenario) >= passed_order.index(after_scenario):
            fail(
                f"passed before-restart scenario {before_scenario} must precede "
                f"{after_scenario} in observations.json"
            )

        before_entry = passed_entries[before_scenario]
        after_entry = passed_entries[after_scenario]
        before_report = require_agent_report(before_scenario, before_entry)
        after_report = require_agent_report(after_scenario, after_entry)

        if after_scenario == TWO_CLIENT_RESTART_AFTER:
            for actor in ("primary", "secondary"):
                if not successful_commands(before_entry, "disconnect", actor):
                    fail(
                        f"{before_scenario} must record a successful {actor} disconnect before restart"
                    )
                if not successful_commands(after_entry, "connect", actor):
                    fail(
                        f"{after_scenario} must record a successful {actor} connect after restart"
                    )
            invariant_validation = after_report.get("restart_invariant_validation")
            if not isinstance(invariant_validation, dict) or invariant_validation.get("status") != "passed":
                fail(
                    f"{after_scenario} must include passed restart_invariant_validation"
                )
        else:
            if not successful_commands(before_entry, "disconnect"):
                fail(f"{before_scenario} must record a successful disconnect before restart")
            if not successful_commands(after_entry, "connect"):
                fail(f"{after_scenario} must record a successful connect after restart")

        if before_report.get("result") != "passed" or after_report.get("result") != "passed":
            fail("restart phases must both have passed client-side scenario reports")
        require_restart_sequence(lines, before_scenario, after_scenario)


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(
            "usage: validate-real-client-restart-evidence.py "
            "AUTOMATION_DRIVER OBSERVATIONS MANIFEST",
            file=sys.stderr,
        )
        return 2
    try:
        validate(Path(argv[1]), Path(argv[2]), Path(argv[3]))
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
