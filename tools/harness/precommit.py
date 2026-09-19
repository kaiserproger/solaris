"""Real-client P5 hook acceptance and request-to-visible-state latency.

Runs inside core-client's owned Xvfb lifecycle. Native owner regressions cover
programmatic mutations, stale tickets and armor transactions; this scenario
exercises ordinary client placement, ordered damage decisions and persistent
fail-closed protection with the real source-built component.
"""

from __future__ import annotations

import json
import shutil
import statistics
import subprocess
import time
import tomllib
from pathlib import Path
from typing import Any

from . import runtime
from .mcp import McpClient

PREFIX = "P5_PRECOMMIT_WITNESS"
PLUGINS = (("hook-a", "hooka", 10), ("hook-b", "hookb", 0))
PLAYER = "PrecommitClient"
STONE = "minecraft:stone"
PICKAXE = "minecraft:iron_pickaxe"


def _command(client: McpClient, text: str) -> None:
    client.call_tool("minecraft_send_chat", {"message": text, "command": True})


def _wait_chat(client: McpClient, text: str) -> tuple[dict[str, Any], str]:
    deadline = time.monotonic() + 20.0
    observed = runtime.current_client_state(client, 20.0)
    while True:
        for line in reversed(observed.get("recent_chat", [])):
            if text in str(line):
                return observed, str(line)
        observed = runtime.next_client_state(client, observed, deadline)


def _fence(client: McpClient, plugin: str, root: str, token: str) -> dict[str, Any]:
    _command(client, f"{root} status {token}")
    observed, line = _wait_chat(client, f"fence={token}")
    if f"{PREFIX} plugin={plugin} end " not in line:
        raise RuntimeError(f"wrong hook witness for {token}: {line}")
    fields = dict(word.split("=", 1) for word in line.split() if "=" in word)
    return {"build": int(fields["build"]), "damage": int(fields["damage"]),
            "line": line, "observed": observed}


def _control(client: McpClient, plugin: str, root: str, action: str, token: str) -> None:
    _command(client, f"{root} {action}")
    # Same connection and host FIFO: this unique report follows the control.
    state = _fence(client, plugin, root, token)
    expected = f"{PREFIX} plugin={plugin} control {action}"
    if not any(expected in str(line) for line in state["observed"].get("recent_chat", [])):
        raise RuntimeError(f"hook control was not accepted: {action}: {state['line']}")


def _count(observed: dict[str, Any], item: str) -> int:
    return sum(int(stack["count"]) for stack in observed["inventory"]
               if stack.get("item_id") == item)


def _support(client: McpClient) -> dict[str, int]:
    observed = runtime.current_client_state(client, 30.0)
    deadline = time.monotonic() + 30.0
    while (observed.get("screen", {}).get("open")
           or observed.get("overlay", {}).get("open")
           or not observed.get("player", {}).get("on_ground")):
        observed = runtime.next_client_state(client, observed, deadline)
    player = observed["player"]
    x, y, z = (int(float(player[key]) // 1) for key in ("x", "y", "z"))
    scan = client.call_tool("minecraft_scan_blocks", {
        "min_x": x - 3, "max_x": x + 3,
        "min_y": y - 2, "max_y": y + 2,
        "min_z": z - 3, "max_z": z + 3, "max_blocks": 245,
    })
    blocks = {(int(block["x"]), int(block["y"]), int(block["z"])): block
              for block in scan["blocks"]}
    solid = {"minecraft:stone", "minecraft:dirt", "minecraft:grass_block",
             "minecraft:snow_block", "minecraft:gravel", "minecraft:andesite"}
    candidates = []
    for (bx, by, bz), block in blocks.items():
        distance = (bx + 0.5 - float(player["x"])) ** 2 + (bz + 0.5 - float(player["z"])) ** 2
        if (by == y - 1 and 1.0 < distance < 8.0 and block.get("block_id") in solid
                and blocks.get((bx, by + 1, bz), {}).get("is_air")
                and blocks.get((bx, by + 2, bz), {}).get("is_air")):
            candidates.append((distance, {"x": bx, "y": by, "z": bz}))
    if not candidates:
        raise RuntimeError(f"no clear reachable placement support around {(x, y, z)}: {scan}")
    return min(candidates, key=lambda candidate: candidate[0])[1]


def _select(client: McpClient, item: str) -> None:
    client.call_tool("minecraft_select_hotbar_item", {
        "item_id": item, "count": 1, "timeout_seconds": 10.0,
    })


def _place(client: McpClient, support: dict[str, int], expected_count: int) -> float:
    target = {**support, "y": support["y"] + 1}
    _select(client, STONE)
    start = time.monotonic_ns()
    client.call_tool("minecraft_use_item_on", {**support, "face": "up"})
    _fence(client, "hook-a", "hooka", f"placed-{expected_count}")
    client.call_tool("minecraft_wait_for_block_state", {
        **target, "block_id": STONE, "timeout_seconds": 10.0,
    })
    observed = runtime.current_client_state(client, 10.0)
    deadline = time.monotonic() + 10.0
    while _count(observed, STONE) != expected_count:
        observed = runtime.next_client_state(client, observed, deadline)
    return (time.monotonic_ns() - start) / 1_000_000


def _clear(client: McpClient, support: dict[str, int]) -> None:
    _select(client, PICKAXE)
    result = client.call_tool("minecraft_break_block", {
        **support, "y": support["y"] + 1, "face": "up",
        "expected_drop_item_id": "minecraft:cobblestone", "expected_drop_count": 1,
        "timeout_seconds": 20.0,
    })
    if not result.get("became_air") or not result.get("pickup_confirmed"):
        raise RuntimeError(f"accepted build could not be mined normally: {result}")


def _refused_place(client: McpClient, support: dict[str, int], count: int, token: str) -> dict[str, Any]:
    _select(client, STONE)
    start = time.monotonic_ns()
    client.call_tool("minecraft_use_item_on", {**support, "face": "up"})
    witness = _fence(client, "hook-a", "hooka", token)
    block = client.call_tool("minecraft_read_block", {**support, "y": support["y"] + 1})
    observed = runtime.current_client_state(client, 10.0)
    if not block.get("is_air") or _count(observed, STONE) != count:
        raise RuntimeError(f"refused placement changed world/inventory: {block}; {observed}")
    return {"client_fenced_ms": (time.monotonic_ns() - start) / 1_000_000,
            "stone_count": count, "block": block, "witness": witness["line"]}


def _health(client: McpClient, expected: float) -> dict[str, Any]:
    deadline = time.monotonic() + 10.0
    observed = runtime.current_client_state(client, 10.0)
    while abs(float(observed["player"]["health"]) - expected) > 0.001:
        observed = runtime.next_client_state(client, observed, deadline)
    return observed


def _damage(client: McpClient, amount: int, expected_health: float, token: str) -> dict[str, Any]:
    start = time.monotonic_ns()
    _command(client, f"debug survival damage {amount}")
    witness = _fence(client, "hook-a", "hooka", token)
    observed = _health(client, expected_health)
    return {"client_fenced_ms": (time.monotonic_ns() - start) / 1_000_000,
            "health": observed["player"]["health"], "witness": witness["line"]}


def _case(root: Path, display: str, timeout_seconds: float, *, hooks: bool) -> dict[str, Any]:
    run_dir = root / ("hooked" if hooks else "direct")
    run_dir.mkdir(parents=True)
    fixture = runtime.REPO_ROOT / ".analysis/loader-live-gate/plugins/ruby-live"
    api = tomllib.loads((fixture / "plugin.toml").read_text())["api"]
    for plugin, command, _order in PLUGINS:
        package = run_dir / "plugins" / plugin
        package.mkdir(parents=True)
        shutil.copyfile(fixture / "plugin.wasm", package / "plugin.wasm")
        (package / "plugin.toml").write_text(
            f'id = "{plugin}"\nname = "{plugin}"\nversion = "0.1.0"\napi = "{api}"\n'
            f'player_commands = ["{command}"]\nhooks = ["before-build", "before-damage"]\n'
        )
        (package / "config.toml").write_text(f'mode = "precommit"\nroot = "{command}"\n')
    server_port, mcp_port = runtime.reserve_port(), runtime.reserve_port()
    config = run_dir / "server.toml"
    registrations = ""
    if hooks:
        registrations = "".join(
            f'\n[[plugins.hooks]]\nplugin_id = "{plugin}"\nkind = "{kind}"\n'
            f'order = {order}\non_failure = "deny"\n'
            for plugin, _command_root, order in PLUGINS
            for kind in ("before-build", "before-damage")
        )
    config.write_text(f'''[server]
name = "wasm-precommit-gate"
motd = "WASM precommit real-client gate"
view_distance = 4
simulation_distance = 4
[network]
bind_address = "127.0.0.1"
port = {server_port}
[auth]
online_mode = false
[admin]
operators = ["{PLAYER}"]
allow_local_dev_operators = false
[plugins]
directory = "{(run_dir / 'plugins').relative_to(runtime.REPO_ROOT).as_posix()}"
runtime = "wasm"
strict = true
expected = ["hook-a", "hook-b"]
{registrations}
[data]
world_dir = "{(run_dir / 'world').relative_to(runtime.REPO_ROOT).as_posix()}"
seed = 0
worldgen_mode = "tellus_like"
[simulation]
random_tick_speed = 0
friendly_spawn_interval_ticks = 0
hostile_spawn_interval_ticks = 0
[autoscale]
enabled = false
''')
    server = client_process = client_log = client = None
    server_log_path = run_dir / "server.log"
    server_log = server_log_path.open("wb")
    result: dict[str, Any] = {"registered_hooks": hooks, "passed": False}
    try:
        server = subprocess.Popen(
            [str(runtime.REPO_ROOT / "target/debug/mc-server"), "--config", str(config), "--no-console"],
            cwd=runtime.REPO_ROOT, stdout=server_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        runtime.wait_port(server_port, min(timeout_seconds, 60), server, log_path=server_log_path)
        game = run_dir / "game"
        game.mkdir()
        (game / "options.txt").write_text("version:4790\nonboardAccessibility:false\n")
        token = f"precommit-{time.time_ns()}"
        client_process, client_log = runtime.start_client(
            game_dir=game, log_path=run_dir / "client.log", token=token,
            mcp_port=mcp_port, username=PLAYER, display=display, platform=None,
        )
        client = McpClient(f"http://127.0.0.1:{mcp_port}/mcp", token, request_timeout_seconds=30)
        client.initialize()
        runtime.wait_client_ready_for_connect(client, min(timeout_seconds, 120))
        client.call_tool("minecraft_connect", {"server_addr": f"127.0.0.1:{server_port}"})
        play = client.call_tool("minecraft_wait_for_play", {"timeout_seconds": min(timeout_seconds, 120)})
        if not play.get("in_play"):
            raise RuntimeError(f"precommit client never entered Play: {play}")
        _fence(client, "hook-a", "hooka", "joined")
        support = _support(client)
        result["support"] = support
        _command(client, "give minecraft:stone 32")
        _command(client, "give minecraft:iron_pickaxe 1")
        _fence(client, "hook-a", "hooka", "equipped")
        client.call_tool("minecraft_wait_for_inventory", {"item_id": STONE, "count": 32, "timeout_seconds": 10})
        client.call_tool("minecraft_wait_for_inventory", {"item_id": PICKAXE, "count": 1, "timeout_seconds": 10})
        samples = []
        for sample in range(5):
            samples.append(_place(client, support, 31 - sample))
            _clear(client, support)
        result["placement_latency_ms"] = {"samples": samples, "median": statistics.median(samples),
                                           "minimum": min(samples), "maximum": max(samples)}
        states = {plugin: _fence(client, plugin, command, f"build-{plugin}")
                  for plugin, command, _order in PLUGINS}
        result["build_witnesses"] = {plugin: state["line"] for plugin, state in states.items()}
        if hooks:
            if any(state["build"] < 10 for state in states.values()):
                raise RuntimeError(f"ordinary placement/mining bypassed hooks: {states}")
            _control(client, "hook-b", "hookb", "build cancel", "cancel-build")
            before = states["hook-a"]["build"]
            result["cancelled_placement"] = _refused_place(client, support, 27, "cancelled-place")
            after = _fence(client, "hook-a", "hooka", "terminal-cancel")
            if after["build"] != before:
                raise RuntimeError("later build handler ran after terminal Cancel")
            cancelled = _fence(client, "hook-b", "hookb", "cancel-handler-ran")
            if cancelled["build"] != states["hook-b"]["build"] + 1:
                raise RuntimeError("placement refusal did not reach the configured Cancel handler")
            _control(client, "hook-b", "hookb", "build keep", "restore-build")
        elif any(state["build"] or state["damage"] for state in states.values()):
            raise RuntimeError(f"unregistered hook was invoked: {states}")
        _command(client, "debug survival heal 20")
        _fence(client, "hook-a", "hooka", "healed")
        _health(client, 20)
        if hooks:
            # Operator order is deliberately the reverse of plugin-id order.
            _control(client, "hook-b", "hookb", "damage replace 2 0", "first-replace")
            _control(client, "hook-a", "hooka", "damage replace 0.5 1", "second-replace")
        result["damage"] = _damage(client, 4, 15 if hooks else 16, "damage-visible")
        if hooks:
            for plugin, command, amount in (("hook-b", "hookb", 4.0), ("hook-a", "hooka", 8.0)):
                witness = _fence(client, plugin, command, f"damage-{plugin}")
                expected = f"amount={amount!r} "
                if not any(f"{PREFIX} plugin={plugin} damage " in str(line) and expected in str(line)
                           for line in witness["observed"].get("recent_chat", [])):
                    raise RuntimeError(f"wrong ordered raw damage context for {plugin}: {witness['line']}")
            _command(client, "debug survival heal 20")
            _fence(client, "hook-a", "hooka", "healed-again")
            _health(client, 20)
            client.call_tool("minecraft_wait_ticks", {"ticks": 21})
            _control(client, "hook-b", "hookb", "damage cancel", "cancel-damage")
            before = _fence(client, "hook-a", "hooka", "before-damage-cancel")["damage"]
            result["cancelled_damage"] = _damage(client, 4, 20, "damage-cancelled")
            after = _fence(client, "hook-a", "hooka", "damage-terminal")["damage"]
            if before != after:
                raise RuntimeError("later damage handler ran after terminal Cancel")
            _control(client, "hook-b", "hookb", "fault build trap", "armed-trap")
            result["trap_protection"] = [
                _refused_place(client, support, 27, "trap-first"),
                _refused_place(client, support, 27, "trap-retired"),
            ]
        else:
            result["zero_subscriber_witnesses"] = {}
            for plugin, command, _order in PLUGINS:
                witness = _fence(client, plugin, command, f"direct-final-{plugin}")
                if witness["build"] or witness["damage"]:
                    raise RuntimeError(f"zero-subscriber path called the guest: {witness['line']}")
                result["zero_subscriber_witnesses"][plugin] = witness["line"]
        screenshots = run_dir / "screenshots"
        screenshots.mkdir()
        result["screenshot"] = client.call_tool("minecraft_screenshot", {"path": str(screenshots / "precommit.png")})
        client.call_tool("minecraft_disconnect")
        result["passed"] = True
        return result
    finally:
        if client is not None:
            client.close()
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        if client_log is not None:
            client_log.close()
        server_log.close()
        (run_dir / "scenario.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")


def run(root: Path, display: str, timeout_seconds: float) -> dict[str, Any]:
    root = root / "wasm-precommit"
    direct = _case(root, display, timeout_seconds, hooks=False)
    hooked = _case(root, display, timeout_seconds, hooks=True)
    return {"passed": True, "direct": direct, "hooked": hooked,
            "measurement": "Client request through a unique post-action command fence to observed block/inventory or health, excluding client prediction alone. MCP, status-report and fence overhead included; five placement samples per case, not a throughput benchmark."}
