#!/usr/bin/env python3
"""Capture multi-location real-client terrain evidence for seed 712816."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import math
import os
import subprocess
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
SEED = 712816
YAW_VIEWS = [("north", 180), ("east", -90), ("south", 0), ("west", 90)]
MOVE_STAGES = [
    ("walk-a", [0, -90, 90, 180]),
    ("walk-b", [90, -90, 0, 180]),
]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load module from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_config(run_dir: Path, server_port: int) -> tuple[Path, Path]:
    world_dir = run_dir / "world"
    source = (REPO_ROOT / "playable.toml").read_text()
    world_rel = world_dir.relative_to(REPO_ROOT).as_posix()
    replacements = [
        ("port = 25565", f"port = {server_port}"),
        ('world_dir = ".analysis/test-world-v11"', f'world_dir = "{world_rel}"'),
        ("seed = 0", f"seed = {SEED}"),
    ]
    for old, new in replacements:
        if source.count(old) != 1:
            raise RuntimeError(f"expected exactly one config token: {old}")
        source = source.replace(old, new, 1)
    config = run_dir / "playable.toml"
    config.write_text(source)
    return config, world_dir


def player_position(observed: dict[str, Any]) -> dict[str, float]:
    player = observed.get("player")
    if not isinstance(player, dict):
        raise RuntimeError(f"player observation missing: {observed}")
    return {
        "x": float(player["x"]),
        "y": float(player["y"]),
        "z": float(player["z"]),
    }


def horizontal_distance(a: dict[str, float], b: dict[str, float]) -> float:
    return math.hypot(b["x"] - a["x"], b["z"] - a["z"])


def capture_views(
    client: Any,
    screenshots_dir: Path,
    label: str,
    position: dict[str, float],
) -> list[dict[str, Any]]:
    captures: list[dict[str, Any]] = []
    for direction, yaw in YAW_VIEWS:
        client.call_tool("minecraft_look", {"yaw_deg": yaw, "pitch_deg": -8})
        client.call_tool("minecraft_wait_ticks", {"ticks": 2})
        frame_observation = client.call_tool("minecraft_observe")
        frame_position = player_position(frame_observation)
        frame_player = frame_observation.get("player")
        if not isinstance(frame_player, dict):
            raise RuntimeError(f"player observation missing before screenshot: {frame_observation}")
        observed_yaw = float(frame_player["yaw"])
        observed_pitch = float(frame_player["pitch"])
        path = screenshots_dir / f"{label}-{direction}.png"
        shot = client.call_tool("minecraft_screenshot", {"path": str(path)})
        written = Path(str(shot.get("path", path)))
        if not written.is_file():
            raise RuntimeError(f"screenshot missing after capture: {written}")
        captures.append(
            {
                "direction": direction,
                "requested_yaw": yaw,
                "requested_pitch": -8,
                "yaw": observed_yaw,
                "pitch": observed_pitch,
                "stage_position": position,
                "position": frame_position,
                "path": str(written),
                "sha256": sha256_file(written),
            }
        )
    return captures


def move_ordinary(
    client: Any,
    candidate_yaws: list[int],
    prior_positions: list[dict[str, float]],
) -> dict[str, Any]:
    attempts: list[dict[str, Any]] = []

    for attempt, yaw in enumerate(candidate_yaws, start=1):
        before_observation = client.call_tool("minecraft_observe")
        before = player_position(before_observation)
        normalized_yaw = ((yaw + 180) % 360) - 180
        client.call_tool("minecraft_look", {"yaw_deg": normalized_yaw, "pitch_deg": 0})
        client.call_tool("minecraft_wait_ticks", {"ticks": 2})
        input_result = client.call_tool(
            "minecraft_press_inputs",
            {"keys": ["forward", "sprint", "jump"], "ticks": 100},
        )
        client.call_tool("minecraft_wait_ticks", {"ticks": 20})
        after_observation = client.call_tool("minecraft_observe")
        after = player_position(after_observation)
        leg_distance = horizontal_distance(before, after)
        prior_distances = [horizontal_distance(position, after) for position in prior_positions]
        attempts.append(
            {
                "attempt": attempt,
                "yaw": normalized_yaw,
                "ticks": 100,
                "keys": ["forward", "sprint", "jump"],
                "input_result": input_result,
                "before": before,
                "after": after,
                "leg_distance": leg_distance,
                "prior_distances": prior_distances,
            }
        )
        if prior_distances and min(prior_distances) >= 8.0:
            return {
                "candidate_yaws": candidate_yaws,
                "attempts": attempts,
                "observation": after_observation,
                "position": after,
                "min_prior_distance": min(prior_distances),
            }

    raise RuntimeError(
        "ordinary movement did not produce a viewpoint 8 blocks from all prior captures: "
        f"{attempts}"
    )


def run(timeout_seconds: float) -> dict[str, Any]:
    helpers = load_module(
        "solaris_loader_live_gate", REPO_ROOT / "tools" / "run-loader-live-gate.py"
    )
    plugin_helpers = load_module(
        "solaris_plugin_client_gate", REPO_ROOT / "tools" / "run-plugin-client-compat-gate.py"
    )
    McpClient = helpers.load_mcp_client()

    run_id = time.strftime("%Y%m%dT%H%M%S")
    run_dir = REPO_ROOT / ".analysis" / "seed-owner-review" / run_id
    run_dir.mkdir(parents=True, exist_ok=False)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir()
    server_port = helpers.reserve_port()
    mcp_port = helpers.reserve_port()
    config, world_dir = write_config(run_dir, server_port)
    if world_dir.exists():
        raise RuntimeError(f"fresh-world path already exists before server start: {world_dir}")
    token = f"seed-owner-{SEED}-{os.getpid()}-{time.time_ns()}"

    xvfb = server = client_process = None
    xvfb_log = server_log = client_log = None
    mcp = None
    result: dict[str, Any] = {
        "run_id": run_id,
        "seed": SEED,
        "artifact_dir": str(run_dir.relative_to(REPO_ROOT)),
        "world_dir": str(world_dir.relative_to(REPO_ROOT)),
        "world_dir_preexisting": False,
        "config": str(config.relative_to(REPO_ROOT)),
        "config_sha256": sha256_file(config),
        "server_port": server_port,
        "mcp_port": mcp_port,
        "operators": [],
        "captures": [],
        "movements": [],
    }
    try:
        subprocess.run(["cargo", "build", "-p", "mc-server"], cwd=REPO_ROOT, check=True)
        xvfb, display, xvfb_log = helpers.start_xvfb(run_dir)
        server_log = (run_dir / "server.log").open("wb")
        server = subprocess.Popen(
            [str(REPO_ROOT / "target" / "debug" / "mc-server"), "--config", str(config)],
            cwd=REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        helpers.wait_port(server_port, min(timeout_seconds, 90.0), server)
        if not world_dir.is_dir():
            raise RuntimeError("server became ready without creating the fresh world directory")
        result["world_dir_created_by_server"] = True

        client_process, client_log, game_dir = plugin_helpers.start_no_loader_client(
            helpers, run_dir, token, mcp_port, "SeedOwnerReview", display
        )
        result["game_dir"] = str(game_dir.relative_to(REPO_ROOT))
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=max(30.0, timeout_seconds + 10.0),
        )
        mcp.initialize()
        helpers.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        mcp.call_tool("minecraft_connect", {"server_addr": f"127.0.0.1:{server_port}"})
        play = mcp.call_tool(
            "minecraft_wait_for_play", {"timeout_seconds": min(timeout_seconds, 120.0)}
        )
        if not play.get("in_play"):
            raise RuntimeError(f"client did not reach Play: {play}")
        mcp.call_tool("minecraft_wait_ticks", {"ticks": 20})

        spawn_observation = mcp.call_tool("minecraft_observe")
        spawn_position = player_position(spawn_observation)
        result["spawn_observation"] = spawn_observation
        result["captures"].append(
            {
                "label": "spawn",
                "position": spawn_position,
                "screenshots": capture_views(
                    mcp, screenshots_dir, "spawn", spawn_position
                ),
            }
        )

        prior_positions = [spawn_position]
        for label, candidate_yaws in MOVE_STAGES:
            movement = move_ordinary(mcp, candidate_yaws, prior_positions)
            result["movements"].append({"label": label, **movement})
            position = movement["position"]
            prior_positions.append(position)
            result["captures"].append(
                {
                    "label": label,
                    "position": position,
                    "screenshots": capture_views(
                        mcp, screenshots_dir, label, position
                    ),
                }
            )

        mcp.call_tool("minecraft_disconnect")
        result["passed"] = True
        return result
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        helpers.stop_process(client_process)
        helpers.stop_process(server, interrupt=True)
        helpers.stop_process(xvfb)
        for handle in (client_log, server_log, xvfb_log):
            if handle is not None:
                handle.close()
        (run_dir / "result.json").write_text(
            json.dumps(result, indent=2, sort_keys=True) + "\n"
        )


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    args = parser.parse_args()
    try:
        result = run(args.timeout_seconds)
    except Exception as error:
        print(f"seed owner review failed: {error}", file=os.sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
