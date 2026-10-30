"""Causal MCP steps and receipts shared by inventory and entity client probes."""
from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any, Callable


class ProbeSteps:
    def __init__(self, client: Any, artifact_dir: Path, timeout: float, scenario: str) -> None:
        self.client = client
        self.artifact_dir = artifact_dir
        self.deadline = time.monotonic() + timeout
        self.transcript: list[dict[str, Any]] = []
        self.report: dict[str, Any] = {
            "id": scenario, "passed": False, "assertions": {}, "fixture_commands": [],
        }

    def __enter__(self) -> ProbeSteps:
        return self

    def remaining(self) -> float:
        remaining = self.deadline - time.monotonic()
        if remaining < 0.1:
            raise TimeoutError(f"{self.report['id']} deadline expired")
        return min(remaining, 120.0)

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        self.remaining()
        entry: dict[str, Any] = {"tool": name, "arguments": arguments or {}}
        self.transcript.append(entry)
        try:
            result = self.client.call_tool(name, arguments or {})
            entry["result"] = result
            return result
        except Exception as error:
            entry["error"] = str(error)
            raise

    def observe(self, *, wait_for_play: bool = False) -> dict[str, Any]:
        state = self.call("minecraft_observe")
        if wait_for_play and state.get("in_play") is not True:
            # Only declared respawn/travel boundaries may await a rebuilt world.
            # The Play tool remains bounded and errors/timeouts still fail.
            self.call("minecraft_wait_for_play", {"timeout_seconds": self.remaining()})
            state = self.call("minecraft_observe")
        self.check("in_play", state.get("in_play") is True)
        return state

    def check(self, name: str, condition: bool) -> None:
        self.report["assertions"][name] = bool(condition)
        if not condition:
            raise RuntimeError(f"{self.report['id']} assertion failed: {name}")

    def wait(self, predicate: Callable[[dict[str, Any]], bool], label: str,
             *, wait_for_play: bool = False,
             timeout_seconds: float | None = None) -> dict[str, Any]:
        deadline = None if timeout_seconds is None else time.monotonic() + timeout_seconds
        try:
            while True:
                if deadline is not None and time.monotonic() >= deadline:
                    raise TimeoutError(f"{self.report['id']} timed out waiting for {label}")
                state = self.observe(wait_for_play=wait_for_play)
                if predicate(state):
                    self.check(label, True)
                    return state
                remaining = self.remaining()
                if deadline is not None:
                    remaining = min(remaining, deadline - time.monotonic())
                    if remaining <= 0:
                        continue
                self.call("minecraft_wait_for_state_change", {
                    "observed_version": state["state_version"], "timeout_seconds": remaining,
                })
        except TimeoutError:
            self.report["assertions"][label] = False
            raise

    def fixture(self, command: str) -> None:
        """Record privileged fixture setup separately from player action evidence."""
        self.report["fixture_commands"].append(command)
        self.call("minecraft_send_chat", {"message": command, "command": True})

    def __exit__(self, kind: Any, error: Any, traceback: Any) -> bool:
        if error is None:
            self.report["passed"] = True
        else:
            self.report["error"] = f"{type(error).__name__}: {error}"
        self.artifact_dir.mkdir(parents=True, exist_ok=True)
        token = getattr(self.client, "token", "")

        def redact(value: Any) -> Any:
            if isinstance(value, dict):
                return {key: redact(item) for key, item in value.items()
                        if "token" not in key.lower() and "authorization" not in key.lower()}
            if isinstance(value, list):
                return [redact(item) for item in value]
            if isinstance(value, str) and token:
                return value.replace(token, "[REDACTED]")
            return value

        self.report = redact(self.report)
        (self.artifact_dir / "scenario.json").write_text(json.dumps(self.report, indent=2) + "\n")
        (self.artifact_dir / "transcript.json").write_text(json.dumps(redact(self.transcript), indent=2) + "\n")
        return False


def item_count(state: dict[str, Any], item_id: str) -> int:
    return sum(int(stack["count"]) for stack in state["inventory"] if stack["item_id"] == item_id)


def container_slot(state: dict[str, Any], slot: int) -> dict[str, Any]:
    container = state["container"]
    if not 0 <= slot < container["slot_count"]:
        raise RuntimeError(f"unknown container slot: {slot}")
    # The canonical observation omits empty stacks, not unknown slots.
    return next((stack for stack in container["slots"] if stack["slot"] == slot),
                {"slot": slot, "item_id": "minecraft:air", "count": 0})
