#!/usr/bin/env python3
"""Real Solaris Loader two-owner compatibility gate for one client platform.

Harness scenario module. The caller creates ``artifact_dir`` and builds
``mc-server``; this module owns live server/client lifecycle and cleanup via
the shared runtime and never writes the canonical top-level ``result.json``.
"""

from __future__ import annotations

import json
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from . import runtime
from .audio import SoundCapture, exercise_sounds
from .mcp import McpClient

REPO_ROOT = runtime.REPO_ROOT
SERVER_ADDRESS = "127.0.0.1:25567"
CONFIRMATION_TITLES = (
    f"Allow Solaris content from {SERVER_ADDRESS}?",
    "Allow Solaris content from localhost:25567?",
)
OWNER_SCREENS = (
    ("loader_ruby", "Ruby Loader Fixture", "Confirm Ruby", 1),
    ("loader_sapphire", "Sapphire Loader Fixture", "Confirm Sapphire", 2),
)
PLATFORMS = ("fabric", "neoforge", "forge")
SCREEN_CLASS = "dev.solaris.loader.minecraft.LoaderTextScreen"
EXPECTED_BUNDLE_CACHE_FILES = [
    "ruby-live/rich-content/1/8530c181143bc3f1bc4590b95738fae72fd6bf0eae38aac98ea1a3ee2e652158.bundle",
    "sapphire-live/rich-content/1/faadec262fb69554d684b0ed6f239dfa35e3c261b5d07b486535d7cfd45a4d5b.bundle",
]


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
            observed = runtime.current_client_state(client, min(timeout_seconds, 120.0))
        else:
            observed = runtime.next_client_state(client, observed, deadline)
    raise RuntimeError(
        f"minecraft_click_confirmation_button did not succeed within {timeout_seconds:.1f}s: {last_error}"
    )


def wait_chat_after(
    client: Any, before: dict[str, Any], messages: list[str], timeout_seconds: float = 30.0
) -> dict[str, Any]:
    observed = runtime.current_client_state(client, timeout_seconds)
    deadline = time.monotonic() + timeout_seconds
    while any(
        observed.get("recent_chat", []).count(message) <= before.get("recent_chat", []).count(message)
        for message in messages
    ):
        observed = runtime.next_client_state(client, observed, deadline)
    return observed


def input_status(client: Any, command: str, expected: str) -> dict[str, Any]:
    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_send_chat", {"message": f"{command} input_status", "command": True})
    return wait_chat_after(client, before, [expected])


def exercise_key_actions(client: Any, artifact_dir: Path) -> dict[str, Any]:
    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.g"], "ticks": 2})
    shared = wait_chat_after(client, before, [
        f"{owner} key {phase} #1."
        for owner in ("Ruby", "Sapphire") for phase in ("press", "release")
    ])
    if shared["screen"]["open"]:
        raise RuntimeError("declared key unexpectedly intercepted ordinary gameplay")
    shared_capture = capture_ui(client, artifact_dir, "shared-key-actions")

    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_send_chat", {"message": "loader_ruby input_modal", "command": True})
    armed = wait_chat_after(client, before, ["Ruby input modal armed."])
    with ThreadPoolExecutor(max_workers=1) as executor:
        held = executor.submit(
            client.call_tool, "minecraft_press_inputs", {"keys": ["key.keyboard.g"], "ticks": 180}
        )
        released = wait_chat_after(client, armed, [
            f"{owner} key {phase} #2."
            for owner in ("Ruby", "Sapphire") for phase in ("press", "release")
        ])
        if held.done():
            held.result()
            raise RuntimeError("focus-loss release was not observed before native key-up")
        if released["screen"]["class"] != SCREEN_CLASS:
            raise RuntimeError("the key action did not open its owner modal")
        modal_capture = capture_ui(client, artifact_dir, "key-focus-release")
        held.result(timeout=30.0)

    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.g"], "ticks": 2})
    modal_status = input_status(
        client, "loader_ruby", "Ruby input status: key=2/2 jump=1/1."
    )
    input_status(client, "loader_sapphire", "Sapphire input status: key=2/2.")
    request_hud(client, "loader_ruby", "Ruby", "hud", 30.0)

    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.e"], "ticks": 1})
    inventory = runtime.current_client_state(client, 30.0)
    if inventory["screen"]["class"] != "net.minecraft.client.gui.screens.inventory.InventoryScreen":
        raise RuntimeError("ordinary inventory key was intercepted")
    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.g"], "ticks": 2})
    inventory_status = input_status(
        client, "loader_ruby", "Ruby input status: key=2/2 jump=1/1."
    )
    input_status(client, "loader_sapphire", "Sapphire input status: key=2/2.")
    inventory_capture = capture_ui(client, artifact_dir, "inventory-input-isolation")
    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.escape"], "ticks": 1})
    if runtime.current_client_state(client, 30.0)["screen"]["open"]:
        raise RuntimeError("ordinary Escape key did not close inventory")
    edge_suppression = edge_status(client, "escape=0/0 f2=0/0 f11=0/0")
    return {
        "shared_key": shared,
        "shared_capture": shared_capture,
        "release_before_native_keyup": released,
        "modal_capture": modal_capture,
        "modal_suppression": modal_status,
        "inventory_suppression": inventory_status,
        "inventory_capture": inventory_capture,
        "declared_escape_suppression": edge_suppression,
    }


def edge_status(client: Any, expected: str) -> dict[str, Any]:
    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_send_chat", {"message": "loader_ruby edge_status", "command": True})
    return wait_chat_after(client, before, [f"Ruby edge status: {expected}."])


def exercise_input_edges(client: Any, artifact_dir: Path) -> dict[str, Any]:
    # The inventory-closing Escape was already checked; also close a Loader modal.
    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_send_chat", {"message": "loader_ruby", "command": True})
    observed = runtime.current_client_state(client, 30.0)
    deadline = time.monotonic() + 30.0
    while observed["screen"]["class"] != SCREEN_CLASS:
        observed = runtime.next_client_state(client, observed, deadline)
    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.escape"], "ticks": 1})
    if runtime.current_client_state(client, 30.0)["screen"]["open"]:
        raise RuntimeError("Escape did not close the Loader modal")
    modal_escape = edge_status(client, "escape=0/0 f2=0/0 f11=0/0")

    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.f2"], "ticks": 2})
    screenshot_actions = wait_chat_after(client, before, ["Ruby f2 press #1.", "Ruby f2 release #1."])
    observed = screenshot_actions
    deadline = time.monotonic() + 30.0
    while not any("Saved screenshot" in line for line in observed.get("recent_chat", [])):
        observed = runtime.next_client_state(client, observed, deadline)
    native_screenshots = sorted((artifact_dir / "game" / "screenshots").glob("*.png"))
    if not native_screenshots:
        raise RuntimeError("F2 did not write its vanilla screenshot")

    fullscreen_captures = []
    for count in (1, 2):
        before = runtime.current_client_state(client, 30.0)
        client.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.f11"], "ticks": 2})
        wait_chat_after(client, before, [f"Ruby f11 {phase} #{count}." for phase in ("press", "release")])
        options = (artifact_dir / "game" / "options.txt").read_text().splitlines()
        if f"fullscreen:{str(count == 1).lower()}" not in options:
            raise RuntimeError("F11 did not preserve the vanilla fullscreen toggle")
        fullscreen_captures.append(capture_ui(client, artifact_dir, f"fullscreen-{count}"))
    special_keys = edge_status(client, "escape=0/0 f2=1/1 f11=2/2")

    # A mixed invalid batch must neither start movement nor trigger a valid named action.
    invalid_batches = []
    for tool_name in ("minecraft_press_inputs", "minecraft_respawn"):
        before = runtime.current_client_state(client, 30.0)
        try:
            client.call_tool(tool_name, {"keys": ["forward", "key.keyboard.g", "key.keyboard.bogus"], "ticks": 2})
        except RuntimeError as error:
            invalid_batches.append({"tool": tool_name, "error": str(error)})
        else:
            raise RuntimeError(f"{tool_name} accepted an unknown named key")
        client.call_tool("minecraft_wait_ticks", {"ticks": 8})
        after = runtime.current_client_state(client, 30.0)
        if any(abs(after["player"][axis] - before["player"][axis]) > 0.01 for axis in ("x", "z")):
            raise RuntimeError(f"{tool_name} mutated movement before rejecting the invalid batch")
        input_status(client, "loader_ruby", "Ruby input status: key=2/2 jump=1/1.")
        input_status(client, "loader_sapphire", "Sapphire input status: key=2/2.")
    return {
        "modal_escape_suppression": modal_escape,
        "screenshot_actions": screenshot_actions,
        "vanilla_screenshots": [str(path.relative_to(artifact_dir)) for path in native_screenshots],
        "fullscreen_captures": fullscreen_captures,
        "special_key_status": special_keys,
        "invalid_batches": invalid_batches,
    }


def capture_ui(client: Any, artifact_dir: Path, name: str) -> dict[str, Any]:
    observed = runtime.current_client_state(client, 30.0)
    deadline = time.monotonic() + 30.0
    while observed["overlay"]["open"]:
        observed = runtime.next_client_state(client, observed, deadline)
    client.call_tool("minecraft_wait_ticks", {"ticks": 1})
    path = artifact_dir / "screenshots" / f"{name}.png"
    return client.call_tool("minecraft_screenshot", {"path": str(path)})


def request_hud(
    client: Any, command: str, owner: str, action: str, timeout_seconds: float
) -> dict[str, Any]:
    observed = runtime.current_client_state(client, timeout_seconds)
    acknowledgement = f"{owner} UI request: {action}."
    previous_count = observed.get("recent_chat", []).count(acknowledgement)
    client.call_tool("minecraft_send_chat", {"message": f"{command} {action}", "command": True})
    deadline = time.monotonic() + timeout_seconds
    while observed.get("recent_chat", []).count(acknowledgement) <= previous_count:
        observed = runtime.next_client_state(client, observed, deadline)
    if observed.get("screen", {}).get("open"):
        raise RuntimeError(f"HUD action {command} {action} left a modal screen open: {observed['screen']}")
    return observed


def request_sound(client: Any, owner: str, action: str) -> dict[str, Any]:
    before = runtime.current_client_state(client, 30.0)
    client.call_tool("minecraft_send_chat", {"message": f"loader_{owner} {action}", "command": True})
    label = "world" if action.startswith("sound_world ") else action
    return wait_chat_after(client, before, [f"{owner.capitalize()} sound: {label}."])


def run(platform: str, timeout_seconds: float, artifact_dir: Path) -> dict[str, Any]:
    if platform not in PLATFORMS:
        raise RuntimeError(f"unsupported platform {platform!r}")
    if runtime.port_open(25567):
        raise RuntimeError("loader live-gate server port 25567 is already in use")

    game_dir = artifact_dir / "game"
    game_dir.mkdir(parents=True, exist_ok=True)
    if platform == "forge":
        # Avoid FML's failing early-window GL-context handoff on this Xvfb host.
        config_dir = game_dir / "config"
        config_dir.mkdir(exist_ok=True)
        (config_dir / "fml.toml").write_text("earlyWindowControl = false\n")
    # Keep the gate focused on Loader compatibility instead of Minecraft's first-run
    # accessibility onboarding. Vanilla's 26.1.2 options datafix uses false for an
    # already-onboarded profile. Isolate Loader audio from vanilla background music.
    (game_dir / "options.txt").write_text(
        "version:4790\nonboardAccessibility:false\nsoundDevice:SolarisLoaderGate\n"
        "soundCategory_music:0.0\n"
    )
    (artifact_dir / "screenshots").mkdir(parents=True, exist_ok=True)
    world_dir = artifact_dir / "world"
    world_dir.mkdir(exist_ok=True)
    run_config = artifact_dir / "playable.toml"
    config_text = (REPO_ROOT / "examples" / "loader-live-gate" / "playable.toml").read_text()
    config_text = config_text.replace(
        'world_dir = ".analysis/loader-live-gate/world"',
        f'world_dir = "{world_dir.relative_to(REPO_ROOT).as_posix()}"',
    )
    run_config.write_text(config_text)
    token = f"solaris-loader-{platform}-{time.time_ns()}"
    mcp_port = runtime.reserve_port()
    username = {"fabric": "GateFabric", "neoforge": "GateNeoForge", "forge": "GateForge"}[platform]

    xvfb = server = client_process = None
    xvfb_log = server_log_handle = client_log_handle = None
    mcp = None
    audio = None
    result: dict[str, Any] = {
        "platform": platform,
        "artifact_dir": str(artifact_dir),
        "game_dir": str(game_dir.relative_to(REPO_ROOT)),
        "world_dir": str(world_dir.relative_to(REPO_ROOT)),
        "run_config": str(run_config.relative_to(REPO_ROOT)),
        "server_address": SERVER_ADDRESS,
        "mcp_port": mcp_port,
        "forge_early_window_control": False if platform == "forge" else None,
    }
    try:
        xvfb, display, xvfb_log = runtime.start_xvfb(artifact_dir)
        audio = SoundCapture(artifact_dir)

        server_log_path = artifact_dir / "server.log"
        server_log_handle = server_log_path.open("wb")
        server = subprocess.Popen(
            [str(REPO_ROOT / "target" / "debug" / "mc-server"), "--config", str(run_config)],
            cwd=REPO_ROOT,
            stdout=server_log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        runtime.wait_port(25567, min(timeout_seconds, 90.0), server, log_path=server_log_path)
        result["server_ready"] = True

        client_process, client_log_handle = runtime.start_client(
            game_dir=game_dir,
            log_path=artifact_dir / "client.log",
            token=token,
            mcp_port=mcp_port,
            username=username,
            display=display,
            platform=platform,
        )
        result["mcp_ready"] = True

        mcp = McpClient(
            f"http://127.0.0.1:{mcp_port}/mcp",
            token,
            request_timeout_seconds=max(30.0, min(timeout_seconds, 240.0) + 10.0),
        )
        initialized = mcp.initialize()
        tools = mcp.list_tools()
        result["protocol_version"] = initialized["protocolVersion"]
        result["tool_count"] = len(tools)

        before = runtime.wait_client_ready_for_connect(mcp, min(timeout_seconds, 120.0))
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
        for command, title, button, expected_paper_count in OWNER_SCREENS:
            mcp.call_tool("minecraft_send_chat", {"message": command, "command": True})
            click = runtime.retry_tool(
                mcp,
                "minecraft_click_screen_button",
                {
                    "expected_screen_class": SCREEN_CLASS,
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
            screen_capture = capture_ui(mcp, artifact_dir, f"{command}-screen")
            hud = request_hud(mcp, command, title.split(" Loader Fixture")[0], "hud", 30.0)
            hud_capture = capture_ui(mcp, artifact_dir, f"{command}-hud")
            owner_results.append(
                {
                    "command": command,
                    "screen_title": title,
                    "button": button,
                    "button_clicked": True,
                    "inventory": inventory,
                    "click": click,
                    "screen_capture": screen_capture,
                    "hud_observation": hud,
                    "hud_capture": hud_capture,
                }
            )
        result["owners"] = owner_results
        result["hud_update"] = request_hud(mcp, "loader_ruby", "Ruby", "update", 30.0)
        result["hud_update_capture"] = capture_ui(mcp, artifact_dir, "ruby-updated")
        result["hud_hide"] = request_hud(mcp, "loader_ruby", "Ruby", "hide", 30.0)
        result["hud_hide_capture"] = capture_ui(mcp, artifact_dir, "ruby-hidden-sapphire-visible")

        observed = runtime.current_client_state(mcp, 30.0)
        grounded_deadline = time.monotonic() + 30.0
        while not observed.get("player", {}).get("on_ground"):
            observed = runtime.next_client_state(mcp, observed, grounded_deadline)
        before_jump = observed["player"]
        mcp.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.space"], "ticks": 2})
        after_jump = mcp.call_tool("minecraft_observe")["player"]
        result["hud_input"] = {"before": before_jump, "after": after_jump}
        if after_jump["y"] - before_jump["y"] <= 0.1:
            raise RuntimeError("ordinary jump input did not move the player while HUD was visible")
        result["jump_actions"] = wait_chat_after(
            mcp, observed, ["Ruby jump press #1.", "Ruby jump release #1."]
        )
        result["key_actions"] = exercise_key_actions(mcp, artifact_dir)
        result["input_edges"] = exercise_input_edges(mcp, artifact_dir)
        result["sounds"] = exercise_sounds(
            audio, lambda owner, action: request_sound(mcp, owner, action),
            runtime.current_client_state(mcp, 30.0)["player"],
        )
        result["sound_capture"] = capture_ui(mcp, artifact_dir, "sound-commands")

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

        request_sound(mcp, "ruby", "sound")
        audio.sample(0.25)
        result["sound_before_disconnect"] = audio.sample()
        sound_baseline = result["sounds"]["personal"]["amplitude"]["440"]
        if result["sound_before_disconnect"]["amplitude"]["440"] < sound_baseline * 0.5:
            raise RuntimeError("disconnect sound fixture was not playing")
        mcp.call_tool("minecraft_disconnect")
        disconnected = runtime.current_client_state(mcp, 30.0)
        disconnect_deadline = time.monotonic() + 30.0
        while disconnected.get("in_play"):
            disconnected = runtime.next_client_state(mcp, disconnected, disconnect_deadline)
        result["disconnected"] = disconnected
        audio.sample(0.25)
        result["sound_after_disconnect"] = audio.sample()
        if result["sound_after_disconnect"]["amplitude"]["440"] > sound_baseline * 0.03:
            raise RuntimeError("old connection sound remained audible after disconnect")
        mcp.call_tool("minecraft_connect", {"server_addr": SERVER_ADDRESS})
        rejoined = mcp.call_tool("minecraft_wait_for_play", {"timeout_seconds": 60.0})
        if not rejoined.get("in_play"):
            raise RuntimeError(f"client did not rejoin after HUD presentation: {rejoined}")
        result["rejoined"] = rejoined
        result["rejoin_capture"] = capture_ui(mcp, artifact_dir, "rejoined-no-hud")
        result["rejoin_input_reset"] = input_status(
            mcp, "loader_ruby", "Ruby input status: key=0/0 jump=0/0."
        )
        input_status(mcp, "loader_sapphire", "Sapphire input status: key=0/0.")
        before_rejoin_key = runtime.current_client_state(mcp, 30.0)
        mcp.call_tool("minecraft_press_inputs", {"keys": ["key.keyboard.g"], "ticks": 2})
        result["rejoin_key_actions"] = wait_chat_after(mcp, before_rejoin_key, [
            f"{owner} key {phase} #1."
            for owner in ("Ruby", "Sapphire") for phase in ("press", "release")
        ])
        result["rejoin_input_capture"] = capture_ui(mcp, artifact_dir, "rejoined-key-actions")
        result["sound_after_reconnect"] = audio.sample()
        if result["sound_after_reconnect"]["amplitude"]["440"] > sound_baseline * 0.03:
            raise RuntimeError("old sound restarted on reconnect")
        request_sound(mcp, "ruby", "sound")
        audio.sample(0.25)
        result["sound_rebound"] = audio.sample()
        if result["sound_rebound"]["amplitude"]["440"] < sound_baseline * 0.5:
            raise RuntimeError("new session sound did not play")
        request_sound(mcp, "ruby", "sound_stop")
        result["ui_visual_review"] = "required: inspect captured HUD text, update, owner isolation, and reconnect cleanup"
        mcp.call_tool("minecraft_disconnect")
        result["passed"] = True
        (artifact_dir / "scenario.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        return result
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        if audio is not None:
            audio.close()
        for handle in (client_log_handle, server_log_handle, xvfb_log):
            if handle is not None:
                handle.close()
        if not result.get("passed"):
            (artifact_dir / "scenario.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
