"""Script a windowed native or vanilla-Java client scene on an isolated v2 world."""
from __future__ import annotations

import asyncio
import ctypes
import ctypes.util
import json
import math
import os
import re
import secrets
import shutil
import subprocess
import threading
from pathlib import Path
from typing import Any

from .. import runtime
from ..mcp import McpClient
from ..profiles import ProfileContext
from . import java_p5, native_client
from .gameplay import prepare_operator_fixture
from .probe_steps import ProbeSteps

# Both clients render the same fixed window; the vanilla client is only a
# rendering oracle, so its camera/GUI options are pinned to the native defaults.
WINDOW = (1280, 720)
NATIVE_FOV = 70
NATIVE_GUI_SCALE = 3

USERNAME = "SceneShot"

def scene_username(scenario: dict[str, Any]) -> str:
    name = scenario.get("username", USERNAME)
    if not isinstance(name, str) or re.fullmatch(r"[A-Za-z0-9_]{1,16}", name) is None:
        raise ValueError("username must be 1..16 ASCII letters, digits or underscores")
    return name


class LatencyProxy:
    """Delay traffic in each direction without changing host networking."""

    def __init__(self, target_port: int, delay_ms: int):
        self.target_port = target_port
        self.delay = delay_ms / 1000
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        self.error = None
        self.port = None
        self.thread = threading.Thread(target=self._run, daemon=True)

    async def _forward(self, reader, writer):
        while data := await reader.read(65536):
            ready = self.loop.create_future()
            timer = self.loop.call_later(self.delay, ready.set_result, None)
            try:
                await ready
            finally:
                timer.cancel()
            writer.write(data)
            await writer.drain()

    async def _connection(self, reader, writer):
        upstream = None
        try:
            upstream_reader, upstream = await asyncio.open_connection("127.0.0.1", self.target_port)
            tasks = [
                asyncio.create_task(self._forward(reader, upstream)),
                asyncio.create_task(self._forward(upstream_reader, writer)),
            ]
            _, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in pending:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        finally:
            writer.close()
            if upstream is not None:
                upstream.close()

    def _run(self):
        asyncio.set_event_loop(self.loop)
        try:
            server = self.loop.run_until_complete(asyncio.start_server(self._connection, "127.0.0.1", 0))
            self.port = server.sockets[0].getsockname()[1]
        except Exception as error:
            self.error = error
            self.ready.set()
            self.loop.close()
            return
        self.ready.set()
        self.loop.run_forever()
        server.close()
        self.loop.run_until_complete(server.wait_closed())
        tasks = asyncio.all_tasks(self.loop)
        for task in tasks:
            task.cancel()
        self.loop.run_until_complete(asyncio.gather(*tasks, return_exceptions=True))
        self.loop.close()

    def start(self):
        self.thread.start()
        self.ready.wait()
        if self.error is not None:
            raise RuntimeError("starting latency proxy failed") from self.error
        return self.port

    def close(self):
        if self.loop.is_running():
            self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join()


def java_options(render_distance: int, fov: int) -> str:
    # Java stores fov normalized: 70 -> 0.0 and 110 -> 1.0.
    return (
        "version:4790\nonboardAccessibility:false\n"
        'graphicsPreset:"custom"\n'
        f"overrideWidth:{WINDOW[0]}\noverrideHeight:{WINDOW[1]}\n"
        f"fov:{(fov - 70) / 40}\nguiScale:{NATIVE_GUI_SCALE}\nrenderDistance:{render_distance}\n"
        f"simulationDistance:{min(render_distance, 12)}\nmaxFps:60\nenableVsync:false\n"
        "pauseOnLostFocus:false\ntutorialStep:none\nsoundCategory_master:0.0\n"
    )


def check_java_render_distance(options_path: Path, requested: int) -> None:
    values = [line.partition(":")[2] for line in options_path.read_text().splitlines()
              if line.startswith("renderDistance:")]
    if values != [str(requested)]:
        raise RuntimeError(f"Java renderDistance is {values!r}, expected {requested} in {options_path}")


def find_java_window(display: str) -> int | None:
    """The harness Xvfb hosts only this client: its largest top-level window is the game."""
    tree = subprocess.run(["xwininfo", "-display", display, "-root", "-children"], check=True,
                          capture_output=True, text=True).stdout
    windows = [(int(w) * int(h), int(ident, 16))
               for ident, w, h in re.findall(r"^\s+(0x[0-9a-f]+) .*?\s(\d+)x(\d+)\+-?\d+\+-?\d+", tree, re.M)]
    return max(windows)[1] if windows else None


def resize_java_window(owned: dict[str, Any]) -> None:
    """OpenGL Java clients (26.1.x Loader) launch at 854x480 despite options.txt; match the native window."""
    x = ctypes.CDLL(ctypes.util.find_library("X11"))
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]
    x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x.XCloseDisplay.argtypes = [ctypes.c_void_p]
    connection = x.XOpenDisplay(owned["display"].encode())
    if not connection:
        raise RuntimeError("cannot open the Java scenario's private Xvfb")
    try:
        x.XResizeWindow(connection, owned["window_id"], *WINDOW)
        x.XSync(connection, 0)
    finally:
        x.XCloseDisplay(connection)


def png_size(path: str) -> tuple[int, int]:
    with open(path, "rb") as image:
        header = image.read(24)
    return int.from_bytes(header[16:20], "big"), int.from_bytes(header[20:24], "big")


def capture_java(probe: ProbeSteps, artifact: Path, name: str) -> str:
    path = java_p5.capture(probe, name)
    if png_size(path) != WINDOW:
        raise RuntimeError(f"Java screenshot {path} is {png_size(path)}, expected {WINDOW}")
    return path


def capture_native(probe: ProbeSteps, artifact: Path, name: str) -> str:
    return native_client.capture(probe.client, artifact, name)["retained_path"]


def wait_play(probe: ProbeSteps) -> None:
    # Each MCP wait is capped at 120s; keep the same overall scenario deadline.
    while True:
        result = probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
        if result.get("in_play") is True:
            return
        if result.get("disconnect_reason"):
            raise RuntimeError(f"client disconnected before play: {result['disconnect_reason']}")


def execute(probe: ProbeSteps, artifact: Path, steps: list[dict[str, Any]], client: str = "native") -> list[str]:
    capture = capture_java if client == "java" else capture_native
    shots = []
    for step in steps:
        if isinstance(step, dict) and set(step) == {"shot", "hold_keys"}:
            if client == "java":
                raise ValueError("hold_keys screenshots need the native tab-list state; unsupported for --client java")
            name, keys = step["shot"], step["hold_keys"]
            if (not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", name)
                    or not isinstance(keys, list) or not keys or any(not isinstance(k, str) for k in keys)):
                raise ValueError(f"invalid held screenshot: {step!r}")
            args = {"keys": keys, "ticks": 1, "hold": True}
            probe.call("minecraft_press_inputs", args)
            try:
                if "key.keyboard.tab" in keys:
                    probe.wait(lambda state: any(str(node.get("id", "")).startswith("tab/player/")
                                                  for node in state.get("ui_nodes", [])),
                               "tab_list_visible")
                shots.append(capture(probe, artifact, name))
            finally:
                probe.call("minecraft_press_inputs", {**args, "hold": False})
            continue
        if not isinstance(step, dict) or len(step) != 1:
            raise ValueError(f"invalid scene step: {step!r}")
        kind, value = next(iter(step.items()))
        if kind in ("shot", "observe"):
            if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", value):
                raise ValueError(f"invalid {kind} name: {value!r}")
            if kind == "shot":
                shots.append(capture(probe, artifact, value))
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
        elif kind == "wait_screen" and isinstance(value, str) and value:
            # Server-driven screens (dialogs, menus) open asynchronously; wait for
            # the client's own screen state whose class or title names `value`.
            probe.wait(lambda state: bool((state.get("screen") or {}).get("open"))
                       and (value in str(state["screen"].get("class", ""))
                            or value in str(state["screen"].get("title", ""))),
                       "screen_open")
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


def start_java(ctx: ProfileContext, artifact: Path, address: str, username: str, timeout: float,
               display: str, render_distance: int, fov: int) -> tuple[Any, Any, McpClient]:
    """Launch the vanilla Java client (loader MCP) and connect it to `address`."""
    game_dir = artifact / "game"
    game_dir.mkdir()
    (game_dir / "options.txt").write_text(java_options(render_distance, fov))
    token, mcp_port = secrets.token_urlsafe(32), runtime.reserve_port()
    log_path = artifact / "client.log"
    ctx.client_evidence["command_index"] = len(ctx.commands)
    ctx.commands.append([str(runtime.AGENT_ROOT / "gradlew"), ":fabric-agent:runClientMcp"])
    ctx.cwds.append(str(runtime.REPO_ROOT))
    ctx.exits.append(None)
    ctx.logs.append(str(log_path))
    process, log = runtime.start_client(game_dir=game_dir, log_path=log_path, token=token,
                                        mcp_port=mcp_port, username=username, display=display)
    mcp = None
    try:
        mcp = McpClient(f"http://127.0.0.1:{mcp_port}/mcp", token, timeout + 10)
        mcp.initialize()
        runtime.wait_client_ready_for_connect(mcp, timeout)
        if "video driver: wayland" in log_path.read_text(errors="replace").lower():
            raise RuntimeError("Java client opened its window on Wayland (the owner's desktop), not the harness Xvfb")
        mcp.call_tool("minecraft_connect", {"server_addr": address})
    except BaseException:
        if mcp is not None:
            mcp.close()
        runtime.stop_process(process)
        log.close()
        raise
    return process, log, mcp


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client not in ("native", "java") or not ctx.windowed or ctx.platform is not None:
        raise ValueError("scene-shot requires --client native|java --windowed")
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
    spawn_protection = scenario.get("spawn_protection", 0)
    if type(spawn_protection) is not int or spawn_protection < 0:
        raise ValueError("spawn_protection must be a non-negative integer")
    operator = scenario.get("operator", True)
    if type(operator) is not bool:
        raise ValueError("operator must be true or false")
    username = scene_username(scenario)
    client_latency_ms = scenario.get("client_latency_ms", 0)
    if type(client_latency_ms) is not int or not 0 <= client_latency_ms <= 5000:
        raise ValueError("client_latency_ms must be an integer in 0..5000")
    settings = scenario.get("client_settings", {})
    if not isinstance(settings, dict) or set(settings) - {"graphics_preset", "render_distance", "fov"}:
        raise ValueError("unsupported client settings")
    if "fov" in settings and (type(settings["fov"]) is not int or not 30 <= settings["fov"] <= 110):
        raise ValueError("fov must be an integer in 30..110")
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
    replacements = (("[server]", f"[server]\nspawn_protection = {spawn_protection}"),
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
    if operator:
        prepare_operator_fixture(ctx, [username])
    xdg = artifact / "xdg"
    settings_path = xdg / "solaris-client" / "client-settings.json"
    source_root = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "solaris-client"
    settings_path.parent.mkdir(parents=True)
    if source_root.is_dir():
        # Downloaded content is read-only and ~650 MB: link it. Copy the rest
        # (settings, caches the client writes) so runs never share state.
        for entry in source_root.iterdir():
            target = settings_path.parent / entry.name
            if entry.name in {"facts", "assets", "resources", "mojang"}:
                target.symlink_to(entry.resolve(), target_is_directory=entry.is_dir())
            elif entry.is_dir():
                shutil.copytree(entry, target, ignore_dangling_symlinks=True)
            elif entry.exists():
                shutil.copy2(entry, target)
    original = os.environ.get("SOLARIS_CLIENT_SETTINGS")
    if original and Path(original).is_file():
        base = json.loads(Path(original).read_text())
    else:
        base_path = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "solaris-client" / "client-settings.json"
        base = json.loads(base_path.read_text()) if base_path.is_file() else {}
    # The owner's personal fov must not leak into a reference comparison.
    base.update({"fov": NATIVE_FOV, **settings})
    if ctx.client == "java" and "graphics_preset" in settings:
        raise ValueError("graphics_preset is a native-client setting; the Java client keeps its default")
    settings_path.write_text(json.dumps(base, indent=2) + "\n")
    server = client = xvfb = mcp = log = client_log = proxy = None
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
        client_port = port
        if client_latency_ms:
            proxy = LatencyProxy(port, client_latency_ms)
            client_port = proxy.start()
        xvfb, display, _ = runtime.start_xvfb(artifact)
        ctx.logs.append(str(artifact / "xvfb.log"))
        old_xdg = os.environ.get("XDG_DATA_HOME")
        old_settings = os.environ.get("SOLARIS_CLIENT_SETTINGS")
        try:
            os.environ["XDG_DATA_HOME"] = str(xdg)
            os.environ["SOLARIS_CLIENT_SETTINGS"] = str(settings_path)
            if ctx.client == "java":
                client, client_log, mcp = start_java(
                    ctx, artifact, f"127.0.0.1:{client_port}", username, timeout, display,
                    base.get("render_distance", 12), base["fov"])
            else:
                client, client_log, mcp = native_client.start(ctx, native_client.resolve_binary(ctx), f"127.0.0.1:{client_port}", username, timeout, display=display)
        finally:
            for key, old in (("XDG_DATA_HOME", old_xdg), ("SOLARIS_CLIENT_SETTINGS", old_settings)):
                if old is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = old
        mcp.request_timeout_seconds = min(timeout + 10, 130.0)
        with ProbeSteps(mcp, artifact / "scene-shot", timeout, "scene-shot") as probe:
            wait_play(probe)
            if ctx.client == "java":
                window_id = find_java_window(display)
                if window_id is None:
                    raise RuntimeError(f"Java client opened no window on the harness Xvfb {display}; refusing to run")
                ctx.client_evidence["window_focus"] = mcp.p4_window = {"display": display, "window_id": window_id}
                resize_java_window(mcp.p4_window)
                probe.call("minecraft_wait_ticks", {"ticks": 10})
            else:
                from . import native_focus
                ctx.client_evidence["window_focus"] = native_focus.acquire(display, client.pid, probe.remaining())
            before = probe.observe()
            probe.call("minecraft_wait_for_state_change", {
                "observed_version": before["state_version"], "timeout_seconds": probe.remaining(),
            })
            shots = execute(probe, artifact, scenario["steps"], ctx.client)
        result = {"screenshots": shots, "scenario": probe.report, "seed": seed, "settings": settings, "client_latency_ms": client_latency_ms}
    finally:
        if mcp is not None:
            mcp.close()
        runtime.stop_process(client)
        if client is not None:
            ctx.exits[ctx.client_evidence["command_index"]] = client.returncode
        if client_log is not None:
            client_log.close()
        if proxy is not None:
            proxy.close()
        runtime.stop_process(server, interrupt=True)
        if server is not None:
            ctx.exits[server_index] = server.returncode
        runtime.stop_process(xvfb)
        if log is not None:
            log.close()
    if ctx.client == "java":
        check_java_render_distance(artifact / "game" / "options.txt", base.get("render_distance", 12))
    return result
