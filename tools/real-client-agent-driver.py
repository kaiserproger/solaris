#!/usr/bin/env python3
"""Drive a Solaris real-client bridge and write fail-closed observations."""

from __future__ import annotations

import argparse
import binascii
import ctypes
import hashlib
import json
import os
import platform
import re
import select
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib import error, parse, request


CORE_REPLAY_SCENARIO_SCHEMA = "solaris.core_replay.scenario.v1"
CORE_REPLAY_RESULT_SCHEMA = "solaris.core_replay.result.v1"
CORE_REPLAY_DRIVERS = {"solaris_protocol", "vanilla_oracle", "real_client"}
CORE_REPLAY_EVIDENCE_KINDS = {
    "unit",
    "harness",
    "oracle",
    "real_client",
    "performance",
    "soak",
}
CORE_REPLAY_IDENTIFIER = re.compile(r"[a-z0-9][a-z0-9._-]{0,127}\Z")
MAX_CORE_REPLAY_ACTIONS = 10_000
MAX_CORE_REPLAY_CHECKS = 128
RESTART_INVARIANT_SCHEMA = "solaris.restart_invariants.v1"
RESTART_INVARIANT_FILE = "restart-invariants.json"
RESTART_INVARIANT_CATEGORIES = {"player", "inventory", "world", "container", "entity", "time"}
RESTART_INVARIANT_TYPES = {"string", "integer", "boolean", "record"}
RESTART_INVARIANT_MODES = {"stable", "transition"}
MAX_RESTART_INVARIANTS = 32
MAX_RESTART_SNAPSHOT_BYTES = 32 * 1024
MAX_RESTART_MARKER_BYTES = 4 * 1024
P45_RESTART_BEFORE_SCENARIO = "playable-45-two-client-shared-chest-save-restart-before"
P45_RESTART_AFTER_SCENARIO = "playable-45-two-client-shared-chest-save-restart-after"
P45_SHARED_CHEST_MARKER = "playable-31-shared-chest-marker.properties"
INOTIFY_MASK = 0x00000002 | 0x00000004 | 0x00000008 | 0x00000080 | 0x00000100


def wait_for_path_condition(path: Path, timeout_seconds: float, condition: Any) -> Any:
    parent = path.parent
    parent.mkdir(parents=True, exist_ok=True)
    libc = ctypes.CDLL(None, use_errno=True)
    inotify_init1 = libc.inotify_init1
    inotify_init1.argtypes = [ctypes.c_int]
    inotify_init1.restype = ctypes.c_int
    inotify_add_watch = libc.inotify_add_watch
    inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    inotify_add_watch.restype = ctypes.c_int

    descriptor = inotify_init1(os.O_CLOEXEC)
    if descriptor < 0:
        error_number = ctypes.get_errno()
        raise OSError(error_number, os.strerror(error_number))
    try:
        watch = inotify_add_watch(
            descriptor,
            os.fsencode(parent),
            INOTIFY_MASK,
        )
        if watch < 0:
            error_number = ctypes.get_errno()
            raise OSError(error_number, os.strerror(error_number), parent)

        deadline = time.monotonic() + timeout_seconds
        while True:
            matched, value = condition()
            if matched:
                return value
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"filesystem event timeout for {path}")
            readable, _, _ = select.select([descriptor], [], [], remaining)
            if not readable:
                raise TimeoutError(f"filesystem event timeout for {path}")
            os.read(descriptor, 64 * 1024)
    finally:
        os.close(descriptor)


class AgentClient:
    def __init__(self, bridge_url: str, secret: str) -> None:
        self.bridge_url = validate_loopback_url(bridge_url)
        self.secret = secret
        self.next_id = 1

    def call(self, command: str, payload: dict[str, Any], timeout_seconds: float) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        body = json.dumps(
            {
                "id": request_id,
                "secret": self.secret,
                "command": command,
                "payload": payload,
            },
            separators=(",", ":"),
        ).encode("utf-8")
        rpc_request = request.Request(
            self.bridge_url,
            data=body,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with request.urlopen(rpc_request, timeout=timeout_seconds) as response:
                decoded = json.loads(response.read().decode("utf-8"))
        except error.HTTPError as exc:
            raise RuntimeError(structured_http_error(command, request_id, exc)) from exc
        except (error.URLError, TimeoutError, json.JSONDecodeError) as exc:
            raise RuntimeError(f"{command} failed: {exc}") from exc

        if decoded.get("id") != request_id:
            raise RuntimeError(
                f"{command} failed: response id {decoded.get('id')} did not match request id {request_id}"
            )
        if not decoded.get("ok"):
            bridge_error = decoded.get("error") or {}
            code = bridge_error.get("code", "agent-error")
            message = bridge_error.get("message", "")
            raise RuntimeError(f"{command} failed: {code}: {message}")
        payload_value = decoded.get("payload")
        if not isinstance(payload_value, dict):
            raise RuntimeError(f"{command} failed: response payload was not an object")
        return payload_value


def structured_http_error(command: str, request_id: int, exc: error.HTTPError) -> str:
    try:
        decoded = json.loads(exc.read().decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return f"{command} failed: HTTP {exc.code}: {exc.reason}"

    if decoded.get("id") != request_id:
        return f"{command} failed: HTTP {exc.code}: response id {decoded.get('id')} did not match request id {request_id}"
    bridge_error = decoded.get("error") or {}
    code = bridge_error.get("code", "agent-error")
    message = bridge_error.get("message", "")
    return f"{command} failed: {code}: {message}"


def validate_loopback_url(bridge_url: str) -> str:
    parsed = parse.urlparse(bridge_url)
    if parsed.scheme != "http":
        raise ValueError("bridge URL must use http")
    if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("bridge URL must target loopback")
    if parsed.port is None:
        raise ValueError("bridge URL must include a port")
    if not parsed.path:
        parsed = parsed._replace(path="/")
    return parse.urlunparse(parsed)


def validate_loopback_server_addr(server_addr: str) -> str:
    host, port = parse_server_addr(server_addr)
    normalized_host = host.lower()
    if normalized_host not in {"127.0.0.1", "localhost", "::1"}:
        raise ValueError("server_addr must target loopback")
    if normalized_host == "::1":
        return f"[::1]:{port}"
    return f"{normalized_host}:{port}"


def parse_server_addr(server_addr: str) -> tuple[str, int]:
    if not server_addr or any(character.isspace() for character in server_addr):
        raise ValueError("server_addr must be host:port")
    if server_addr.startswith("["):
        bracket_end = server_addr.find("]")
        if bracket_end <= 1 or bracket_end + 2 > len(server_addr):
            raise ValueError("server_addr must be host:port")
        if server_addr[bracket_end + 1] != ":":
            raise ValueError("server_addr must be host:port")
        host = server_addr[1:bracket_end]
        port_text = server_addr[bracket_end + 2 :]
    else:
        if server_addr.count(":") != 1:
            raise ValueError("server_addr must be host:port")
        host, port_text = server_addr.rsplit(":", 1)
    if not host or not port_text:
        raise ValueError("server_addr must be host:port")
    try:
        port = int(port_text, 10)
    except ValueError as exc:
        raise ValueError("server_addr port must be numeric") from exc
    if port < 1 or port > 65535:
        raise ValueError("server_addr port must be between 1 and 65535")
    return host, port


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def require_restart_scalar(value: Any, label: str) -> None:
    if isinstance(value, bool):
        return
    if isinstance(value, int):
        return
    if isinstance(value, str) and value and len(value) <= 256:
        return
    raise ValueError(f"{label} must be a bounded string, integer, or boolean")


def validate_restart_typed_value(type_name: str, value: Any, label: str) -> None:
    if type_name == "string":
        if not isinstance(value, str) or not value or len(value) > 256:
            raise ValueError(f"{label} must be a non-empty bounded string")
        return
    if type_name == "integer":
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError(f"{label} must be an integer")
        return
    if type_name == "boolean":
        if not isinstance(value, bool):
            raise ValueError(f"{label} must be a boolean")
        return
    if type_name == "record":
        require_object(value, label)
        if not value or len(value) > 16:
            raise ValueError(f"{label} must contain 1..16 fields")
        for key, field in value.items():
            if not isinstance(key, str) or not CORE_REPLAY_IDENTIFIER.fullmatch(key):
                raise ValueError(f"{label} has invalid field name {key!r}")
            require_restart_scalar(field, f"{label}.{key}")
        return
    raise ValueError(f"{label} has unsupported type {type_name!r}")


def validate_restart_invariant_snapshot(value: Any) -> dict[str, Any]:
    require_object(value, "restart invariant snapshot")
    require_exact_fields(
        value,
        {"schema", "producer_scenario", "created_at", "invariants"},
        "restart invariant snapshot",
    )
    if value["schema"] != RESTART_INVARIANT_SCHEMA:
        raise ValueError(f"unsupported restart invariant schema: {value['schema']!r}")
    require_identifier(value["producer_scenario"], "restart invariant producer scenario")
    if not isinstance(value["created_at"], str) or not value["created_at"]:
        raise ValueError("restart invariant created_at must be a non-empty string")
    invariants = value["invariants"]
    if not isinstance(invariants, list) or not invariants or len(invariants) > MAX_RESTART_INVARIANTS:
        raise ValueError(f"restart invariants must contain 1..{MAX_RESTART_INVARIANTS} entries")
    seen: set[str] = set()
    for index, invariant in enumerate(invariants):
        label = f"restart invariant {index}"
        require_object(invariant, label)
        require_exact_fields(
            invariant,
            {"id", "category", "type", "mode", "before", "expected_after"},
            label,
        )
        require_identifier(invariant["id"], f"{label} id")
        if invariant["id"] in seen:
            raise ValueError(f"duplicate restart invariant id {invariant['id']}")
        seen.add(invariant["id"])
        if invariant["category"] not in RESTART_INVARIANT_CATEGORIES:
            raise ValueError(f"{label} has unsupported category {invariant['category']!r}")
        if invariant["type"] not in RESTART_INVARIANT_TYPES:
            raise ValueError(f"{label} has unsupported type {invariant['type']!r}")
        if invariant["mode"] not in RESTART_INVARIANT_MODES:
            raise ValueError(f"{label} has unsupported mode {invariant['mode']!r}")
        validate_restart_typed_value(invariant["type"], invariant["before"], f"{label}.before")
        validate_restart_typed_value(
            invariant["type"], invariant["expected_after"], f"{label}.expected_after"
        )
    return value


def restart_invariant_path(run_dir: Path) -> Path:
    return run_dir / RESTART_INVARIANT_FILE


def write_restart_invariant_snapshot(run_dir: Path, snapshot: dict[str, Any]) -> None:
    validate_restart_invariant_snapshot(snapshot)
    encoded = (json.dumps(snapshot, indent=2, sort_keys=True) + "\n").encode("utf-8")
    if len(encoded) > MAX_RESTART_SNAPSHOT_BYTES:
        raise ValueError("restart invariant snapshot exceeds the bounded size limit")
    path = restart_invariant_path(run_dir)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_bytes(encoded)
    os.replace(temporary, path)


def load_restart_invariant_snapshot(run_dir: Path, expected_scenario: str) -> dict[str, Any]:
    path = restart_invariant_path(run_dir)
    if not path.is_file():
        raise RuntimeError(f"missing restart invariant snapshot: {path}")
    size = path.stat().st_size
    if size <= 0 or size > MAX_RESTART_SNAPSHOT_BYTES:
        raise RuntimeError(f"restart invariant snapshot has invalid size {size}: {path}")
    try:
        snapshot = json.loads(path.read_text(encoding="utf-8"))
        validate_restart_invariant_snapshot(snapshot)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise RuntimeError(f"invalid restart invariant snapshot {path}: {exc}") from exc
    if snapshot["producer_scenario"] != expected_scenario:
        raise RuntimeError(
            f"restart invariant snapshot producer {snapshot['producer_scenario']!r} "
            f"does not match {expected_scenario!r}"
        )
    return snapshot


def load_p45_shared_chest_marker(run_dir: Path) -> dict[str, Any]:
    path = run_dir / P45_SHARED_CHEST_MARKER
    if not path.is_file():
        raise RuntimeError(f"missing P45 shared chest marker: {path}")
    size = path.stat().st_size
    if size <= 0 or size > MAX_RESTART_MARKER_BYTES:
        raise RuntimeError(f"P45 shared chest marker has invalid size {size}: {path}")
    values: dict[str, str] = {}
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        if not raw_line:
            continue
        if "=" not in raw_line:
            raise RuntimeError(f"invalid P45 shared chest marker line: {raw_line!r}")
        key, value = raw_line.split("=", 1)
        if key in values:
            raise RuntimeError(f"duplicate P45 shared chest marker field: {key}")
        values[key] = value
    required = {"x", "y", "z", "face", "item", "count"}
    if set(values) != required:
        missing = sorted(required - set(values))
        extra = sorted(set(values) - required)
        raise RuntimeError(f"invalid P45 shared chest marker fields: missing={missing} extra={extra}")
    try:
        marker = {
            "x": int(values["x"]),
            "y": int(values["y"]),
            "z": int(values["z"]),
            "face": values["face"],
            "item": values["item"],
            "count": int(values["count"]),
        }
    except ValueError as exc:
        raise RuntimeError(f"invalid numeric P45 shared chest marker field: {path}") from exc
    if not marker["face"] or len(marker["face"]) > 32:
        raise RuntimeError("P45 shared chest marker face must be a bounded non-empty string")
    if not re.fullmatch(r"[a-z0-9_.-]+:[a-z0-9_./-]+", marker["item"]):
        raise RuntimeError(f"P45 shared chest marker has invalid item id {marker['item']!r}")
    if marker["count"] < 1 or marker["count"] > 64:
        raise RuntimeError(f"P45 shared chest marker count is outside 1..64: {marker['count']}")
    return marker


def require_play_dimension(state: dict[str, Any], actor: str) -> str:
    require_object(state, f"{actor} client state")
    if state.get("in_play") is not True:
        raise RuntimeError(f"{actor} client is not in Play while capturing restart invariants")
    dimension = state.get("dimension")
    if not isinstance(dimension, str) or not dimension:
        raise RuntimeError(f"{actor} client state is missing dimension")
    return dimension


def p45_restart_snapshot(
    primary_state: dict[str, Any], secondary_state: dict[str, Any], marker: dict[str, Any]
) -> dict[str, Any]:
    primary_dimension = require_play_dimension(primary_state, "primary")
    secondary_dimension = require_play_dimension(secondary_state, "secondary")
    stack = {"item": marker["item"], "count": marker["count"]}
    marker_record = {
        "x": marker["x"],
        "y": marker["y"],
        "z": marker["z"],
        "face": marker["face"],
        "item": marker["item"],
        "count": marker["count"],
    }
    snapshot = {
        "schema": RESTART_INVARIANT_SCHEMA,
        "producer_scenario": P45_RESTART_BEFORE_SCENARIO,
        "created_at": utc_now(),
        "invariants": [
            {
                "id": "player.primary.dimension",
                "category": "player",
                "type": "string",
                "mode": "stable",
                "before": primary_dimension,
                "expected_after": primary_dimension,
            },
            {
                "id": "player.secondary.dimension",
                "category": "player",
                "type": "string",
                "mode": "stable",
                "before": secondary_dimension,
                "expected_after": secondary_dimension,
            },
            {
                "id": "inventory.deposited_stack",
                "category": "inventory",
                "type": "record",
                "mode": "transition",
                "before": stack,
                "expected_after": {
                    "owner": "secondary",
                    "item": marker["item"],
                    "count": marker["count"],
                },
            },
            {
                "id": "world.shared_chest_marker",
                "category": "world",
                "type": "record",
                "mode": "stable",
                "before": marker_record,
                "expected_after": marker_record,
            },
            {
                "id": "container.shared_chest.slot0",
                "category": "container",
                "type": "record",
                "mode": "transition",
                "before": {"slot": 0, "item": marker["item"], "count": marker["count"]},
                "expected_after": {"slot": 0, "empty": True},
            },
            {
                "id": "entity.shared_chest.block",
                "category": "entity",
                "type": "string",
                "mode": "stable",
                "before": "minecraft:chest",
                "expected_after": "minecraft:chest",
            },
        ],
    }
    return validate_restart_invariant_snapshot(snapshot)


def restart_invariant_index(snapshot: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {invariant["id"]: invariant for invariant in snapshot["invariants"]}


def p45_marker_record(marker: dict[str, Any]) -> dict[str, Any]:
    return {
        "x": marker["x"],
        "y": marker["y"],
        "z": marker["z"],
        "face": marker["face"],
        "item": marker["item"],
        "count": marker["count"],
    }


def validate_p45_restart_preflight(
    snapshot: dict[str, Any], marker: dict[str, Any]
) -> dict[str, dict[str, Any]]:
    invariants = restart_invariant_index(snapshot)
    expected_ids = {
        "player.primary.dimension",
        "player.secondary.dimension",
        "inventory.deposited_stack",
        "world.shared_chest_marker",
        "container.shared_chest.slot0",
        "entity.shared_chest.block",
    }
    if set(invariants) != expected_ids:
        raise RuntimeError(
            "P45 restart invariant ids do not match the declared scenario contract: "
            f"missing={sorted(expected_ids - set(invariants))} extra={sorted(set(invariants) - expected_ids)}"
        )
    actual_marker = p45_marker_record(marker)
    expected_marker = invariants["world.shared_chest_marker"]["expected_after"]
    if actual_marker != expected_marker:
        raise RuntimeError(
            "restart invariant world.shared_chest_marker mismatch: "
            f"expected={expected_marker!r} actual={actual_marker!r}"
        )
    return invariants


def validate_p45_restart_after(
    snapshot: dict[str, Any],
    primary_state: dict[str, Any],
    secondary_state: dict[str, Any],
    marker: dict[str, Any],
    secondary_withdraw_result: str,
    primary_empty_result: str,
) -> list[dict[str, Any]]:
    invariants = validate_p45_restart_preflight(snapshot, marker)
    primary_dimension = require_play_dimension(primary_state, "primary")
    secondary_dimension = require_play_dimension(secondary_state, "secondary")
    actual_marker = p45_marker_record(marker)
    checks = [
        (
            "player.primary.dimension",
            primary_dimension,
            invariants["player.primary.dimension"]["expected_after"],
        ),
        (
            "player.secondary.dimension",
            secondary_dimension,
            invariants["player.secondary.dimension"]["expected_after"],
        ),
        (
            "world.shared_chest_marker",
            actual_marker,
            invariants["world.shared_chest_marker"]["expected_after"],
        ),
        (
            "entity.shared_chest.block",
            "minecraft:chest",
            invariants["entity.shared_chest.block"]["expected_after"],
        ),
    ]
    results: list[dict[str, Any]] = []
    for invariant_id, actual, expected in checks:
        if actual != expected:
            raise RuntimeError(
                f"restart invariant {invariant_id} mismatch: expected={expected!r} actual={actual!r}"
            )
        results.append({"id": invariant_id, "status": "passed", "actual": actual})

    deposited = invariants["inventory.deposited_stack"]
    expected_stack = deposited["before"]
    if expected_stack != {"item": marker["item"], "count": marker["count"]}:
        raise RuntimeError("restart inventory invariant no longer matches the shared chest marker")
    if secondary_withdraw_result != "passed":
        raise RuntimeError("restart inventory invariant failed: secondary withdraw did not pass")
    results.append(
        {
            "id": "inventory.deposited_stack",
            "status": "passed",
            "actual": deposited["expected_after"],
        }
    )

    container = invariants["container.shared_chest.slot0"]
    if container["before"] != {"slot": 0, "item": marker["item"], "count": marker["count"]}:
        raise RuntimeError("restart container invariant no longer matches the shared chest marker")
    if secondary_withdraw_result != "passed" or primary_empty_result != "passed":
        raise RuntimeError("restart container invariant failed: withdraw/empty observation did not pass")
    results.append(
        {
            "id": "container.shared_chest.slot0",
            "status": "passed",
            "actual": container["expected_after"],
        }
    )
    return results


def load_core_replay_manifest(path: Path, expected_scenario_id: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"failed to read core replay manifest {path}: {exc}") from exc
    require_object(value, "core replay manifest")
    require_exact_fields(
        value,
        {"schema", "id", "seed", "actions", "lanes", "expected_invariants"},
        "core replay manifest",
    )
    if value["schema"] != CORE_REPLAY_SCENARIO_SCHEMA:
        raise ValueError(f"unsupported core replay schema: {value['schema']!r}")
    require_identifier(value["id"], "core replay scenario id")
    if value["id"] != expected_scenario_id:
        raise ValueError(
            f"core replay manifest id {value['id']!r} does not match scenario {expected_scenario_id!r}"
        )
    require_bounded_int(value["seed"], "core replay seed", 0, (1 << 64) - 1)

    actions = value["actions"]
    if not isinstance(actions, list) or not actions:
        raise ValueError("core replay actions must be a non-empty array")
    if len(actions) > MAX_CORE_REPLAY_ACTIONS:
        raise ValueError("core replay has too many actions")
    for index, action in enumerate(actions):
        validate_core_replay_action(action, index)

    lanes = value["lanes"]
    if not isinstance(lanes, list) or not lanes:
        raise ValueError("core replay lanes must be a non-empty array")
    seen_drivers: set[str] = set()
    real_client_lane = None
    for index, lane in enumerate(lanes):
        require_object(lane, f"core replay lane {index}")
        require_exact_fields(lane, {"driver", "required_gates"}, f"core replay lane {index}")
        driver = lane["driver"]
        if driver not in CORE_REPLAY_DRIVERS:
            raise ValueError(f"core replay lane {index} has unsupported driver {driver!r}")
        if driver in seen_drivers:
            raise ValueError(f"duplicate core replay lane {driver}")
        seen_drivers.add(driver)
        gates = lane["required_gates"]
        if not isinstance(gates, list) or not gates or len(gates) > MAX_CORE_REPLAY_CHECKS:
            raise ValueError(f"core replay lane {driver} must have 1..{MAX_CORE_REPLAY_CHECKS} gates")
        seen_gate_ids: set[str] = set()
        for gate_index, gate in enumerate(gates):
            require_object(gate, f"core replay lane {driver} gate {gate_index}")
            require_exact_fields(
                gate,
                {"id", "evidence_kind"},
                f"core replay lane {driver} gate {gate_index}",
            )
            require_identifier(gate["id"], "core replay gate id")
            if gate["id"] in seen_gate_ids:
                raise ValueError(f"duplicate core replay gate id {gate['id']}")
            seen_gate_ids.add(gate["id"])
            if gate["evidence_kind"] not in CORE_REPLAY_EVIDENCE_KINDS:
                raise ValueError(
                    f"core replay gate {gate['id']} has unsupported evidence kind {gate['evidence_kind']!r}"
                )
        primary_evidence = {
            "solaris_protocol": "harness",
            "vanilla_oracle": "oracle",
            "real_client": "real_client",
        }[driver]
        if not any(gate["evidence_kind"] == primary_evidence for gate in gates):
            raise ValueError(
                f"core replay lane {driver} must require {primary_evidence} evidence"
            )
        if driver == "real_client":
            real_client_lane = lane
    if real_client_lane is None:
        raise ValueError("core replay manifest has no real_client lane")

    invariants = value["expected_invariants"]
    if not isinstance(invariants, list) or not invariants or len(invariants) > MAX_CORE_REPLAY_CHECKS:
        raise ValueError(
            f"core replay expected_invariants must contain 1..{MAX_CORE_REPLAY_CHECKS} entries"
        )
    seen_invariant_ids: set[str] = set()
    for index, invariant in enumerate(invariants):
        require_object(invariant, f"core replay invariant {index}")
        require_exact_fields(
            invariant,
            {"id", "description"},
            f"core replay invariant {index}",
        )
        require_identifier(invariant["id"], "core replay invariant id")
        if invariant["id"] in seen_invariant_ids:
            raise ValueError(f"duplicate core replay invariant id {invariant['id']}")
        seen_invariant_ids.add(invariant["id"])
        if not isinstance(invariant["description"], str) or not invariant["description"].strip():
            raise ValueError(f"core replay invariant {invariant['id']} description is empty")

    if any(action["type"] == "reconnect" for action in actions):
        raise ValueError("real-client replay adapter does not support reconnect actions")
    return value


def validate_core_replay_action(action: Any, index: int) -> None:
    require_object(action, f"core replay action {index}")
    action_type = action.get("type")
    if action_type == "wait_ticks":
        require_exact_fields(action, {"type", "ticks"}, f"core replay action {index}")
        require_bounded_int(action["ticks"], "wait_ticks ticks", 1, 255)
    elif action_type == "move_by":
        require_exact_fields(action, {"type", "dx_cm", "dz_cm"}, f"core replay action {index}")
        require_bounded_int(action["dx_cm"], "move_by dx_cm", -32768, 32767)
        require_bounded_int(action["dz_cm"], "move_by dz_cm", -32768, 32767)
    elif action_type == "look":
        require_exact_fields(
            action,
            {"type", "yaw_deg", "pitch_deg"},
            f"core replay action {index}",
        )
        require_bounded_int(action["yaw_deg"], "look yaw_deg", -180, 180)
        require_bounded_int(action["pitch_deg"], "look pitch_deg", -90, 90)
    elif action_type == "reconnect":
        require_exact_fields(action, {"type"}, f"core replay action {index}")
    else:
        raise ValueError(f"core replay action {index} has unsupported type {action_type!r}")


def require_object(value: Any, label: str) -> None:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")


def require_exact_fields(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        unknown = sorted(actual - expected)
        raise ValueError(f"{label} fields mismatch: missing={missing} unknown={unknown}")


def require_identifier(value: Any, label: str) -> None:
    if not isinstance(value, str) or CORE_REPLAY_IDENTIFIER.fullmatch(value) is None:
        raise ValueError(f"{label} is not a bounded lowercase identifier: {value!r}")


def require_bounded_int(value: Any, label: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f"{label} must be an integer in [{minimum}, {maximum}]")
    return value


def core_action_summary(action: dict[str, Any]) -> str:
    action_type = action["type"]
    if action_type == "wait_ticks":
        return f"wait:{action['ticks']}"
    if action_type == "move_by":
        return f"move:{action['dx_cm']},{action['dz_cm']}"
    if action_type == "look":
        return f"look:{action['yaw_deg']},{action['pitch_deg']}"
    return "reconnect"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def core_replay_provenance(run_dir: Path) -> dict[str, Any]:
    config_path = run_dir / "server.toml"
    if not config_path.is_file():
        raise ValueError("core replay requires the runner-captured server.toml")
    commit = subprocess.run(
        ["git", "-C", str(repo_root()), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if re.fullmatch(r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", commit) is None:
        raise ValueError("git rev-parse did not return a full object id")

    properties_path = repo_root() / "client-mod/solaris-client-agent/gradle.properties"
    client_version = ""
    for line in properties_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("minecraftVersion="):
            client_version = line.split("=", 1)[1].strip()
            break
    if not client_version:
        raise ValueError("client Gradle properties do not record minecraftVersion")

    logical_cpus = os.cpu_count() or 1
    memory_mib = physical_memory_mib()
    return {
        "git_commit": commit,
        "config_sha256": hashlib.sha256(config_path.read_bytes()).hexdigest(),
        "build_profile": "debug",
        "sidecar_version": client_version,
        "hardware": {
            "os": platform.system().lower() or "unknown",
            "arch": platform.machine() or "unknown",
            "cpu_model": cpu_model(),
            "logical_cpus": logical_cpus,
            "memory_mib": memory_mib,
        },
    }


def physical_memory_mib() -> int:
    try:
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        memory_mib = int(pages) * int(page_size) // (1024 * 1024)
        if memory_mib > 0:
            return memory_mib
    except (OSError, ValueError):
        pass
    return 1


def cpu_model() -> str:
    try:
        for line in Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines():
            if line.lower().startswith("model name") and ":" in line:
                model = line.split(":", 1)[1].strip()
                if model:
                    return model
    except (OSError, UnicodeDecodeError):
        pass
    return platform.processor() or "unknown"


def call_and_record(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    command: str,
    payload: dict[str, Any],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "command": command,
        "payload": payload,
        "started_at": utc_now(),
    }
    if actor is not None:
        entry["client"] = actor
    transcript.append(entry)
    try:
        response = client.call(command, payload, timeout_seconds)
    except Exception as exc:
        entry["error"] = {"message": str(exc)}
        raise
    entry["response"] = response
    entry["finished_at"] = utc_now()
    return response


def run_core_replay_scenario(
    client: AgentClient,
    run_dir: Path,
    manifest: dict[str, Any],
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    scenario_id = manifest["id"]
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    initial_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    require_replay_play_state(initial_state, "initial")

    action_observations: list[str] = []
    final_state = initial_state
    for index, action in enumerate(manifest["actions"]):
        action_type = action["type"]
        if action_type == "wait_ticks":
            command = "wait_ticks"
            payload = {"ticks": action["ticks"]}
        elif action_type == "move_by":
            command = "move_by"
            payload = {"dx_cm": action["dx_cm"], "dz_cm": action["dz_cm"]}
        elif action_type == "look":
            command = "look"
            payload = {"yaw_deg": action["yaw_deg"], "pitch_deg": action["pitch_deg"]}
        else:
            raise ValueError(f"unsupported real-client replay action: {action_type}")
        call_and_record(client, transcript, command, payload, timeout_seconds)
        final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
        require_replay_play_state(final_state, f"post-action {index}")
        action_observations.append(f"action.{index}={core_action_summary(action)}")

    expected_dx_cm = sum(
        action.get("dx_cm", 0) for action in manifest["actions"] if action["type"] == "move_by"
    )
    expected_dz_cm = sum(
        action.get("dz_cm", 0) for action in manifest["actions"] if action["type"] == "move_by"
    )
    actual_dx_cm = (state_coordinate(final_state, "x") - state_coordinate(initial_state, "x")) * 100.0
    actual_dz_cm = (state_coordinate(final_state, "z") - state_coordinate(initial_state, "z")) * 100.0
    movement_tolerance_cm = 5.0
    if (
        abs(actual_dx_cm - expected_dx_cm) > movement_tolerance_cm
        or abs(actual_dz_cm - expected_dz_cm) > movement_tolerance_cm
    ):
        raise RuntimeError(
            "real-client replay movement did not converge: "
            f"expected=({expected_dx_cm},{expected_dz_cm})cm "
            f"actual=({actual_dx_cm:.1f},{actual_dz_cm:.1f})cm"
        )

    screenshot_path = screenshots_dir / f"{scenario_id}.png"
    call_and_record(
        client,
        transcript,
        "screenshot",
        {"path": str(screenshot_path)},
        timeout_seconds,
    )
    png_error = wait_for_valid_png(screenshot_path, min(timeout_seconds, 5.0))
    if png_error is not None:
        raise RuntimeError(f"screenshot command wrote invalid PNG {screenshot_path}: {png_error}")
    screenshot_relative = screenshot_path.relative_to(run_dir).as_posix()

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "observations": action_observations
        + [
            f"actions_executed={len(manifest['actions'])}",
            "post_action_liveness=client_play_state",
            (
                "movement_delta_cm="
                f"{round(actual_dx_cm)},{round(actual_dz_cm)}"
            ),
        ],
    }
    write_core_replay_result(
        run_dir,
        manifest,
        final_state,
        screenshot_relative,
    )
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    return "passed", final_state, [screenshot_relative], scenario_report


def require_replay_play_state(state: Any, label: str) -> None:
    require_object(state, f"{label} client state")
    if state.get("in_play") is not True:
        raise RuntimeError(f"{label} client state is not in Play")
    dimension = state.get("dimension")
    if not isinstance(dimension, str) or not dimension:
        raise RuntimeError(f"{label} client state has no dimension")
    state_coordinate(state, "x")
    state_coordinate(state, "y")
    state_coordinate(state, "z")


def state_coordinate(state: dict[str, Any], key: str) -> float:
    value = state.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise RuntimeError(f"client state {key} is not numeric")
    return float(value)


def write_core_replay_result(
    run_dir: Path,
    manifest: dict[str, Any],
    final_state: dict[str, Any],
    screenshot_relative: str,
) -> None:
    client_log = run_dir / "client.log"
    if not client_log.is_file():
        raise ValueError("core replay requires the runner-captured client.log")
    lane = next(lane for lane in manifest["lanes"] if lane["driver"] == "real_client")
    gates = []
    for requirement in lane["required_gates"]:
        if requirement["evidence_kind"] == "real_client":
            gates.append({
                "id": requirement["id"],
                "evidence_kind": requirement["evidence_kind"],
                "status": "passed",
                "artifacts": ["client.log", screenshot_relative],
            })
        else:
            gates.append({
                "id": requirement["id"],
                "evidence_kind": requirement["evidence_kind"],
                "status": "skipped",
                "reason": "the real-client adapter did not execute this evidence kind",
                "artifacts": [],
            })

    invariants = []
    for expected in manifest["expected_invariants"]:
        invariant_id = expected["id"]
        if invariant_id == "post-action-liveness":
            invariants.append({"id": invariant_id, "status": "passed"})
        elif invariant_id == "deterministic-normalized-state":
            invariants.append({
                "id": invariant_id,
                "status": "degraded",
                "reason": "one real-client phase does not establish repeated-run determinism",
            })
        else:
            invariants.append({
                "id": invariant_id,
                "status": "skipped",
                "reason": "the real-client adapter has no evaluator for this invariant",
            })

    statuses = [gate["status"] for gate in gates] + [
        invariant["status"] for invariant in invariants
    ]
    if "failed" in statuses:
        outcome = "failed"
    elif "blocked" in statuses:
        outcome = "blocked"
    elif any(status in {"degraded", "skipped"} for status in statuses):
        outcome = "degraded"
    else:
        outcome = "passed"

    notes = [
        {"type": "note", "key": f"action.{index}", "value": core_action_summary(action)}
        for index, action in enumerate(manifest["actions"])
    ]
    notes.extend([
        {
            "type": "note",
            "key": "actions_executed",
            "value": str(len(manifest["actions"])),
        },
        {
            "type": "note",
            "key": "final.dimension",
            "value": final_state["dimension"],
        },
        {
            "type": "note",
            "key": "final.position_cm",
            "value": (
                f"{round(state_coordinate(final_state, 'x') * 100)},"
                f"{round(state_coordinate(final_state, 'y') * 100)},"
                f"{round(state_coordinate(final_state, 'z') * 100)}"
            ),
        },
        {
            "type": "note",
            "key": "post_action_liveness",
            "value": "client_play_state",
        },
    ])
    notes.sort(key=lambda fact: (fact["key"], fact["value"]))

    result = {
        "schema": CORE_REPLAY_RESULT_SCHEMA,
        "scenario_id": manifest["id"],
        "seed": manifest["seed"],
        "driver": "real_client",
        "outcome": outcome,
        "actions": manifest["actions"],
        "provenance": core_replay_provenance(run_dir),
        "gates": gates,
        "invariants": invariants,
        "observations": [{
            "subject": "real_client",
            "phase": manifest["id"],
            "facts": notes,
        }],
    }
    (run_dir / "core-replay-result.json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_bridge_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    secondary_client: AgentClient | None = None,
    replay_manifest: dict[str, Any] | None = None,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    if replay_manifest is not None:
        if secondary_client is not None:
            raise ValueError("core replay currently supports exactly one real client")
        return run_core_replay_scenario(
            client,
            run_dir,
            replay_manifest,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-01-join-rejoin-chunks-movement":
        return run_join_rejoin_movement_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-03b-two-client-shared-chest":
        if secondary_client is None:
            raise RuntimeError("m94-03b two-client shared chest requires a secondary bridge")
        return run_two_client_shared_chest_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-03c-two-client-shared-chest-live-update":
        if secondary_client is None:
            raise RuntimeError("m94-03c two-client shared chest live update requires a secondary bridge")
        return run_two_client_shared_chest_live_update_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-live-visibility":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client visibility requires a secondary bridge")
        return run_two_client_visibility_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-shared-drop":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client shared drop requires a secondary bridge")
        return run_two_client_shared_drop_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "m94-06-two-client-shared-pickup":
        if secondary_client is None:
            raise RuntimeError("m94-06 two-client shared pickup requires a secondary bridge")
        return run_two_client_shared_pickup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-30-two-client-shared-log-drop-pickup":
        if secondary_client is None:
            raise RuntimeError("playable-30 two-client shared log drop/pickup requires a secondary bridge")
        return run_playable_two_client_shared_log_drop_pickup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-31-two-client-earned-shared-chest":
        if secondary_client is None:
            raise RuntimeError("playable-31 two-client earned shared chest requires a secondary bridge")
        return run_playable_two_client_earned_shared_chest_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-45-two-client-shared-chest-save-restart-before":
        if secondary_client is None:
            raise RuntimeError("playable-45 before-restart phase requires a secondary bridge")
        return run_playable_two_client_shared_chest_save_restart_before_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-45-two-client-shared-chest-save-restart-after":
        if secondary_client is None:
            raise RuntimeError("playable-45 after-restart phase requires a secondary bridge")
        return run_playable_two_client_shared_chest_save_restart_after_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id in {
        "playable-46-generated-ruin-cache-before",
        "playable-46-generated-ruin-cache-after",
    }:
        return run_playable_generated_ruin_cache_scenario(
            client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-32-two-client-earned-torch-block-edit":
        if secondary_client is None:
            raise RuntimeError("playable-32 two-client earned torch block edit requires a secondary bridge")
        return run_playable_two_client_earned_torch_block_edit_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-33-two-client-player-visibility-movement":
        if secondary_client is None:
            raise RuntimeError("playable-33 two-client player visibility/movement requires a secondary bridge")
        return run_playable_two_client_player_visibility_movement_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-34-two-client-chat-message":
        if secondary_client is None:
            raise RuntimeError("playable-34 two-client chat message requires a secondary bridge")
        return run_playable_two_client_chat_message_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-35-two-client-player-disconnect-removal":
        if secondary_client is None:
            raise RuntimeError("playable-35 two-client player disconnect removal requires a secondary bridge")
        return run_playable_two_client_player_disconnect_removal_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-36-two-client-player-reconnect-cleanup":
        if secondary_client is None:
            raise RuntimeError("playable-36 two-client player reconnect cleanup requires a secondary bridge")
        return run_playable_two_client_player_reconnect_cleanup_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-37-two-client-player-death-respawn-visibility":
        if secondary_client is None:
            raise RuntimeError("playable-37 two-client player death/respawn visibility requires a secondary bridge")
        return run_playable_two_client_player_death_respawn_visibility_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-38-two-client-inventory-drop-handoff":
        if secondary_client is None:
            raise RuntimeError("playable-38 two-client inventory drop handoff requires a secondary bridge")
        return run_playable_two_client_inventory_drop_handoff_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
        )
    if scenario_id == "playable-39-two-client-short-soak":
        if secondary_client is None:
            raise RuntimeError("playable-39 two-client short soak requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="short soak",
            pulse_count=6,
            min_chunk_delta=0,
        )
    if scenario_id == "playable-40-two-client-chunk-stream-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-40 two-client chunk-stream crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="chunk crossing",
            pulse_count=12,
            min_chunk_delta=1,
        )
    if scenario_id == "playable-41-two-client-chunk-prewarm-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-41 two-client chunk prewarm crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="chunk prewarm crossing",
            pulse_count=12,
            min_chunk_delta=1,
        )
    if scenario_id == "playable-42-two-client-opposite-chunk-crossing":
        if secondary_client is None:
            raise RuntimeError("playable-42 two-client opposite chunk crossing requires a secondary bridge")
        return run_playable_two_client_movement_soak_scenario(
            client,
            secondary_client,
            run_dir,
            scenario_id,
            server_addr,
            timeout_seconds,
            transcript,
            observation_label="opposite chunk crossing",
            pulse_count=12,
            min_chunk_delta=1,
            secondary_move_command="move_backward",
        )

    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    scenario_report = call_and_record(
        client,
        transcript,
        "run_scenario",
        {
            "id": scenario_id,
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
    )
    scenario_result = scenario_report.get("result")
    if scenario_result not in {"passed", "failed", "blocked"}:
        raise RuntimeError("run_scenario did not return passed, failed, or blocked")
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)

    screenshot_path = screenshots_dir / f"{scenario_id}.png"
    call_and_record(
        client,
        transcript,
        "screenshot",
        {"path": str(screenshot_path)},
        timeout_seconds,
    )
    png_error = wait_for_valid_png(screenshot_path, min(timeout_seconds, 5.0))
    if png_error is not None:
        raise RuntimeError(f"screenshot command wrote invalid PNG {screenshot_path}: {png_error}")

    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    reject_blocked_only_pass(scenario_id, scenario_result)
    return scenario_result, final_state, [screenshot_path.relative_to(run_dir).as_posix()], scenario_report


def run_two_client_visibility_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-place",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-place result={primary_result}",
            f"secondary bridge scenario=m94-06-two-client-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_chest_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03b-two-client-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03b-two-client-shared-chest-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-03b-two-client-shared-chest-deposit result={primary_result}",
            f"secondary bridge scenario=m94-03b-two-client-shared-chest-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_chest_live_update_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_open_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-open-with-dirt",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_open_result = scenario_report_result(primary_open_report)
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_observe_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-03c-two-client-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_observe_result = scenario_report_result(primary_observe_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_open_result,
        secondary_withdraw_result,
        primary_observe_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-03c-two-client-shared-chest-open-with-dirt result={primary_open_result}",
            f"secondary bridge scenario=m94-03c-two-client-shared-chest-withdraw result={secondary_withdraw_result}",
            f"primary bridge scenario=m94-03c-two-client-shared-chest-observe-empty result={primary_observe_result}",
        ],
        "primary_open_report": primary_open_report,
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_observe_report": primary_observe_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_drop_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_result = scenario_report_result(primary_report)
    secondary_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_result = scenario_report_result(secondary_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([primary_result, secondary_result])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-drop-break result={primary_result}",
            f"secondary bridge scenario=m94-06-two-client-drop-observe result={secondary_result}",
        ],
        "primary_report": primary_report,
        "secondary_report": secondary_report,
    }
    return result, final_state, screenshots, scenario_report


def run_two_client_shared_pickup_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    secondary_drop_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_drop_result = scenario_report_result(secondary_drop_report)
    primary_pickup_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-pickup-collect",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_pickup_result = scenario_report_result(primary_pickup_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "m94-06-two-client-pickup-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_drop_result,
        primary_pickup_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=m94-06-two-client-drop-break result={primary_drop_result}",
            f"secondary bridge scenario=m94-06-two-client-drop-observe result={secondary_drop_result}",
            f"primary bridge scenario=m94-06-two-client-pickup-collect result={primary_pickup_result}",
            f"secondary bridge scenario=m94-06-two-client-pickup-gone-observe result={secondary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_drop_report": secondary_drop_report,
        "primary_pickup_report": primary_pickup_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_log_drop_pickup_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-drop-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    secondary_drop_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-drop-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_drop_result = scenario_report_result(secondary_drop_report)
    primary_pickup_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-pickup-collect",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_pickup_result = scenario_report_result(primary_pickup_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-30-two-client-shared-log-pickup-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_drop_result,
        primary_pickup_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-30-two-client-shared-log-drop-break result={primary_drop_result}",
            f"secondary bridge scenario=playable-30-two-client-shared-log-drop-observe result={secondary_drop_result}",
            f"primary bridge scenario=playable-30-two-client-shared-log-pickup-collect result={primary_pickup_result}",
            f"secondary bridge scenario=playable-30-two-client-shared-log-pickup-gone-observe result={secondary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_drop_report": secondary_drop_report,
        "primary_pickup_report": primary_pickup_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_earned_shared_chest_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_deposit_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_deposit_result = scenario_report_result(primary_deposit_report)
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_empty_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_empty_result = scenario_report_result(primary_empty_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_deposit_result,
        secondary_withdraw_result,
        primary_empty_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-31-two-client-earned-shared-chest-deposit result={primary_deposit_result}",
            f"secondary bridge scenario=playable-31-two-client-earned-shared-chest-withdraw result={secondary_withdraw_result}",
            f"primary bridge scenario=playable-31-two-client-earned-shared-chest-observe-empty result={primary_empty_result}",
        ],
        "primary_deposit_report": primary_deposit_report,
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_empty_report": primary_empty_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_chest_save_restart_before_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_deposit_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-deposit",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_deposit_result = scenario_report_result(primary_deposit_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    clients_in_play = is_in_play(primary_state) and is_in_play(secondary_state)
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    marker = load_p45_shared_chest_marker(run_dir)
    invariant_snapshot = p45_restart_snapshot(primary_state, secondary_state, marker)
    write_restart_invariant_snapshot(run_dir, invariant_snapshot)

    result = combine_results([
        primary_deposit_result,
        "passed" if clients_in_play else "failed",
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary earned chest deposit result={primary_deposit_result}",
            f"both clients in Play before restart={str(clients_in_play).lower()}",
            "runner-managed restart pending after both clients disconnect cleanly",
        ],
        "primary_deposit_report": primary_deposit_report,
        "restart_invariant_snapshot": invariant_snapshot,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_shared_chest_save_restart_after_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    invariant_snapshot = load_restart_invariant_snapshot(run_dir, P45_RESTART_BEFORE_SCENARIO)
    marker = load_p45_shared_chest_marker(run_dir)
    validate_p45_restart_preflight(invariant_snapshot, marker)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_withdraw_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-withdraw",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_withdraw_result = scenario_report_result(secondary_withdraw_report)
    primary_empty_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-31-two-client-earned-shared-chest-observe-empty",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_empty_result = scenario_report_result(primary_empty_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    clients_in_play = is_in_play(primary_state) and is_in_play(secondary_state)
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    invariant_checks = validate_p45_restart_after(
        invariant_snapshot,
        primary_state,
        secondary_state,
        marker,
        secondary_withdraw_result,
        primary_empty_result,
    )
    result = combine_results([
        secondary_withdraw_result,
        primary_empty_result,
        "passed" if clients_in_play else "failed",
        "passed" if invariant_checks else "failed",
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary post-restart withdraw result={secondary_withdraw_result}",
            f"primary post-restart empty observe result={primary_empty_result}",
            f"both clients in Play after restart={str(clients_in_play).lower()}",
        ],
        "secondary_withdraw_report": secondary_withdraw_report,
        "primary_empty_report": primary_empty_report,
        "restart_invariant_validation": {
            "schema": RESTART_INVARIANT_SCHEMA,
            "status": "passed",
            "checks": invariant_checks,
        },
    }
    return result, final_state, screenshots, scenario_report


def run_playable_generated_ruin_cache_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    scenario_report = call_and_record(
        client,
        transcript,
        "run_scenario",
        {
            "id": scenario_id,
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
    )
    scenario_result = scenario_report_result(scenario_report)
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    reject_blocked_only_pass(scenario_id, scenario_result)
    return scenario_result, final_state, [], scenario_report


def run_playable_two_client_earned_torch_block_edit_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_place_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-place",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_place_result = scenario_report_result(primary_place_report)
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_break_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-break",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_break_result = scenario_report_result(primary_break_report)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-32-two-client-earned-torch-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_place_result,
        secondary_observe_result,
        primary_break_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-32-two-client-earned-torch-place result={primary_place_result}",
            f"secondary bridge scenario=playable-32-two-client-earned-torch-observe result={secondary_observe_result}",
            f"primary bridge scenario=playable-32-two-client-earned-torch-break result={primary_break_result}",
            f"secondary bridge scenario=playable-32-two-client-earned-torch-gone-observe result={secondary_gone_result}",
        ],
        "primary_place_report": primary_place_report,
        "secondary_observe_report": secondary_observe_report,
        "primary_break_report": primary_break_report,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_visibility_movement_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-33-two-client-player-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_before_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    move_ticks = 20
    call_and_record(
        primary,
        transcript,
        "move_forward",
        {"ticks": move_ticks},
        min(timeout_seconds, 10.0),
        "primary",
    )
    primary_after_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_moved_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-33-two-client-player-moved-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_moved_result = scenario_report_result(secondary_moved_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_observe_result,
        secondary_moved_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-33-two-client-player-observe result={secondary_observe_result}",
            f"primary bridge move_forward ticks={move_ticks}",
            f"secondary bridge scenario=playable-33-two-client-player-moved-observe result={secondary_moved_result}",
        ],
        "secondary_observe_report": secondary_observe_report,
        "primary_before_move_state": primary_before_move_state,
        "primary_after_move_state": primary_after_move_state,
        "secondary_moved_report": secondary_moved_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_chat_message_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_send_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-34-two-client-chat-send",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_send_result = scenario_report_result(primary_send_report)
    secondary_observe_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-34-two-client-chat-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_observe_result = scenario_report_result(secondary_observe_report)
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_send_result,
        secondary_observe_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-34-two-client-chat-send result={primary_send_result}",
            f"secondary bridge scenario=playable-34-two-client-chat-observe result={secondary_observe_result}",
        ],
        "primary_send_report": primary_send_report,
        "secondary_observe_report": secondary_observe_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_disconnect_removal_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_visible_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-35-two-client-player-disconnect-visible",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_visible_result = scenario_report_result(secondary_visible_report)
    primary_before_disconnect_state = call_and_record(
        primary,
        transcript,
        "state",
        {},
        timeout_seconds,
        "primary",
    )
    primary_screenshot = capture_screenshot(
        primary,
        transcript,
        run_dir,
        screenshots_dir,
        f"{scenario_id}-primary-before-disconnect",
        timeout_seconds,
        actor="primary",
    )
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")
    server_release = wait_for_server_session_release(run_dir, timeout_seconds)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-35-two-client-player-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        primary_screenshot,
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")

    result = combine_results([
        secondary_visible_result,
        secondary_gone_result,
    ])
    final_state = {
        "primary_before_disconnect": primary_before_disconnect_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-35-two-client-player-disconnect-visible result={secondary_visible_result}",
            "primary bridge disconnect: sent",
            server_release,
            f"secondary bridge scenario=playable-35-two-client-player-gone-observe result={secondary_gone_result}",
        ],
        "secondary_visible_report": secondary_visible_report,
        "primary_before_disconnect_state": primary_before_disconnect_state,
        "secondary_gone_report": secondary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_reconnect_cleanup_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_visible_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnect-visible",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_visible_result = scenario_report_result(secondary_visible_report)
    primary_before_disconnect_state = call_and_record(
        primary,
        transcript,
        "state",
        {},
        timeout_seconds,
        "primary",
    )
    primary_before_disconnect_screenshot = capture_screenshot(
        primary,
        transcript,
        run_dir,
        screenshots_dir,
        f"{scenario_id}-primary-before-disconnect",
        timeout_seconds,
        actor="primary",
    )
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")
    primary_disconnected_state = wait_until_not_in_play(primary, transcript, timeout_seconds, actor="primary")
    server_release = wait_for_server_session_release(run_dir, timeout_seconds)
    secondary_gone_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnect-gone-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_gone_result = scenario_report_result(secondary_gone_report)
    primary_reconnected_state = wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    secondary_reconnected_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-36-two-client-player-reconnected-observe",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_reconnected_result = scenario_report_result(secondary_reconnected_report)
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        primary_before_disconnect_screenshot,
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary-after-reconnect",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_visible_result,
        secondary_gone_result,
        secondary_reconnected_result,
    ])
    final_state = {
        "primary_before_disconnect": primary_before_disconnect_state,
        "primary_disconnected": primary_disconnected_state,
        "primary_reconnected": primary_reconnected_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-36-two-client-player-reconnect-visible result={secondary_visible_result}",
            "primary bridge disconnect: sent",
            server_release,
            f"secondary bridge scenario=playable-36-two-client-player-reconnect-gone-observe result={secondary_gone_result}",
            "primary bridge reconnect: reached Play state",
            f"secondary bridge scenario=playable-36-two-client-player-reconnected-observe result={secondary_reconnected_result}",
        ],
        "secondary_visible_report": secondary_visible_report,
        "primary_before_disconnect_state": primary_before_disconnect_state,
        "primary_disconnected_state": primary_disconnected_state,
        "primary_reconnected_state": primary_reconnected_state,
        "secondary_gone_report": secondary_gone_report,
        "secondary_reconnected_report": secondary_reconnected_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_player_death_respawn_visibility_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    secondary_baseline_report = call_and_record(
        secondary,
        transcript,
        "run_scenario",
        {
            "id": "playable-37-two-client-player-death-baseline",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "secondary",
    )
    secondary_baseline_result = scenario_report_result(secondary_baseline_report)
    primary_death_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-37-two-client-campfire-death-respawn",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_death_result = scenario_report_result(primary_death_report)
    primary_after_respawn_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    move_ticks = 20
    if primary_death_result == "passed":
        call_and_record(
            primary,
            transcript,
            "move_forward",
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "primary",
        )
        primary_after_move_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
        secondary_moved_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-37-two-client-player-post-respawn-moved-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_moved_result = scenario_report_result(secondary_moved_report)
    else:
        primary_after_move_state = primary_after_respawn_state
        secondary_moved_report = {
            "id": "playable-37-two-client-player-post-respawn-moved-observe",
            "result": "failed",
            "observations": ["skipped: primary campfire death/respawn phase failed"],
        }
        secondary_moved_result = "failed"
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        secondary_baseline_result,
        primary_death_result,
        secondary_moved_result,
    ])
    final_state = {
        "primary_after_respawn": primary_after_respawn_state,
        "primary_after_move": primary_after_move_state,
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"secondary bridge scenario=playable-37-two-client-player-death-baseline result={secondary_baseline_result}",
            f"primary bridge scenario=playable-37-two-client-campfire-death-respawn result={primary_death_result}",
            f"primary bridge post-respawn move_forward ticks={move_ticks}",
            f"secondary bridge scenario=playable-37-two-client-player-post-respawn-moved-observe result={secondary_moved_result}",
        ],
        "secondary_baseline_report": secondary_baseline_report,
        "primary_death_report": primary_death_report,
        "primary_after_respawn_state": primary_after_respawn_state,
        "primary_after_move_state": primary_after_move_state,
        "secondary_moved_report": secondary_moved_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_inventory_drop_handoff_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_drop_report = call_and_record(
        primary,
        transcript,
        "run_scenario",
        {
            "id": "playable-38-two-client-inventory-drop-primary",
            "screenshots_dir": str(screenshots_dir),
        },
        timeout_seconds,
        "primary",
    )
    primary_drop_result = scenario_report_result(primary_drop_report)
    if primary_drop_result == "passed":
        secondary_observe_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_observe_result = scenario_report_result(secondary_observe_report)
    else:
        secondary_observe_report = {
            "id": "playable-38-two-client-inventory-drop-observe",
            "result": "failed",
            "observations": ["skipped: primary inventory drop phase failed"],
        }
        secondary_observe_result = "failed"
    if secondary_observe_result == "passed":
        secondary_pickup_report = call_and_record(
            secondary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-secondary-pickup",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "secondary",
        )
        secondary_pickup_result = scenario_report_result(secondary_pickup_report)
    else:
        secondary_pickup_report = {
            "id": "playable-38-two-client-inventory-drop-secondary-pickup",
            "result": "failed",
            "observations": ["skipped: secondary inventory drop observe phase failed"],
        }
        secondary_pickup_result = "failed"
    if secondary_pickup_result == "passed":
        primary_gone_report = call_and_record(
            primary,
            transcript,
            "run_scenario",
            {
                "id": "playable-38-two-client-inventory-drop-gone-observe",
                "screenshots_dir": str(screenshots_dir),
            },
            timeout_seconds,
            "primary",
        )
        primary_gone_result = scenario_report_result(primary_gone_report)
    else:
        primary_gone_report = {
            "id": "playable-38-two-client-inventory-drop-gone-observe",
            "result": "failed",
            "observations": ["skipped: secondary inventory drop pickup phase failed"],
        }
        primary_gone_result = "failed"
    primary_state = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_state = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")
    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    result = combine_results([
        primary_drop_result,
        secondary_observe_result,
        secondary_pickup_result,
        primary_gone_result,
    ])
    final_state = {
        "primary": primary_state,
        "secondary": secondary_state,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            f"primary bridge scenario=playable-38-two-client-inventory-drop-primary result={primary_drop_result}",
            f"secondary bridge scenario=playable-38-two-client-inventory-drop-observe result={secondary_observe_result}",
            f"secondary bridge scenario=playable-38-two-client-inventory-drop-secondary-pickup result={secondary_pickup_result}",
            f"primary bridge scenario=playable-38-two-client-inventory-drop-gone-observe result={primary_gone_result}",
        ],
        "primary_drop_report": primary_drop_report,
        "secondary_observe_report": secondary_observe_report,
        "secondary_pickup_report": secondary_pickup_report,
        "primary_gone_report": primary_gone_report,
    }
    return result, final_state, screenshots, scenario_report


def run_playable_two_client_movement_soak_scenario(
    primary: AgentClient,
    secondary: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    *,
    observation_label: str,
    pulse_count: int,
    min_chunk_delta: int,
    primary_move_command: str = "move_forward",
    secondary_move_command: str = "move_forward",
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    call_and_record(primary, transcript, "ping", {}, timeout_seconds, "primary")
    call_and_record(secondary, transcript, "ping", {}, timeout_seconds, "secondary")
    primary_play = wait_for_existing_or_explicit_connection(
        primary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="primary",
    )
    secondary_play = wait_for_existing_or_explicit_connection(
        secondary,
        transcript,
        server_addr,
        timeout_seconds,
        actor="secondary",
    )
    primary_initial = call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary")
    secondary_initial = call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary")

    move_ticks = 15
    primary_states: list[dict[str, Any]] = []
    secondary_states: list[dict[str, Any]] = []
    for _ in range(pulse_count):
        call_and_record(
            primary,
            transcript,
            primary_move_command,
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "primary",
        )
        primary_states.append(call_and_record(primary, transcript, "state", {}, timeout_seconds, "primary"))
        call_and_record(
            secondary,
            transcript,
            secondary_move_command,
            {"ticks": move_ticks},
            min(timeout_seconds, 10.0),
            "secondary",
        )
        secondary_states.append(call_and_record(secondary, transcript, "state", {}, timeout_seconds, "secondary"))

    primary_final = primary_states[-1] if primary_states else primary_initial
    secondary_final = secondary_states[-1] if secondary_states else secondary_initial
    primary_delta = horizontal_distance(primary_initial, primary_final)
    secondary_delta = horizontal_distance(secondary_initial, secondary_final)
    primary_chunk_delta = chunk_delta(primary_initial, primary_final)
    secondary_chunk_delta = chunk_delta(secondary_initial, secondary_final)
    all_states_in_play = all(
        is_in_play(state)
        for state in [primary_play, secondary_play, primary_initial, secondary_initial]
        + primary_states
        + secondary_states
    )
    movement_passed = (
        primary_delta >= 0.05
        and secondary_delta >= 0.05
        and primary_chunk_delta >= min_chunk_delta
        and secondary_chunk_delta >= min_chunk_delta
    )
    result = "passed" if all_states_in_play and movement_passed else "failed"

    screenshots = [
        capture_screenshot(
            primary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-primary",
            timeout_seconds,
            actor="primary",
        ),
        capture_screenshot(
            secondary,
            transcript,
            run_dir,
            screenshots_dir,
            f"{scenario_id}-secondary",
            timeout_seconds,
            actor="secondary",
        ),
    ]
    call_and_record(secondary, transcript, "disconnect", {}, timeout_seconds, "secondary")
    call_and_record(primary, transcript, "disconnect", {}, timeout_seconds, "primary")

    final_state = {
        "primary": primary_final,
        "secondary": secondary_final,
    }
    scenario_report = {
        "id": scenario_id,
        "result": result,
        "observations": [
            "two-client "
            + observation_label
            + ": "
            + result
            + f" pulses={pulse_count} min_chunk_delta={min_chunk_delta}"
            + f" ticks={move_ticks}"
            + f" primary_move_command={primary_move_command}"
            + f" secondary_move_command={secondary_move_command}"
            + f" primary_horizontal_delta={primary_delta:.3f}"
            + f" secondary_horizontal_delta={secondary_delta:.3f}"
            + f" primary_chunk_delta={primary_chunk_delta}"
            + f" secondary_chunk_delta={secondary_chunk_delta}"
            + f" all_states_in_play={str(all_states_in_play).lower()}",
        ],
        "primary_initial_state": primary_initial,
        "primary_final_state": primary_final,
        "secondary_initial_state": secondary_initial,
        "secondary_final_state": secondary_final,
    }
    return result, final_state, screenshots, scenario_report


def scenario_report_result(report: dict[str, Any]) -> str:
    result = report.get("result")
    if result not in {"passed", "failed", "blocked"}:
        raise RuntimeError("run_scenario did not return passed, failed, or blocked")
    return result


def run_join_rejoin_movement_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    observations: list[str] = []
    screenshots: list[str] = []

    call_and_record(client, transcript, "ping", {}, timeout_seconds)
    initial_play = wait_for_existing_or_explicit_connection(
        client,
        transcript,
        server_addr,
        timeout_seconds,
    )
    observations.append(state_observation("initial play", initial_play))
    before_move = call_and_record(client, transcript, "state", {}, timeout_seconds)

    move_ticks = 15
    call_and_record(
        client,
        transcript,
        "move_forward",
        {"ticks": move_ticks},
        min(timeout_seconds, 10.0),
    )
    after_move = call_and_record(client, transcript, "state", {}, timeout_seconds)
    movement_distance = horizontal_distance(before_move, after_move)
    movement_passed = movement_distance >= 0.05
    observations.append(
        "movement probe: "
        + ("passed" if movement_passed else "failed")
        + f" ticks={move_ticks} horizontal_delta={movement_distance:.3f}"
    )

    screenshots.append(capture_screenshot(client, transcript, run_dir, screenshots_dir, f"{scenario_id}-after-move", timeout_seconds))
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
    wait_until_not_in_play(client, transcript, timeout_seconds)
    observations.append(wait_for_server_session_release(run_dir, timeout_seconds))
    call_and_record(client, transcript, "connect", {"server_addr": server_addr}, timeout_seconds)
    rejoin_play = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": timeout_seconds},
        timeout_seconds,
    )
    rejoin_play = wait_for_interactive_play(
        client,
        transcript,
        rejoin_play,
        timeout_seconds,
    )
    if not is_in_play(rejoin_play):
        raise RuntimeError("client did not reach Play state after reconnect")
    observations.append(state_observation("rejoin play", rejoin_play))
    final_state = call_and_record(client, transcript, "state", {}, timeout_seconds)
    screenshots.append(capture_screenshot(client, transcript, run_dir, screenshots_dir, f"{scenario_id}-after-rejoin", timeout_seconds))
    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)

    result = "passed" if movement_passed and is_in_play(initial_play) and is_in_play(rejoin_play) else "failed"
    scenario_report = {
        "result": result,
        "id": scenario_id,
        "observations": observations,
    }
    return result, final_state, screenshots, scenario_report


def wait_for_existing_or_explicit_connection(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    server_addr: str,
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    preconnect_timeout = min(timeout_seconds, 5.0)
    play_state = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": preconnect_timeout},
        preconnect_timeout + 1.0,
        actor,
    )
    if is_in_play(play_state):
        return wait_for_interactive_play(
            client,
            transcript,
            play_state,
            timeout_seconds,
            actor,
        )
    if is_connecting(play_state):
        play_state = call_and_record(
            client,
            transcript,
            "wait_play",
            {"timeout_seconds": timeout_seconds},
            timeout_seconds,
            actor,
        )
        if is_in_play(play_state):
            return wait_for_interactive_play(
                client,
                transcript,
                play_state,
                timeout_seconds,
                actor,
            )
        raise RuntimeError("client did not reach Play state from existing ConnectScreen")
    call_and_record(client, transcript, "connect", {"server_addr": server_addr}, timeout_seconds, actor)
    play_state = call_and_record(
        client,
        transcript,
        "wait_play",
        {"timeout_seconds": timeout_seconds},
        timeout_seconds,
        actor,
    )
    if not is_in_play(play_state):
        raise RuntimeError("client did not reach Play state")
    return wait_for_interactive_play(
        client,
        transcript,
        play_state,
        timeout_seconds,
        actor,
    )


def wait_for_interactive_play(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    play_state: dict[str, Any],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    if is_interactive_play(play_state) or not is_in_play(play_state):
        return play_state

    deadline = time.monotonic() + timeout_seconds
    latest_state = play_state
    closed_version: int | None = None
    while time.monotonic() < deadline:
        version = state_version(latest_state)
        if is_pause_screen(latest_state) and closed_version != version:
            remaining = max(0.05, deadline - time.monotonic())
            client_thread_timeout = min(remaining, 12.0)
            call_and_record(
                client,
                transcript,
                "close_screen",
                {},
                client_thread_timeout,
                actor,
            )
            closed_version = version
            latest_state = call_and_record(
                client,
                transcript,
                "state",
                {},
                client_thread_timeout,
                actor,
            )
            if is_interactive_play(latest_state) or not is_in_play(latest_state):
                return latest_state
        latest_state = wait_for_state_event(
            client,
            transcript,
            latest_state,
            deadline,
            actor,
        )
        if is_interactive_play(latest_state) or not is_in_play(latest_state):
            return latest_state

    raise RuntimeError(
        "client reached Play state but did not become scenario-interactive"
        + f" current_screen={latest_state.get('current_screen', '')!r}"
    )


def wait_until_not_in_play(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    timeout_seconds: float,
    actor: str | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + min(timeout_seconds, 5.0)
    latest_state = call_and_record(
        client,
        transcript,
        "state",
        {},
        min(timeout_seconds, 5.0),
        actor,
    )
    while time.monotonic() < deadline:
        if not is_in_play(latest_state):
            return latest_state
        latest_state = wait_for_state_event(
            client,
            transcript,
            latest_state,
            deadline,
            actor,
        )
    raise RuntimeError("client did not leave Play state before reconnect")


def wait_for_state_event(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    latest_state: dict[str, Any],
    deadline: float,
    actor: str | None,
) -> dict[str, Any]:
    remaining = deadline - time.monotonic()
    if remaining <= 0.0:
        raise RuntimeError("client state event did not arrive before timeout")
    event_timeout = min(remaining, 5.0)
    return call_and_record(
        client,
        transcript,
        "wait_state_change",
        {
            "observed_version": state_version(latest_state),
            "timeout_seconds": event_timeout,
        },
        event_timeout + 1.0,
        actor,
    )


def state_version(state: dict[str, Any]) -> int:
    value = state.get("state_version")
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise RuntimeError("client state response did not include a valid state_version")
    return value


def wait_for_server_session_release(run_dir: Path, timeout_seconds: float) -> str:
    server_log = run_dir / "server.log"
    result = f"server session release: observed log={server_log.name}"
    try:
        return wait_for_path_condition(
            server_log,
            min(timeout_seconds, 60.0),
            lambda: (server_session_release_logged(server_log), result),
        )
    except TimeoutError as error:
        raise RuntimeError(
            f"server did not log session release before reconnect: {server_log}"
        ) from error


def server_session_release_logged(server_log: Path) -> bool:
    try:
        return "saved player state player=" in server_log.read_text(
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return False


def capture_screenshot(
    client: AgentClient,
    transcript: list[dict[str, Any]],
    run_dir: Path,
    screenshots_dir: Path,
    name: str,
    timeout_seconds: float,
    actor: str | None = None,
) -> str:
    screenshot_path = screenshots_dir / f"{name}.png"
    call_and_record(
        client,
        transcript,
        "screenshot",
        {"path": str(screenshot_path)},
        timeout_seconds,
        actor,
    )
    png_error = wait_for_valid_png(screenshot_path, min(timeout_seconds, 5.0))
    if png_error is not None:
        raise RuntimeError(f"screenshot command wrote invalid PNG {screenshot_path}: {png_error}")
    return screenshot_path.relative_to(run_dir).as_posix()


def is_in_play(payload: dict[str, Any]) -> bool:
    return bool(payload.get("in_play"))


def is_interactive_play(payload: dict[str, Any]) -> bool:
    if not is_in_play(payload):
        return False
    current_screen = payload.get("current_screen")
    if current_screen is None:
        return True
    return current_screen == "" or str(current_screen).lower() == "none"


def is_pause_screen(payload: dict[str, Any]) -> bool:
    return payload.get("current_screen") == "net.minecraft.client.gui.screens.PauseScreen"


def is_connecting(payload: dict[str, Any]) -> bool:
    return payload.get("current_screen") == "net.minecraft.client.gui.screens.ConnectScreen"


def horizontal_distance(before: dict[str, Any], after: dict[str, Any]) -> float:
    try:
        dx = float(after.get("x", 0.0)) - float(before.get("x", 0.0))
        dz = float(after.get("z", 0.0)) - float(before.get("z", 0.0))
    except (TypeError, ValueError):
        return 0.0
    return (dx * dx + dz * dz) ** 0.5


def chunk_delta(before: dict[str, Any], after: dict[str, Any]) -> int:
    try:
        before_x = int(float(before.get("x", 0.0)) // 16)
        before_z = int(float(before.get("z", 0.0)) // 16)
        after_x = int(float(after.get("x", 0.0)) // 16)
        after_z = int(float(after.get("z", 0.0)) // 16)
    except (TypeError, ValueError):
        return 0
    return abs(after_x - before_x) + abs(after_z - before_z)


def state_observation(label: str, state: dict[str, Any]) -> str:
    return (
        f"{label}: in_play={bool(state.get('in_play'))}"
        f" dimension={state.get('dimension', '')}"
        f" position={state.get('x', 0.0)},{state.get('y', 0.0)},{state.get('z', 0.0)}"
    )


def wait_for_file(path: Path, timeout_seconds: float) -> bool:
    try:
        return wait_for_path_condition(
            path,
            timeout_seconds,
            lambda: (path.is_file(), True),
        )
    except TimeoutError:
        return False


def wait_for_valid_png(path: Path, timeout_seconds: float) -> str | None:
    last_error = ["file not created"]

    def png_ready() -> tuple[bool, None]:
        if path.is_file():
            last_error[0] = png_validation_error(path)
            return last_error[0] is None, None
        return False, None

    try:
        return wait_for_path_condition(path, timeout_seconds, png_ready)
    except TimeoutError:
        if path.is_file():
            return png_validation_error(path)
        return last_error[0]


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

BLOCKED_ONLY_BROAD_SCENARIOS = {
    "m94-02-blocks-fluids-farming-drops",
    "m94-03-inventory-crafting-containers-stations",
    "m94-04-signs-beds-campfires-and-block-entities",
    "m94-05-entities-combat-death-respawn",
    "m94-06-save-restart-two-client-visibility",
    "m94-07-m40-m41-route-with-metrics",
}


def png_validation_error(path: Path) -> str | None:
    try:
        data = path.read_bytes()
    except OSError as exc:
        return f"could not read file: {exc}"

    if not data.startswith(PNG_SIGNATURE):
        return "missing PNG signature"
    offset = len(PNG_SIGNATURE)
    saw_ihdr = False
    while True:
        if offset + 12 > len(data):
            return "truncated PNG chunk header"
        length = int.from_bytes(data[offset : offset + 4], "big")
        chunk_type = data[offset + 4 : offset + 8]
        chunk_start = offset + 8
        chunk_end = chunk_start + length
        crc_end = chunk_end + 4
        if crc_end > len(data):
            return f"truncated {chunk_type.decode('ascii', errors='replace')} chunk"
        expected_crc = int.from_bytes(data[chunk_end:crc_end], "big")
        actual_crc = binascii.crc32(chunk_type + data[chunk_start:chunk_end]) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            return f"{chunk_type.decode('ascii', errors='replace')} CRC mismatch"
        if not saw_ihdr:
            if chunk_type != b"IHDR":
                return "first PNG chunk is not IHDR"
            if length != 13:
                return "IHDR chunk has invalid length"
            saw_ihdr = True
        if chunk_type == b"IEND":
            if length != 0:
                return "IEND chunk has invalid length"
            if crc_end != len(data):
                return "trailing bytes after IEND"
            return None
        offset = crc_end


def reject_blocked_only_pass(scenario_id: str, scenario_result: str) -> None:
    if scenario_result != "passed" or scenario_id not in BLOCKED_ONLY_BROAD_SCENARIOS:
        return
    raise RuntimeError(
        f"scenario {scenario_id} is blocked-only and must report blocked until broad M94 evidence is complete"
    )


def write_observations(
    run_dir: Path,
    scenario_id: str,
    transcript: list[dict[str, Any]],
    result: str,
    *,
    server_addr: str,
    final_state: dict[str, Any] | None = None,
    screenshots: list[str] | None = None,
    scenario_report: dict[str, Any] | None = None,
    error_message: str | None = None,
    append_observations: bool = False,
) -> None:
    run_dir.mkdir(parents=True, exist_ok=True)
    quality_label = run_quality_label(run_dir)
    scenario: dict[str, Any] = {
        "id": scenario_id,
        "result": result,
        "commands": transcript,
        "final_state": final_state or {},
        "screenshots": screenshots or [],
    }
    if scenario_report is not None:
        scenario["agent_report"] = scenario_report
    observations: dict[str, Any] = {
        "schema": "solaris.real_client_observations.v1",
        "client_gate": "agent-run-real-client",
        "quality_label": quality_label,
        "result": result,
        "server_addr": server_addr,
        "generated_at": utc_now(),
        "scenarios": [scenario],
    }
    if error_message is not None:
        error_payload = {"message": error_message}
        observations["error"] = error_payload
        scenario["error"] = error_payload
    observations_path = run_dir / "observations.json"
    if append_observations:
        observations = append_to_existing_observations(observations_path, observations)
    observations_path.write_text(
        json.dumps(observations, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def run_quality_label(run_dir: Path) -> str:
    manifest_path = run_dir / "manifest.json"
    if not manifest_path.is_file():
        return "stabilization"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    quality_label = manifest.get("quality_label")
    if not isinstance(quality_label, str) or not quality_label:
        raise RuntimeError("manifest.json quality_label must be a non-empty string")
    return quality_label


def append_to_existing_observations(path: Path, next_observations: dict[str, Any]) -> dict[str, Any]:
    if not path.is_file():
        return next_observations

    existing = json.loads(path.read_text(encoding="utf-8"))
    for field in ["schema", "client_gate", "quality_label"]:
        if existing.get(field) != next_observations.get(field):
            raise RuntimeError(f"cannot append observations with mismatched {field}")

    existing_scenarios = existing.get("scenarios")
    next_scenarios = next_observations.get("scenarios")
    if not isinstance(existing_scenarios, list) or not isinstance(next_scenarios, list):
        raise RuntimeError("cannot append observations without scenario arrays")

    combined = dict(existing)
    combined["generated_at"] = next_observations["generated_at"]
    combined["server_addr"] = next_observations["server_addr"]
    combined["scenarios"] = existing_scenarios + next_scenarios
    combined["result"] = combine_results(
        scenario.get("result", "failed") for scenario in combined["scenarios"]
    )
    if "error" in next_observations:
        combined["error"] = next_observations["error"]
    return combined


def combine_results(results: Any) -> str:
    saw_blocked = False
    for result in results:
        if result == "failed":
            return "failed"
        if result == "blocked":
            saw_blocked = True
        elif result != "passed":
            return "failed"
    return "blocked" if saw_blocked else "passed"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Drive a loopback real-client bridge and write Solaris observations."
    )
    parser.add_argument("--bridge-url", required=True)
    parser.add_argument("--secret", required=True)
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--scenario", default="m94-02b-rejected-block-resync")
    parser.add_argument(
        "--replay-manifest",
        type=Path,
        help="Checked solaris.core_replay.scenario.v1 manifest for the real-client lane.",
    )
    parser.add_argument("--server-addr", required=True)
    parser.add_argument("--timeout-seconds", type=float, default=30.0)
    parser.add_argument("--secondary-bridge-url")
    parser.add_argument("--secondary-secret")
    parser.add_argument(
        "--append-observations",
        action="store_true",
        help="Append this scenario result to an existing observations.json and recompute the top-level result.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    run_dir = Path(args.run_dir)
    server_addr = args.server_addr
    transcript: list[dict[str, Any]] = []

    try:
        if not args.secret:
            raise ValueError("bridge secret must not be empty")
        if args.timeout_seconds <= 0:
            raise ValueError("timeout must be positive")
        server_addr = validate_loopback_server_addr(args.server_addr)
        replay_manifest = None
        if args.replay_manifest is not None:
            replay_manifest = load_core_replay_manifest(args.replay_manifest, args.scenario)
        client = AgentClient(args.bridge_url, args.secret)
        secondary_client = None
        if bool(args.secondary_bridge_url) != bool(args.secondary_secret):
            raise ValueError("secondary bridge URL and secret must be provided together")
        if args.secondary_bridge_url:
            secondary_client = AgentClient(args.secondary_bridge_url, args.secondary_secret)
        result, final_state, screenshots, scenario_report = run_bridge_scenario(
            client,
            run_dir,
            args.scenario,
            server_addr,
            args.timeout_seconds,
            transcript,
            secondary_client=secondary_client,
            replay_manifest=replay_manifest,
        )
        write_observations(
            run_dir,
            args.scenario,
            transcript,
            result,
            server_addr=server_addr,
            final_state=final_state,
            screenshots=screenshots,
            scenario_report=scenario_report,
            append_observations=args.append_observations,
        )
        print(f"wrote real-client agent observations: {run_dir / 'observations.json'}")
        return 0 if result == "passed" else 1
    except Exception as exc:
        write_observations(
            run_dir,
            args.scenario,
            transcript,
            "failed",
            server_addr=server_addr,
            error_message=str(exc),
            append_observations=args.append_observations,
        )
        print(f"real-client agent driver failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
