#!/usr/bin/env python3
"""Real-client compatibility gate for server-only and Loader-required plugins.

Harness scenario module. The caller creates ``artifact_dir`` and builds
``mc-server``; this module owns live server/client lifecycle and cleanup via
the shared runtime and never writes the canonical top-level ``result.json``.
"""

from __future__ import annotations

import json
import os
import runpy
import shutil
import subprocess
import time
import tomllib
from pathlib import Path
from typing import Any

from . import precommit, runtime
from .mcp import McpClient

REPO_ROOT = runtime.REPO_ROOT

INVENTORY_SCENARIO_ID = "m94-03a-inventory-oak-log-to-planks"


def observe_screen_class(observed: dict[str, Any]) -> str:
    screen = observed.get("screen")
    if isinstance(screen, dict):
        return str(screen.get("class", ""))
    return str(observed.get("current_screen", ""))



def recent_chat_contains(observed: dict[str, Any], expected: str) -> bool:
    recent = observed.get("recent_chat")
    return isinstance(recent, list) and any(expected in str(message) for message in recent)

def wait_for_disconnect(client: Any, timeout_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last = runtime.current_client_state(client, min(timeout_seconds, 120.0))
    while True:
        reason = str(last.get("disconnect_reason") or "")
        if reason:
            return last
        if last.get("in_play"):
            raise RuntimeError("no-Loader client unexpectedly entered Play on client-required server")
        if time.monotonic() >= deadline:
            break
        last = runtime.next_client_state(client, last, deadline)
    raise RuntimeError(
        "client-required disconnect was not observed: "
        + json.dumps(last, ensure_ascii=False, sort_keys=True)
    )


def run_inventory_probe(mcp: Any, run_dir: Path) -> dict[str, Any]:
    """Native oak-log crafting scenario plus cursor pickup/return and selection."""
    mcp.call_tool("minecraft_close_screen")
    screenshots_dir = run_dir / "client-screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)
    report = mcp.call_tool(
        "minecraft_run_scenario",
        {"id": INVENTORY_SCENARIO_ID, "artifacts_dir": str(screenshots_dir)},
    )
    (run_dir / "crafting-scenario.json").write_text(json.dumps(report, indent=2) + "\n")
    if report.get("result") != "passed":
        raise RuntimeError(
            "inventory crafting scenario failed: " + json.dumps(report, ensure_ascii=False)
        )
    mcp.call_tool("minecraft_open_inventory")
    mcp.call_tool(
        "minecraft_click_container_slot",
        {"slot": 9, "button": "primary", "timeout_seconds": 20.0},
    )
    mcp.call_tool(
        "minecraft_wait_for_inventory",
        {"item_id": "minecraft:apple", "count": 0, "timeout_seconds": 20.0},
    )
    mcp.call_tool(
        "minecraft_click_container_slot",
        {"slot": 9, "button": "primary", "timeout_seconds": 20.0},
    )
    mcp.call_tool(
        "minecraft_wait_for_inventory",
        {"item_id": "minecraft:apple", "count": 2, "timeout_seconds": 20.0},
    )
    mcp.call_tool("minecraft_close_screen")
    mcp.call_tool(
        "minecraft_select_hotbar_item",
        {"item_id": "minecraft:oak_planks", "count": 4, "timeout_seconds": 20.0},
    )
    mcp.call_tool(
        "minecraft_wait_for_inventory",
        {"item_id": "minecraft:oak_planks", "count": 4, "timeout_seconds": 20.0},
    )
    return {
        "crafting": report,
        "crafting_scenario": INVENTORY_SCENARIO_ID,
        "cursor_roundtrip_apples": 2,
        "selected_crafted_planks": 4,
    }




def run_wasm_server_only(
    root: Path, display: str, timeout_seconds: float
) -> dict[str, Any]:
    """Exercise the real component runtime with a graphical client that has no Loader."""
    run_dir = root / "wasm-server-only"
    package = run_dir / "plugins" / "hello"
    package.mkdir(parents=True)
    fixture = REPO_ROOT / ".analysis" / "loader-live-gate" / "plugins" / "ruby-live"
    api = tomllib.loads((fixture / "plugin.toml").read_text())["api"]
    shutil.copyfile(fixture / "plugin.wasm", package / "plugin.wasm")
    (package / "plugin.toml").write_text(
        f'id = "hello"\nname = "hello"\nversion = "0.1.0"\napi = "{api}"\n'
        'events = ["player.joined"]\nplayer_commands = ["hello"]\n'
    )
    (package / "config.toml").write_text('greeting = "WASM server-only"\n')
    (package / "rules.lua").write_text("this is not valid Luau source\n")
    server_port = runtime.reserve_port()
    mcp_port = runtime.reserve_port()
    config = run_dir / "server.toml"
    config.write_text(
        f"""[server]
name = "wasm-no-loader-gate"
motd = "WASM server-only compatibility"
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {server_port}
[auth]
online_mode = false
[plugins]
directory = "{(run_dir / 'plugins').relative_to(REPO_ROOT).as_posix()}"
strict = true
expected = ["hello"]
[data]
world_dir = "{(run_dir / 'world').relative_to(REPO_ROOT).as_posix()}"
seed = 0
worldgen_mode = "tellus_like"
[simulation]
random_tick_speed = 0
friendly_spawn_interval_ticks = 0
hostile_spawn_interval_ticks = 0
[autoscale]
enabled = false
"""
    )
    token = f"wasm-no-loader-{time.time_ns()}"
    server_log_path = run_dir / "server.log"
    server_log = server_log_path.open("wb")
    client_log_handle = None
    server = client_process = None
    mcp = None
    try:
        server = subprocess.Popen(
            [str(REPO_ROOT / "target" / "debug" / "mc-server"),
             "--config", str(config), "--no-console"],
            cwd=REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        runtime.wait_port(
            server_port, min(timeout_seconds, 120.0), server, log_path=server_log_path
        )
        game_dir = run_dir / "game"
        game_dir.mkdir()
        (game_dir / "options.txt").write_text("version:4790\nonboardAccessibility:false\n")
        client_process, client_log_handle = runtime.start_client(
            game_dir=game_dir,
            log_path=run_dir / "client.log",
            token=token,
            mcp_port=mcp_port,
            username="NoLoaderWasm",
            display=display,
            platform=None,
        )
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp", token, request_timeout_seconds=30.0
        )
        mcp.initialize()
        runtime.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        mcp.call_tool("minecraft_connect", {"server_addr": f"127.0.0.1:{server_port}"})
        play = mcp.call_tool(
            "minecraft_wait_for_play",
            {"timeout_seconds": min(timeout_seconds, 120.0)},
        )
        if not play.get("in_play"):
            raise RuntimeError(f"WASM/no-Loader client did not enter Play: {play}")
        observed = runtime.current_client_state(mcp, 30.0)
        deadline = time.monotonic() + 30.0
        greeting = "WASM server-only NoLoaderWasm"
        while not recent_chat_contains(observed, greeting):
            observed = runtime.next_client_state(mcp, observed, deadline)
        mcp.call_tool("minecraft_send_chat", {"message": "hello", "command": True})
        deadline = time.monotonic() + 30.0
        reply = "Hello from a WASM plugin."
        while not recent_chat_contains(observed, reply):
            observed = runtime.next_client_state(mcp, observed, deadline)
        deadline = time.monotonic() + 60.0
        while observed["screen"]["open"] or observed.get("overlay", {}).get("open"):
            observed = runtime.next_client_state(mcp, observed, deadline)
        mcp.call_tool("minecraft_open_inventory")
        observed = runtime.current_client_state(mcp, 30.0)
        deadline = time.monotonic() + 30.0
        while not observe_screen_class(observed).endswith("InventoryScreen"):
            observed = runtime.next_client_state(mcp, observed, deadline)
        screenshots = run_dir / "screenshots"
        screenshots.mkdir()
        mcp.call_tool("minecraft_wait_ticks", {"ticks": 1})
        capture = mcp.call_tool(
            "minecraft_screenshot", {"path": str(screenshots / "wasm-no-loader.png")}
        )
        mcp.call_tool("minecraft_close_screen")
        mcp.call_tool("minecraft_disconnect")
        return {
            "passed": True,
            "plugin_runtime": "wasm",
            "loader_platform": None,
            "in_play": True,
            "greeting": greeting,
            "command_reply": reply,
            "inventory_screen": observe_screen_class(observed),
            "invalid_rules_lua_ignored": True,
            "screenshot": capture,
        }
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        if client_log_handle is not None:
            client_log_handle.close()
        server_log.close()


def run_client_required_rejection(
    root: Path,
    display: str,
    timeout_seconds: float,
) -> dict[str, Any]:
    run_dir = root / "client-required-rejection"
    run_dir.mkdir()
    mcp_port = runtime.reserve_port()
    token = f"client-required-{time.time_ns()}"
    world_dir = run_dir / "world"
    world_dir.mkdir()
    config = run_dir / "playable.toml"
    text = (REPO_ROOT / "examples" / "loader-live-gate" / "playable.toml").read_text()
    text = text.replace(
        'world_dir = ".analysis/loader-live-gate/world"',
        f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
    )
    config.write_text(text)
    server_log_path = run_dir / "server.log"
    server_log = server_log_path.open("wb")
    client_log_handle = None
    server = client_process = None
    mcp = None
    try:
        server = subprocess.Popen(
            [str(REPO_ROOT / "target" / "debug" / "mc-server"), "--config", str(config)],
            cwd=REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        runtime.wait_port(25567, min(timeout_seconds, 60.0), server, log_path=server_log_path)
        game_dir = run_dir / "game"
        game_dir.mkdir()
        (game_dir / "options.txt").write_text("version:4790\nonboardAccessibility:false\n")
        client_process, client_log_handle = runtime.start_client(
            game_dir=game_dir,
            log_path=run_dir / "client.log",
            token=token,
            mcp_port=mcp_port,
            username="NoLoaderGate",
            display=display,
            platform=None,
        )
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=30.0,
        )
        mcp.initialize()
        runtime.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        mcp.call_tool("minecraft_connect", {"server_addr": "127.0.0.1:25567"})
        observed = wait_for_disconnect(mcp, 30.0)
        reason = str(observed.get("disconnect_reason") or "")
        required_reason_terms = [
            "Solaris Loader",
            "Fabric",
            "NeoForge",
            "Forge",
            "ruby-live:rich-content@1",
            "sapphire-live:rich-content@1",
        ]
        missing_reason_terms = [term for term in required_reason_terms if term not in reason]
        if missing_reason_terms:
            raise RuntimeError(
                "disconnect reason omitted required Loader contract terms "
                f"{missing_reason_terms}: {reason!r}"
            )
        return {
            "passed": True,
            "mcp_port": mcp_port,
            "in_play": False,
            "disconnect_reason": reason,
            "game_dir": str(game_dir.relative_to(REPO_ROOT)),
        }
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        if client_log_handle is not None:
            client_log_handle.close()
        server_log.close()


def run(
    timeout_seconds: float,
    artifact_dir: Path,
    *,
    inventory: bool = False,
) -> dict[str, Any]:
    result: dict[str, Any] = {"inventory": inventory}
    xvfb = None
    xvfb_log = None
    try:
        xvfb, display, xvfb_log = runtime.start_xvfb(artifact_dir)
        result["wasm_server_only"] = run_wasm_server_only(
            artifact_dir, display, timeout_seconds
        )
        result["wasm_precommit"] = precommit.run(
            artifact_dir, display, timeout_seconds
        )
        plugins_root = Path(
            os.environ.get(
                "SOLARIS_DEFAULT_PLUGINS_ROOT",
                str(REPO_ROOT.parent / "solaris-default-plugins"),
            )
        )
        economy_gate = runpy.run_path(str(plugins_root / "tools" / "basic_economy_client.py"))
        result["server_only"] = economy_gate["run_server_only"](
            artifact_dir, display, timeout_seconds, inventory=inventory
        )
        result["client_required_rejection"] = run_client_required_rejection(
            artifact_dir, display, timeout_seconds
        )
        result["passed"] = True
        return result
    finally:
        runtime.stop_process(xvfb)
        if xvfb_log is not None:
            xvfb_log.close()
        (artifact_dir / "scenario.json").write_text(
            json.dumps(result, indent=2, sort_keys=True) + "\n"
        )
