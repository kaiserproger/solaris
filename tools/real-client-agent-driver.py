#!/usr/bin/env python3
"""Drive a Solaris real-client bridge and write fail-closed observations."""

from __future__ import annotations

import argparse
import json
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib import error, parse, request


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


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


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


def run_bridge_scenario(
    client: AgentClient,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    secondary_client: AgentClient | None = None,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
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
    if not wait_for_file(screenshot_path, min(timeout_seconds, 5.0)):
        raise RuntimeError(f"screenshot command did not create {screenshot_path}")

    call_and_record(client, transcript, "disconnect", {}, timeout_seconds)
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

    move_duration_millis = 750
    call_and_record(
        client,
        transcript,
        "move_forward",
        {"duration_millis": move_duration_millis},
        min(timeout_seconds, 10.0),
    )
    after_move = call_and_record(client, transcript, "state", {}, timeout_seconds)
    movement_distance = horizontal_distance(before_move, after_move)
    movement_passed = movement_distance >= 0.05
    observations.append(
        "movement probe: "
        + ("passed" if movement_passed else "failed")
        + f" duration_millis={move_duration_millis} horizontal_delta={movement_distance:.3f}"
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
    while time.monotonic() < deadline:
        time.sleep(0.05)
        remaining = max(0.05, deadline - time.monotonic())
        rpc_timeout = min(remaining, 5.0)
        latest_state = call_and_record(
            client,
            transcript,
            "wait_play",
            {"timeout_seconds": rpc_timeout},
            rpc_timeout + 1.0,
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
) -> dict[str, Any]:
    deadline = time.monotonic() + min(timeout_seconds, 5.0)
    latest_state: dict[str, Any] = {}
    while time.monotonic() < deadline:
        latest_state = call_and_record(client, transcript, "state", {}, min(timeout_seconds, 5.0))
        if not is_in_play(latest_state):
            return latest_state
        time.sleep(0.05)
    raise RuntimeError("client did not leave Play state before reconnect")


def wait_for_server_session_release(run_dir: Path, timeout_seconds: float) -> str:
    server_log = run_dir / "server.log"
    deadline = time.monotonic() + min(timeout_seconds, 60.0)
    while time.monotonic() < deadline:
        if server_session_release_logged(server_log):
            return f"server session release: observed log={server_log.name}"
        time.sleep(0.05)
    raise RuntimeError(f"server did not log session release before reconnect: {server_log}")


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
    if not wait_for_file(screenshot_path, min(timeout_seconds, 5.0)):
        raise RuntimeError(f"screenshot command did not create {screenshot_path}")
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


def is_connecting(payload: dict[str, Any]) -> bool:
    return payload.get("current_screen") == "net.minecraft.client.gui.screens.ConnectScreen"


def horizontal_distance(before: dict[str, Any], after: dict[str, Any]) -> float:
    try:
        dx = float(after.get("x", 0.0)) - float(before.get("x", 0.0))
        dz = float(after.get("z", 0.0)) - float(before.get("z", 0.0))
    except (TypeError, ValueError):
        return 0.0
    return (dx * dx + dz * dz) ** 0.5


def state_observation(label: str, state: dict[str, Any]) -> str:
    return (
        f"{label}: in_play={bool(state.get('in_play'))}"
        f" dimension={state.get('dimension', '')}"
        f" position={state.get('x', 0.0)},{state.get('y', 0.0)},{state.get('z', 0.0)}"
    )


def wait_for_file(path: Path, timeout_seconds: float) -> bool:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if path.is_file():
            return True
        time.sleep(0.05)
    return path.is_file()


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
        "quality_label": "stabilization",
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
    transcript: list[dict[str, Any]] = []

    try:
        if not args.secret:
            raise ValueError("bridge secret must not be empty")
        if args.timeout_seconds <= 0:
            raise ValueError("timeout must be positive")
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
            args.server_addr,
            args.timeout_seconds,
            transcript,
            secondary_client=secondary_client,
        )
        write_observations(
            run_dir,
            args.scenario,
            transcript,
            result,
            server_addr=args.server_addr,
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
            server_addr=args.server_addr,
            error_message=str(exc),
            append_observations=args.append_observations,
        )
        print(f"real-client agent driver failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
