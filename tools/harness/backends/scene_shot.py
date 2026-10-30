"""Script a windowed native-client scene on an isolated v2 world."""
from __future__ import annotations

import json
import math
import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import prepare_operator_fixture
from .probe_steps import ProbeSteps

USERNAME = "SceneShot"



def wait_play(probe: ProbeSteps) -> None:
    # Each MCP wait is capped at 120s; keep the same overall scenario deadline.
    while True:
        result = probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
        if result.get("in_play") is True:
            return
        if result.get("disconnect_reason"):
            raise RuntimeError(f"client disconnected before play: {result['disconnect_reason']}")


def execute(probe: ProbeSteps, artifact: Path, steps: list[dict[str, Any]]) -> list[str]:
    shots = []
    for step in steps:
        if not isinstance(step, dict) or len(step) != 1:
            raise ValueError(f"invalid scene step: {step!r}")
        kind, value = next(iter(step.items()))
        if kind in ("shot", "observe"):
            if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", value):
                raise ValueError(f"invalid {kind} name: {value!r}")
            if kind == "shot":
                shots.append(native_client.capture(probe.client, artifact, value)["retained_path"])
            else:
                (artifact / f"{value}.json").write_text(json.dumps(probe.observe(), indent=2) + "\n")
        elif kind == "cmd" and isinstance(value, str) and value and not value.startswith("/"):
            wait_play(probe)
            probe.call("minecraft_send_chat", {"message": value, "command": True})
        elif kind == "tp" and isinstance(value, list) and len(value) == 5 and all(type(n) in (int, float) and math.isfinite(n) for n in value):
            probe.call("minecraft_send_chat", {"message": "tp @s " + " ".join(map(str, value)), "command": True})
            probe.wait(
                lambda state: state.get("player") is not None
                and abs(state["player"]["x"] - value[0]) < 1.5
                and abs(state["player"]["z"] - value[2]) < 1.5,
                "teleport_arrived", wait_for_play=True,
            )
        elif kind == "wait_ticks" and type(value) is int and value > 0:
            remaining = value
            while remaining:
                ticks = min(remaining, 100)
                probe.call("minecraft_wait_ticks", {"ticks": ticks})
                remaining -= ticks
        elif kind == "wait_play" and value is True:
            wait_play(probe)
        elif kind == "key" and isinstance(value, dict) and set(value) == {"keys", "ticks"}:
            probe.call("minecraft_press_inputs", value)
        elif kind == "use_item_on" and isinstance(value, dict):
            probe.call("minecraft_use_item_on", value)
        elif kind == "click_screen_button" and isinstance(value, dict):
            probe.call("minecraft_click_screen_button", value)
        elif kind in ("scan_blocks", "read_block", "navigate_to_block", "list_entities") and isinstance(value, dict):
            name, args = value.get("name"), value.get("args")
            if (set(value) != {"name", "args"} or not isinstance(name, str)
                    or not re.fullmatch(r"[A-Za-z0-9_-]+", name) or not isinstance(args, dict)):
                raise ValueError(f"invalid {kind} step: {value!r}")
            result = probe.call("minecraft_" + kind, args)
            (artifact / f"{name}.json").write_text(json.dumps(result, indent=2) + "\n")
        else:
            raise ValueError(f"invalid scene step: {step!r}")
    return shots


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native" or not ctx.windowed or ctx.platform is not None:
        raise ValueError("scene-shot requires --client native --windowed")
    args = ctx.extra_args
    if args and args[0] == "--":
        args = args[1:]
    if len(args) > 1:
        raise ValueError("scene-shot accepts one scenario JSON path")
    scenario_path = args[0] if args else os.environ.get("SCENE_SHOT_SCENARIO")
    if not scenario_path:
        raise ValueError("set SCENE_SHOT_SCENARIO or pass a scenario JSON path")
    scenario = json.loads(Path(scenario_path).read_text())
    if not isinstance(scenario, dict) or not isinstance(scenario.get("steps"), list):
        raise ValueError("scenario must contain a steps array")
    seed = scenario.get("seed", 100)
    if type(seed) is not int:
        raise ValueError("seed must be an integer")
    server_view = scenario.get("server_view_distance", 4)
    if type(server_view) is not int or not 2 <= server_view <= 32:
        raise ValueError("server_view_distance must be in 2..32")
    settings = scenario.get("client_settings", {})
    if not isinstance(settings, dict) or set(settings) - {"graphics_preset", "render_distance"}:
        raise ValueError("unsupported client settings")
    if "graphics_preset" in settings:
        preset = settings["graphics_preset"]
        if not isinstance(preset, str) or preset.lower() not in ("low", "medium", "high"):
            raise ValueError("graphics_preset must be Low, Medium or High")
        settings["graphics_preset"] = preset.lower()
    if "render_distance" in settings and (type(settings["render_distance"]) is not int or not 2 <= settings["render_distance"] <= 32):
        raise ValueError("render_distance must be in 2..32")
    binary_text = os.environ.get("SOLARIS_SERVER_BINARY")
    if not binary_text:
        raise RuntimeError("scene-shot requires SOLARIS_SERVER_BINARY")
    binary = Path(binary_text).expanduser().resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError(f"missing executable server binary: {binary}")
    timeout = ctx.timeout_seconds or 600.0
    if not 0 < timeout <= 3600:
        raise ValueError("timeout must be in (0, 3600]")
    artifact = ctx.artifact_dir
    port = runtime.reserve_port()
    config = artifact / "scene-shot.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (("[server]", "[server]\nspawn_protection = 0"),
                    ("view_distance = 4", f"view_distance = {server_view}"),
                    ("port = 25565", f"port = {port}"),
                    ('world_dir = ".analysis/test-world-v12"', f'world_dir = {json.dumps(str(artifact / "world"))}'),
                    ('directory = "plugins"', f'directory = {json.dumps(str(artifact / "plugins"))}'),
                    ("seed = 0", f"seed = {seed}"))
    for old, new in replacements:
        if source.count(old) != 1:
            raise RuntimeError(f"expected one config token: {old}")
        source = source.replace(old, new, 1)
    config.write_text(source)
    if scenario.get("world"):
        shutil.copytree(Path(scenario["world"]).expanduser().resolve(), artifact / "world")
    (artifact / "plugins").mkdir()
    prepare_operator_fixture(ctx, [USERNAME])
    xdg = artifact / "xdg"
    settings_path = xdg / "solaris-client" / "client-settings.json"
    source_root = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "solaris-client"
    if source_root.is_dir():
        shutil.copytree(source_root, settings_path.parent, ignore_dangling_symlinks=True)
    else:
        settings_path.parent.mkdir(parents=True)
    original = os.environ.get("SOLARIS_CLIENT_SETTINGS")
    if original and Path(original).is_file():
        base = json.loads(Path(original).read_text())
    else:
        base_path = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "solaris-client" / "client-settings.json"
        base = json.loads(base_path.read_text()) if base_path.is_file() else {}
    base.update(settings)
    settings_path.write_text(json.dumps(base, indent=2) + "\n")
    server = client = xvfb = mcp = log = client_log = None
    server_index = len(ctx.commands)
    server_command = [str(binary), "--config", str(config), "--no-console"]
    ctx.commands.append(server_command)
    ctx.cwds.append(str(runtime.REPO_ROOT))
    ctx.exits.append(None)
    try:
        log = (artifact / "server.log").open("wb")
        ctx.logs.append(str(artifact / "server.log"))
        server = subprocess.Popen(server_command, cwd=runtime.REPO_ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        runtime.wait_port(port, timeout, server, log_path=artifact / "server.log")
        xvfb, display, _ = runtime.start_xvfb(artifact)
        ctx.logs.append(str(artifact / "xvfb.log"))
        old_xdg = os.environ.get("XDG_DATA_HOME")
        old_settings = os.environ.get("SOLARIS_CLIENT_SETTINGS")
        try:
            os.environ["XDG_DATA_HOME"] = str(xdg)
            os.environ["SOLARIS_CLIENT_SETTINGS"] = str(settings_path)
            client, client_log, mcp = native_client.start(ctx, native_client.resolve_binary(ctx), f"127.0.0.1:{port}", USERNAME, timeout, display=display)
        finally:
            for key, old in (("XDG_DATA_HOME", old_xdg), ("SOLARIS_CLIENT_SETTINGS", old_settings)):
                if old is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = old
        mcp.request_timeout_seconds = min(timeout + 10, 130.0)
        with ProbeSteps(mcp, artifact / "scene-shot", timeout, "scene-shot") as probe:
            wait_play(probe)
            from . import native_focus
            ctx.client_evidence["window_focus"] = native_focus.acquire(display, client.pid, probe.remaining())
            before = probe.observe()
            probe.call("minecraft_wait_for_state_change", {
                "observed_version": before["state_version"], "timeout_seconds": probe.remaining(),
            })
            shots = execute(probe, artifact, scenario["steps"])
        return {"screenshots": shots, "scenario": probe.report, "seed": seed, "settings": settings}
    finally:
        if mcp is not None:
            mcp.close()
        runtime.stop_process(client)
        if client is not None:
            ctx.exits[ctx.client_evidence["command_index"]] = client.returncode
        if client_log is not None:
            client_log.close()
        runtime.stop_process(server, interrupt=True)
        if server is not None:
            ctx.exits[server_index] = server.returncode
        runtime.stop_process(xvfb)
        if log is not None:
            log.close()
