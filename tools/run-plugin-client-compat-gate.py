#!/usr/bin/env python3
"""Real-client compatibility gate for server-only and Loader-required plugins."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load module from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def reserve_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def write_server_only_config(run_dir: Path, port: int) -> Path:
    plugin_root = run_dir / "plugins"
    plugin_root.mkdir()
    shutil.copytree(
        REPO_ROOT / "examples" / "plugins" / "basic-economy",
        plugin_root / "basic-economy",
    )
    economy_config = plugin_root / "basic-economy" / "config.toml"
    economy_text = economy_config.read_text()
    economy_text = economy_text.replace('resource = "minecraft:emerald"', 'resource = "minecraft:dirt"', 1)
    economy_text = economy_text.replace('singular = "Emerald"', 'singular = "Dirt"', 1)
    economy_text = economy_text.replace('plural = "Emeralds"', 'plural = "Dirt"', 1)
    economy_text = economy_text.replace("price = 3", "price = 1", 1)
    economy_config.write_text(economy_text)
    world_dir = run_dir / "world"
    world_dir.mkdir()
    config = run_dir / "server-only.toml"
    config.write_text(
        "\n".join(
            [
                "[server]",
                'name = "server-only-plugin-gate"',
                'motd = "Solaris server-only plugin real-client gate"',
                "view_distance = 4",
                "simulation_distance = 4",
                "",
                "[network]",
                'bind_address = "127.0.0.1"',
                f"port = {port}",
                "",
                "[auth]",
                "online_mode = false",
                "whitelist_enabled = false",
                "whitelist = []",
                "banned_players = []",
                "",
                "[admin]",
                "operators = []",
                "allow_local_dev_operators = false",
                "",
                "[plugins]",
                f'directory = "{plugin_root.relative_to(REPO_ROOT).as_posix()}"',
                "",
                "[data]",
                f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
                'vanilla_data_dir = "data/vanilla"',
                "seed = 0",
                'worldgen_mode = "tellus_like"',
                "",
                "[simulation]",
                "random_tick_speed = 0",
                "save_interval_ticks = 1200",
                "friendly_spawn_interval_ticks = 0",
                "hostile_spawn_interval_ticks = 0",
                "",
                "[chunk_pipeline]",
                "chunk_send_rate = 8",
                "chunk_load_rate = 16",
                "chunk_generate_rate = 16",
                "chunk_prepare_budget_ms = 0",
                "chunk_prepare_batch_size = 8",
                "chunk_result_queue_size = 64",
                "region_cache_size = 9",
                "",
                "[autoscale]",
                "enabled = false",
                'profile = "balanced"',
                "min_view_distance = 4",
                "max_view_distance = 4",
                "",
            ]
        )
    )
    return config


def start_no_loader_client(
    helpers: Any,
    run_dir: Path,
    token: str,
    mcp_port: int,
    username: str,
    display: str,
):
    game_dir = run_dir / "game"
    game_dir.mkdir()
    (game_dir / "options.txt").write_text("version:4790\nonboardAccessibility:false\n")
    env = os.environ.copy()
    env.update(
        {
            "DISPLAY": display,
            "SOLARIS_CLIENT_MCP_TOKEN": token,
            "SOLARIS_CLIENT_MCP_PORT": str(mcp_port),
            "SOLARIS_CLIENT_MCP_GAME_DIR": str(game_dir),
            "SOLARIS_CLIENT_MCP_USERNAME": username,
        }
    )
    log = (run_dir / "client.log").open("wb")
    process = subprocess.Popen(
        [str(REPO_ROOT / "tools" / "run-minecraft-client-mcp.sh")],
        cwd=REPO_ROOT,
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    helpers.wait_port(mcp_port, 180.0, process)
    return process, log, game_dir


def observe_screen_class(observed: dict[str, Any]) -> str:
    screen = observed.get("screen")
    if isinstance(screen, dict):
        return str(screen.get("class", ""))
    return str(observed.get("current_screen", ""))


def wait_for_economy_menu(helpers: Any, client: Any, timeout_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last = helpers.current_client_state(client, min(timeout_seconds, 120.0))
    while True:
        container = last.get("container")
        if (
            observe_screen_class(last).endswith("ContainerScreen")
            and isinstance(container, dict)
            and str(container.get("menu_class", "")).endswith("ChestMenu")
        ):
            return last
        if time.monotonic() >= deadline:
            break
        last = helpers.next_client_state(client, last, deadline)
    raise RuntimeError(
        "economy ChestMenu did not open: "
        + json.dumps(last, ensure_ascii=False, sort_keys=True)
    )


def find_nearby_dirt_currency(client: Any) -> dict[str, int]:
    observed = client.call_tool("minecraft_observe")
    player = observed.get("player")
    if not isinstance(player, dict):
        raise RuntimeError("player snapshot missing while searching for dirt currency")
    px = int(float(player["x"]) // 1)
    py = int(float(player["y"]) // 1)
    pz = int(float(player["z"]) // 1)
    scan = client.call_tool(
        "minecraft_scan_blocks",
        {
            "min_x": px - 8,
            "min_y": py - 5,
            "min_z": pz - 8,
            "max_x": px + 8,
            "max_y": py + 1,
            "max_z": pz + 8,
            "max_blocks": 2048,
        },
    )
    blocks = scan.get("blocks")
    if not isinstance(blocks, list):
        raise RuntimeError("block scan omitted blocks")
    indexed = {
        (int(block["x"]), int(block["y"]), int(block["z"])): block
        for block in blocks
        if isinstance(block, dict)
    }
    candidates: list[tuple[float, dict[str, Any]]] = []
    for (x, y, z), block in indexed.items():
        if block.get("block_id") not in {"minecraft:dirt", "minecraft:grass_block"}:
            continue
        above = indexed.get((x, y + 1, z))
        if not isinstance(above, dict) or not above.get("is_air"):
            continue
        distance = (x + 0.5 - float(player["x"])) ** 2 + (z + 0.5 - float(player["z"])) ** 2
        candidates.append((distance, block))
    if not candidates:
        raise RuntimeError("no reachable dirt/grass surface found in the loaded 17x7x17 scan")
    candidates.sort(key=lambda entry: entry[0])
    selected = candidates[0][1]
    return {"x": int(selected["x"]), "y": int(selected["y"]), "z": int(selected["z"])}


def recent_chat_contains(observed: dict[str, Any], expected: str) -> bool:
    recent = observed.get("recent_chat")
    return isinstance(recent, list) and any(expected in str(message) for message in recent)


def economy_menu_owned_count(observed: dict[str, Any], expected: int) -> bool:
    container = observed.get("container")
    if not isinstance(container, dict):
        return False
    slots = container.get("slots")
    if not isinstance(slots, list) or not slots:
        return False
    first = slots[0]
    return isinstance(first, dict) and f"owned {expected}" in str(first.get("name", ""))


def wait_for_disconnect(helpers: Any, client: Any, timeout_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last = helpers.current_client_state(client, min(timeout_seconds, 120.0))
    while True:
        reason = str(last.get("disconnect_reason") or "")
        if reason:
            return last
        if last.get("in_play"):
            raise RuntimeError("no-Loader client unexpectedly entered Play on client-required server")
        if time.monotonic() >= deadline:
            break
        last = helpers.next_client_state(client, last, deadline)
    raise RuntimeError(
        "client-required disconnect was not observed: "
        + json.dumps(last, ensure_ascii=False, sort_keys=True)
    )


def run_server_only(
    helpers: Any,
    McpClient: Any,
    root: Path,
    display: str,
    timeout_seconds: float,
) -> dict[str, Any]:
    run_dir = root / "server-only"
    run_dir.mkdir()
    server_port = reserve_port()
    mcp_port = reserve_port()
    config = write_server_only_config(run_dir, server_port)
    token = f"server-only-{os.getpid()}-{time.time_ns()}"
    server_log = (run_dir / "server.log").open("wb")
    client_log = None
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
        helpers.wait_port(server_port, min(timeout_seconds, 60.0), server)
        client_process, client_log, game_dir = start_no_loader_client(
            helpers, run_dir, token, mcp_port, "ServerOnlyGate", display
        )
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=30.0,
        )
        mcp.initialize()
        helpers.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        mcp.call_tool("minecraft_connect", {"server_addr": f"127.0.0.1:{server_port}"})
        play = mcp.call_tool(
            "minecraft_wait_for_play",
            {"timeout_seconds": min(timeout_seconds, 120.0)},
        )
        if not play.get("in_play"):
            raise RuntimeError(f"server-only client did not reach Play: {play}")

        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:dirt", "count": 0, "timeout_seconds": 10.0},
        )
        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:apple", "count": 0, "timeout_seconds": 10.0},
        )
        dirt_target = find_nearby_dirt_currency(mcp)
        mcp.call_tool(
            "minecraft_navigate_to_block",
            {**dirt_target, "timeout_seconds": 30.0},
        )
        break_result = mcp.call_tool(
            "minecraft_break_block",
            {
                **dirt_target,
                "face": "up",
                "expected_drop_item_id": "minecraft:dirt",
                "expected_drop_count": 1,
                "timeout_seconds": 30.0,
            },
        )
        if not break_result.get("pickup_confirmed"):
            raise RuntimeError(f"natural dirt currency was not picked up: {break_result}")
        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:dirt", "count": 1, "timeout_seconds": 20.0},
        )

        mcp.call_tool("minecraft_send_chat", {"message": "economy", "command": True})
        menu = wait_for_economy_menu(helpers, mcp, 20.0)
        if not economy_menu_owned_count(menu, 0):
            raise RuntimeError("fresh economy ledger did not render owned 0 for the first product")
        first_slot = menu.get("container", {}).get("slots", [{}])[0]
        if "buy 1 Dirt" not in str(first_slot.get("name", "")):
            raise RuntimeError(f"economy gate config did not expose the one-dirt purchase: {first_slot}")
        mcp.call_tool(
            "minecraft_click_container_slot",
            {"slot": 0, "button": "primary", "timeout_seconds": 20.0},
        )
        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:dirt", "count": 0, "timeout_seconds": 20.0},
        )
        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:apple", "count": 2, "timeout_seconds": 20.0},
        )
        committed = mcp.call_tool("minecraft_observe")
        if not recent_chat_contains(committed, "Purchased Apples."):
            raise RuntimeError(
                "economy purchase committed inventory but did not publish the plugin success message"
            )

        mcp.call_tool("minecraft_send_chat", {"message": "economy", "command": True})
        refreshed_menu = wait_for_economy_menu(helpers, mcp, 20.0)
        if not economy_menu_owned_count(refreshed_menu, 1):
            raise RuntimeError("durable economy ledger did not refresh to owned 1 after purchase")
        mcp.call_tool("minecraft_disconnect")
        return {
            "passed": True,
            "server_port": server_port,
            "mcp_port": mcp_port,
            "in_play": True,
            "plugin": "basic-economy",
            "economy_menu_screen": observe_screen_class(menu),
            "game_dir": str(game_dir.relative_to(REPO_ROOT)),
            "gameplay_loop": {
                "currency": "minecraft:dirt",
                "currency_source": "natural_dirt_or_grass_block",
                "currency_target": dirt_target,
                "break_result": break_result,
                "purchase_slot": 0,
                "currency_after_purchase": 0,
                "apples_after_purchase": 2,
                "success_message": "Purchased Apples.",
                "ledger_owned_after_purchase": 1,
            },
        }
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        helpers.stop_process(client_process)
        helpers.stop_process(server, interrupt=True)
        if client_log is not None:
            client_log.close()
        server_log.close()


def run_client_required_rejection(
    helpers: Any,
    McpClient: Any,
    root: Path,
    display: str,
    timeout_seconds: float,
) -> dict[str, Any]:
    run_dir = root / "client-required-rejection"
    run_dir.mkdir()
    mcp_port = reserve_port()
    token = f"client-required-{os.getpid()}-{time.time_ns()}"
    world_dir = run_dir / "world"
    world_dir.mkdir()
    config = run_dir / "playable.toml"
    text = (REPO_ROOT / "examples" / "loader-live-gate" / "playable.toml").read_text()
    text = text.replace(
        'world_dir = ".analysis/loader-live-gate/world"',
        f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
    )
    config.write_text(text)
    server_log = (run_dir / "server.log").open("wb")
    client_log = None
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
        helpers.wait_port(25567, min(timeout_seconds, 60.0), server)
        client_process, client_log, game_dir = start_no_loader_client(
            helpers, run_dir, token, mcp_port, "NoLoaderGate", display
        )
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=30.0,
        )
        mcp.initialize()
        helpers.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        mcp.call_tool("minecraft_connect", {"server_addr": "127.0.0.1:25567"})
        observed = wait_for_disconnect(helpers, mcp, 30.0)
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
        helpers.stop_process(client_process)
        helpers.stop_process(server, interrupt=True)
        if client_log is not None:
            client_log.close()
        server_log.close()


def run(timeout_seconds: float) -> dict[str, Any]:
    helpers = load_module("solaris_loader_live_gate", REPO_ROOT / "tools" / "run-loader-live-gate.py")
    McpClient = load_module(
        "solaris_minecraft_mcp_smoke", REPO_ROOT / "tools" / "minecraft-client-mcp-smoke.py"
    ).McpClient
    subprocess.run(["cargo", "build", "-p", "mc-server"], cwd=REPO_ROOT, check=True)
    run_id = time.strftime("%Y%m%dT%H%M%S")
    root = REPO_ROOT / ".analysis" / "plugin-client-compat" / run_id
    root.mkdir(parents=True, exist_ok=False)
    xvfb = None
    xvfb_log = None
    result: dict[str, Any] = {
        "run_id": run_id,
        "artifact_dir": str(root.relative_to(REPO_ROOT)),
    }
    try:
        xvfb, display, xvfb_log = helpers.start_xvfb(root)
        result["server_only"] = run_server_only(
            helpers, McpClient, root, display, timeout_seconds
        )
        result["client_required_rejection"] = run_client_required_rejection(
            helpers, McpClient, root, display, timeout_seconds
        )
        result["passed"] = True
        return result
    finally:
        helpers.stop_process(xvfb)
        if xvfb_log is not None:
            xvfb_log.close()
        (root / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    args = parser.parse_args()
    try:
        result = run(args.timeout_seconds)
    except Exception as error:
        print(f"Plugin client compatibility gate failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
