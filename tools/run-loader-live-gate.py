#!/usr/bin/env python3
"""Run the real Solaris Loader two-owner compatibility gate for one client platform."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
SERVER_ADDRESS = "127.0.0.1:25567"
CONFIRMATION_TITLES = (
    f"Allow Solaris content from {SERVER_ADDRESS}?",
    "Allow Solaris content from localhost:25567?",
)
OWNER_SCREENS = (
    ("loader_ruby", "Ruby Loader Fixture", "Confirm Ruby", 1),
    ("loader_sapphire", "Sapphire Loader Fixture", "Confirm Sapphire", 2),
)
SCREEN_CLASSES = {
    "fabric": "dev.solaris.loader.fabric.LoaderTextScreen",
    "neoforge": "dev.solaris.loader.neoforge.LoaderTextScreen",
    "forge": "dev.solaris.loader.forge.LoaderTextScreen",
}
EXPECTED_BUNDLE_CACHE_FILES = [
    "ruby-live/rich-content/1/70dd527ac0c5075faf1dff65e8e426f657746d42215e4fc4fd18244ac5b9d765.bundle",
    "sapphire-live/rich-content/1/6c16425b2bf9c5415184345c4cb6bc10e98bf41a3e73dc27b3915aa7962418a5.bundle",
]


def load_mcp_client():
    module_path = REPO_ROOT / "tools" / "minecraft-client-mcp-smoke.py"
    spec = importlib.util.spec_from_file_location("solaris_minecraft_mcp_smoke", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load MCP client from {module_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.McpClient


def reserve_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def wait_port(port: int, timeout_seconds: float, process: subprocess.Popen[Any]) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if port_open(port):
            return
        if process.poll() is not None:
            raise RuntimeError(f"process exited before port {port} became ready: {process.returncode}")
        remaining = deadline - time.monotonic()
        try:
            process.wait(timeout=min(0.2, max(0.0, remaining)))
        except subprocess.TimeoutExpired:
            continue
        raise RuntimeError(f"process exited before port {port} became ready: {process.returncode}")
    raise RuntimeError(f"port {port} did not become ready within {timeout_seconds:.1f}s")


def stop_process(process: subprocess.Popen[Any] | None, *, interrupt: bool = False) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGINT if interrupt else signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def start_xvfb(artifact_dir: Path) -> tuple[subprocess.Popen[Any], str, Any]:
    log = (artifact_dir / "xvfb.log").open("wb")
    for display_number in range(99, 110):
        process = subprocess.Popen(
            ["Xvfb", f":{display_number}", "-screen", "0", "1280x720x24", "-nolisten", "tcp"],
            cwd=REPO_ROOT,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            process.wait(timeout=0.25)
        except subprocess.TimeoutExpired:
            return process, f":{display_number}", log
    log.close()
    raise RuntimeError("could not start an isolated Xvfb display")


def current_client_state(client: Any, timeout_seconds: float) -> dict[str, Any]:
    del timeout_seconds
    observed = client.call_tool("minecraft_observe")
    version = observed.get("state_version")
    if not isinstance(version, int) or version < 0:
        raise RuntimeError(f"client observation omitted state_version: {observed}")
    return observed


def next_client_state(client: Any, observed: dict[str, Any], deadline: float) -> dict[str, Any]:
    version = observed.get("state_version")
    if not isinstance(version, int) or version < 0:
        raise RuntimeError(f"client state snapshot omitted state_version: {observed}")
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("client state event deadline elapsed")
    client.call_tool(
        "minecraft_wait_for_state_change",
        {"observed_version": version, "timeout_seconds": min(remaining, 120.0)},
    )
    return current_client_state(client, remaining)


def retry_tool(client: Any, name: str, arguments: dict[str, Any], timeout_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last_error: Exception | None = None
    observed: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        try:
            return client.call_tool(name, arguments)
        except Exception as error:  # MCP returns precise transient screen/state mismatch errors.
            last_error = error
            if observed is None:
                observed = current_client_state(client, min(timeout_seconds, 120.0))
            else:
                observed = next_client_state(client, observed, deadline)
    raise RuntimeError(f"{name} did not succeed within {timeout_seconds:.1f}s: {last_error}")


def wait_client_ready_for_connect(client: Any, timeout_seconds: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last = current_client_state(client, min(timeout_seconds, 120.0))
    while True:
        screen_payload = last.get("screen")
        if isinstance(screen_payload, dict):
            screen = str(screen_payload.get("class", ""))
        else:
            screen = str(last.get("current_screen", ""))
        if not last.get("in_play") and screen.endswith("TitleScreen"):
            return last
        if screen.endswith("LoadingErrorScreen"):
            raise RuntimeError(
                "Forge loading warning/error screen blocked bootstrap: "
                + json.dumps(last, ensure_ascii=False, sort_keys=True)
            )
        if time.monotonic() >= deadline:
            break
        last = next_client_state(client, last, deadline)
    raise RuntimeError(
        "client bootstrap did not reach TitleScreen before connect: "
        + json.dumps(last, ensure_ascii=False, sort_keys=True)
    )


def confirm_loader_permission(client: Any, timeout_seconds: float) -> str:
    deadline = time.monotonic() + timeout_seconds
    last_error: Exception | None = None
    observed: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        for title in CONFIRMATION_TITLES:
            try:
                client.call_tool(
                    "minecraft_click_confirmation_button",
                    {"expected_title": title, "button_label": "Allow"},
                )
                return title
            except Exception as error:
                last_error = error
        if observed is None:
            observed = current_client_state(client, min(timeout_seconds, 120.0))
        else:
            observed = next_client_state(client, observed, deadline)
    raise RuntimeError(
        f"minecraft_click_confirmation_button did not succeed within {timeout_seconds:.1f}s: {last_error}"
    )


def run(platform: str, timeout_seconds: float) -> dict[str, Any]:
    if platform not in SCREEN_CLASSES:
        raise RuntimeError(f"unsupported platform {platform!r}")
    if port_open(25567):
        raise RuntimeError("loader live-gate server port 25567 is already in use")

    run_id = f"{time.strftime('%Y%m%dT%H%M%S')}-{platform}"
    artifact_dir = REPO_ROOT / ".analysis" / "loader-live-gate" / "runs" / run_id
    artifact_dir.mkdir(parents=True, exist_ok=False)
    game_dir = artifact_dir / "game"
    game_dir.mkdir()
    # Keep the gate focused on Loader compatibility instead of Minecraft's first-run
    # accessibility onboarding. Vanilla's 26.1.2 options datafix uses false for an
    # already-onboarded profile; all other options still fall back to client defaults.
    (game_dir / "options.txt").write_text(
        "version:4790\nonboardAccessibility:false\n"
    )
    world_dir = artifact_dir / "world"
    world_dir.mkdir()
    run_config = artifact_dir / "playable.toml"
    config_text = (REPO_ROOT / "examples" / "loader-live-gate" / "playable.toml").read_text()
    config_text = config_text.replace(
        'world_dir = ".analysis/loader-live-gate/world"',
        f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
    )
    run_config.write_text(config_text)
    token = f"solaris-loader-{platform}-{os.getpid()}-{time.time_ns()}"
    mcp_port = reserve_port()
    username = {"fabric": "GateFabric", "neoforge": "GateNeoForge", "forge": "GateForge"}[platform]

    xvfb = server = client_process = None
    xvfb_log = server_log = client_log = None
    mcp = None
    result: dict[str, Any] = {
        "platform": platform,
        "run_id": run_id,
        "artifact_dir": str(artifact_dir.relative_to(REPO_ROOT)),
        "game_dir": str(game_dir.relative_to(REPO_ROOT)),
        "world_dir": str(world_dir.relative_to(REPO_ROOT)),
        "run_config": str(run_config.relative_to(REPO_ROOT)),
        "server_address": SERVER_ADDRESS,
        "mcp_port": mcp_port,
    }
    try:
        xvfb, display, xvfb_log = start_xvfb(artifact_dir)
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

        server_log = (artifact_dir / "server.log").open("wb")
        subprocess.run(
            ["cargo", "build", "-p", "mc-server"],
            cwd=REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            check=True,
        )
        server = subprocess.Popen(
            [str(REPO_ROOT / "target" / "debug" / "mc-server"), "--config", str(run_config)],
            cwd=REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_port(25567, min(timeout_seconds, 90.0), server)
        result["server_ready"] = True

        client_log = (artifact_dir / "client.log").open("wb")
        client_process = subprocess.Popen(
            [str(REPO_ROOT / "tools" / "run-loader-client-mcp.sh"), platform],
            cwd=REPO_ROOT,
            env=env,
            stdout=client_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        wait_port(mcp_port, min(timeout_seconds, 180.0), client_process)
        result["mcp_ready"] = True

        McpClient = load_mcp_client()
        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=max(30.0, min(timeout_seconds, 240.0) + 10.0),
        )
        initialized = mcp.initialize()
        tools = mcp.list_tools()
        result["protocol_version"] = initialized["protocolVersion"]
        result["tool_count"] = len(tools)

        before = wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
        before_screen = before.get("screen")
        result["client_ready_screen"] = (
            before_screen.get("class") if isinstance(before_screen, dict) else before.get("current_screen")
        )
        result["in_play_before_connect"] = bool(before.get("in_play"))
        mcp.call_tool("minecraft_connect", {"server_addr": SERVER_ADDRESS})
        result["permission_title"] = confirm_loader_permission(
            mcp, min(timeout_seconds, 90.0)
        )
        result["permission_confirmed"] = True

        push_wait_seconds = 5.0 if platform == "forge" else min(timeout_seconds, 120.0)
        play = mcp.call_tool(
            "minecraft_wait_for_play",
            {"timeout_seconds": push_wait_seconds},
        )
        if not play.get("in_play") and platform == "forge":
            result["forge_push_wait_snapshot"] = play
            play = mcp.call_tool(
                "minecraft_wait_for_play",
                {"timeout_seconds": min(timeout_seconds, 30.0)},
            )
            if play.get("in_play"):
                result["forge_direct_observe_recovered_play"] = True
        if not play.get("in_play"):
            raise RuntimeError(f"real {platform} client did not reach Play: {play}")
        result["in_play"] = True

        # A unique offline username keeps this gate's persisted inventory isolated.
        mcp.call_tool(
            "minecraft_wait_for_inventory",
            {"item_id": "minecraft:paper", "count": 0, "timeout_seconds": 10.0},
        )

        owner_results: list[dict[str, Any]] = []
        screen_class = SCREEN_CLASSES[platform]
        for command, title, button, expected_paper_count in OWNER_SCREENS:
            mcp.call_tool("minecraft_send_chat", {"message": command, "command": True})
            click = retry_tool(
                mcp,
                "minecraft_click_screen_button",
                {
                    "expected_screen_class": screen_class,
                    "expected_title": title,
                    "button_label": button,
                },
                30.0,
            )
            inventory = mcp.call_tool(
                "minecraft_wait_for_inventory",
                {
                    "item_id": "minecraft:paper",
                    "count": expected_paper_count,
                    "timeout_seconds": 20.0,
                },
            )
            try:
                mcp.call_tool("minecraft_close_screen")
            except Exception:
                pass
            owner_results.append(
                {
                    "command": command,
                    "screen_title": title,
                    "button": button,
                    "button_clicked": True,
                    "inventory": inventory,
                    "click": click,
                }
            )
        result["owners"] = owner_results
        result["in_play_after_owner_actions"] = bool(mcp.call_tool("minecraft_observe").get("in_play"))
        if not result["in_play_after_owner_actions"]:
            raise RuntimeError("client left Play during Loader owner actions")

        cache_dir = game_dir / "solaris-loader-cache"
        permissions = cache_dir / "permissions.properties"
        bundles = sorted(cache_dir.rglob("*.bundle")) if cache_dir.exists() else []
        bundle_cache_files = [str(path.relative_to(cache_dir)) for path in bundles]
        result["cache_dir"] = str(cache_dir.relative_to(REPO_ROOT))
        result["permission_file_exists"] = permissions.is_file()
        result["bundle_cache_count"] = len(bundles)
        result["bundle_cache_files"] = bundle_cache_files
        if not result["permission_file_exists"]:
            raise RuntimeError("Loader permission decision was not stored in the isolated game-dir cache")
        if bundle_cache_files != EXPECTED_BUNDLE_CACHE_FILES:
            raise RuntimeError(
                "Loader cache identities do not match the exact Ruby/Sapphire fixture: "
                + json.dumps(bundle_cache_files)
            )

        mcp.call_tool("minecraft_disconnect")
        result["passed"] = True
        (artifact_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        return result
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        stop_process(client_process)
        stop_process(server, interrupt=True)
        stop_process(xvfb)
        for handle in (client_log, server_log, xvfb_log):
            if handle is not None:
                handle.close()
        if not result.get("passed"):
            (artifact_dir / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=sorted(SCREEN_CLASSES))
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        result = run(args.platform, args.timeout_seconds)
    except Exception as error:
        print(f"Loader live gate failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
