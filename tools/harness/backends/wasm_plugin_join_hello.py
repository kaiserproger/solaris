"""Real graphical P2 join/hello acceptance through existing bridge primitives.

Chat waits use the client's versioned packet/lifecycle notifications. Repeated
commands and an unregistered root form the bounded exploratory pass; screenshots
and the observed chat are retained by the canonical regression runner.
"""

from __future__ import annotations

import json
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

WASM_PLUGIN_JOIN_HELLO_SCENARIOS = frozenset({"wasm-p2-plugin-join-hello"})


@dataclass(frozen=True)
class HarnessHooks:
    """Existing driver helpers, shared in script and imported execution modes."""

    call: Callable[..., dict[str, Any]]
    capture_screenshot: Callable[..., str]
    connect_or_confirm_play: Callable[..., dict[str, Any]]
    is_in_play: Callable[[dict[str, Any]], bool]


def run_wasm_plugin_join_hello_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: HarnessHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [entry for entry in manifest["scenarios"] if entry["id"] == scenario_id]
    if len(scenarios) != 1 or scenarios[0].get("no_debug_commands") is not True:
        raise ValueError("P2 requires one declared no-debug scenario")
    expected = scenarios[0]["wasm_plugin_expectation"]
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    checks: list[dict[str, Any]] = []

    def call(command: str, payload: dict[str, Any], timeout: float = timeout_seconds):
        return hooks.call(client, transcript, command, payload, timeout)

    def wait_chat(predicate: Callable[[list[str]], bool], label: str) -> dict[str, Any]:
        deadline = time.monotonic() + min(timeout_seconds, 30.0)
        while True:
            observation = call("observe", {})
            if not hooks.is_in_play(observation):
                raise RuntimeError(f"client left Play while waiting for {label}")
            lines = observation["recent_chat"]
            if predicate(lines):
                checks.append({"check": label, "passed": True, "recent_chat": lines})
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"missing {label}; observed chat: {lines!r}")
            # observe captures chat and state_version on the same client thread.
            # If a packet arrived since, this returns immediately; otherwise the
            # packet producer wakes it. No polling interval or tick wait is used.
            call(
                "wait_state_change",
                {"observed_version": observation["state_version"], "timeout_seconds": remaining},
                remaining + 1.0,
            )

    call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    observation = call("observe", {})
    player = observation["player"]["name"]
    if player != expected["player_username"]:
        raise RuntimeError(f"unexpected client identity: {player!r}")
    greeting = expected["join_greeting_template"].replace("{player}", player)
    observation = wait_chat(lambda lines: greeting in lines, "external WASM join greeting")

    response = expected["command_response"]
    for attempt in (1, 2):
        baseline = observation["recent_chat"].count(response)
        call("send_chat", {"message": expected["player_command"], "command": True})
        observation = wait_chat(
            lambda lines: lines.count(response) > baseline,
            f"ordinary /{expected['player_command']} answer {attempt}",
        )
    screenshots = [hooks.capture_screenshot(
        client, transcript, run_dir, screenshots_dir,
        f"{scenario_id}-greeting-and-hello", timeout_seconds,
    )]

    baseline = Counter(observation["recent_chat"])
    call("send_chat", {"message": expected["unknown_command_probe"], "command": True})

    def fresh_rejection(lines: list[str]) -> bool:
        return any(
            count > baseline[line]
            and any(marker in line for marker in expected["unknown_command_feedback"])
            for line, count in Counter(lines).items()
        )

    wait_chat(fresh_rejection, "unregistered command receives fresh server rejection")
    screenshots.append(hooks.capture_screenshot(
        client, transcript, run_dir, screenshots_dir,
        f"{scenario_id}-unregistered-command", timeout_seconds,
    ))
    final_state = call("state", {})
    call("disconnect", {})
    return "passed", final_state, screenshots, {
        "id": scenario_id,
        "result": "passed",
        "observations": [check["check"] for check in checks],
        "checks": checks,
        "expectation": expected,
        "exploratory_checks": ["repeated ordinary command", "unregistered command rejection"],
    }
