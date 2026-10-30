"""Autonomous generated-village QA: ownerless growth, no-give building, claim fencing.

Seed 100 has a 160-piece generated village whose bounds (≈35..206 x, -74..89 z)
sit within walking distance of the safe spawn (128,69,0). The player walks to
the village through the ordinary movement path (no teleport for the approach),
then the scenario proves:

1. The village becomes an ownerless settlement (settlement info: owner=autonomous).
2. The founding chest exists and starts empty (client-opened container slots).
3. Generated buildings certify (building list shows entries) + natural
   mine/lumber zones appear.
4. Workers produce: storage totals grow over real ticks with no /give.
5. A Builder completes Storage L1, a further autonomously chosen building,
   and an autonomous upgrade with NO player-provided items; blocks are client-visible.
6. Player mutations on the unowned settlement are refused.
7. Claim works via the dialog Claim button; a second player's
   /settlement claim is refused ("already has an owner").
8. After claim the owner can order a building (Projects -> Order, or
   /settlement build).
9. One bounded adversarial pass (break a block mid-build / restart resume).

Strictness: PASS only with direct client-observed evidence (MCP block reads,
entity lists, container slots, chat, screenshots). INCONCLUSIVE counts as FAIL.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import subprocess
import time
from pathlib import Path
from typing import Any, Callable

from .. import runtime
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib, prepare_operator_fixture
from .probe_steps import ProbeSteps

QA_SEED = int(os.environ.get("VILLAGE_AUTONOMY_SEED", "100") or 100)
QA_VILLAGE = os.environ.get("VILLAGE_AUTONOMY_CENTER", "")

def _parse_center(text: str, fallback: tuple[int, int, int]) -> tuple[int, int, int]:
    try:
        parts = [int(p) for p in text.split(",")]
        if len(parts) == 3:
            return (parts[0], parts[1], parts[2])
    except ValueError:
        pass
    return fallback
USERNAME = "VillageAuto"
SECOND_USERNAME = "VillageAutoB"
SEED = QA_SEED
# Owner-order candidates, preferred first. Storage needs ~25 logs of stock; Wall
# is cobblestone-only, which the founding chest keeps after autonomous growth.
OWNER_ORDER_BLUEPRINTS = ("storage", "wall")
# Survey receipt: seed 100 safe spawn (128,69,0); village site start chunk (7,0),
# bounds x 35..206, y 50..118, z -74..89, 160 pieces. Recorded 2026-10-20 by a
# temporary in-tree survey test (since removed).
# Seed/center are overridable via VILLAGE_AUTONOMY_SEED / VILLAGE_AUTONOMY_CENTER
# ("x,y,z"); the assignment profile pins seed 87122, center (-461,~70,350).
SPAWN = (128, 69, 0)
VILLAGE_CENTER = _parse_center(QA_VILLAGE, (120, 70, 7))
DIALOG_CLASS = "net.minecraft.client.gui.screens.dialog.MultiButtonDialogScreen"

# Storage L1 base needs (authored blueprint materials): these must arrive via
# worker production only -- the scenario never /gives them.
STORAGE_L1_NEEDS = {
    "minecraft:cobblestone": 50,
    "minecraft:oak_log": 41,
    "minecraft:coal": 1,
}

REQUIRED_BEHAVIOURS = (
    "walk_to_village",
    "ownerless_settlement",
    "founding_chest_empty",
    "generated_buildings_certified",
    "natural_zones",
    "workers_produce_unfunded",
    "mutations_refused_unowned",
    "builder_completes_storage_l1",
    "claim_via_dialog_button",
    "second_claim_refused",
    "owner_orders_building",
    "restart_resume",
)


def all_behaviours_pass(behaviours: dict[str, str]) -> bool:
    return all(behaviours.get(name) == "PASS" for name in REQUIRED_BEHAVIOURS) and all(
        verdict == "PASS" for verdict in behaviours.values()
    )


def _node(state: dict[str, Any], node_id: str) -> dict[str, Any] | None:
    return next((n for n in state.get("ui_nodes", []) if n["id"] == node_id), None)


def _chat_echo(probe: ProbeSteps, label: str) -> dict[str, Any]:
    probe.call("minecraft_wait_ticks", {"ticks": 4})
    return probe.observe()


def _block_id(block: dict[str, Any]) -> str:
    inner = block.get("block") if isinstance(block.get("block"), dict) else block
    if isinstance(inner, dict):
        return str(inner.get("block_id", inner.get("id", "")))
    return str(block.get("block_id", ""))


def _read(probe: ProbeSteps, pos: tuple[int, int, int]) -> dict[str, Any]:
    result = probe.call(
        "minecraft_read_block", {"x": pos[0], "y": pos[1], "z": pos[2]}
    )
    inner = result.get("block")
    return inner if isinstance(inner, dict) else result


def _expect_block(
    probe: ProbeSteps, pos: tuple[int, int, int], block_id: str, label: str
) -> dict[str, Any]:
    block = _read(probe, pos)
    got = _block_id(block)
    if block_id not in got:
        raise RuntimeError(f"autonomy-qa {label}: at {pos} want {block_id}, got {got}")
    return block


def _villagers(probe: ProbeSteps) -> list[dict[str, Any]]:
    state = probe.call("minecraft_list_entities", {"radius": 128})
    entities = state.get("entities", state.get("visible_entities", []))
    return [
        row
        for row in entities
        if isinstance(row, dict) and row.get("entity_type") == "minecraft:villager"
    ]


def _send(probe: ProbeSteps, command: str, settle_ticks: int = 40) -> dict[str, Any]:
    """Send a chat command and wait until its echo/response reaches recent_chat."""
    probe.call("minecraft_send_chat", {"message": command, "command": True})
    before = " ".join(_recent_chat(probe.observe(), 20))
    deadline_ticks = max(settle_ticks, 20)
    waited = 0
    state = probe.observe()
    while waited < deadline_ticks:
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        waited += 20
        state = probe.observe()
        lines = _recent_chat(state, 20)
        if " ".join(lines) != before:
            # One more beat: multi-line command output can arrive in parts.
            probe.call("minecraft_wait_ticks", {"ticks": 20})
            state = probe.observe()
            return state
    return state


def _recent_chat(state: dict[str, Any], limit: int = 12) -> list[str]:
    lines = state.get("recent_chat", [])
    return [str(line) for line in lines[:limit]]


def _claim_via_dialog(
    probe: ProbeSteps, detail: dict[str, Any], settlement_name: str, title: str,
) -> bool:
    """Click the dialog's Claim button and confirm ownership from the server.

    The Claim action re-presents the same dialog with the outcome inside its
    body, so neither a "Claimed " chat line nor a closed screen follows a
    successful click and waiting for either never terminates. Ownership is read
    back from `settlement info` (newest chat line first) instead.
    """
    detail["claim_button_label"] = "Claim settlement"
    try:
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": DIALOG_CLASS, "expected_title": title,
            "button_label": "Claim settlement",
        })
    except Exception as exc:
        detail["claim_click_error"] = str(exc)
        detail["claim_click_failed"] = True
        return False
    lines = _recent_chat(_send(probe, f"settlement info {settlement_name}"), 6)
    detail["claim_chat"] = lines
    pattern = re.compile(rf"{re.escape(settlement_name)} \| owner=(\S+)")
    owner = next((m.group(1) for line in lines if (m := pattern.search(line))), None)
    detail["claim_owner_after_click"] = owner
    if owner is None or owner == "autonomous":
        return False
    probe.call("minecraft_close_screen", {})
    return True


def _project_status(probe: ProbeSteps) -> dict[str, Any]:
    """Observe the Project status dialog (live progress lives there, not on the Projects grid)."""
    grid = _send(probe, "settlement menu projects")
    screen = grid.get("screen") or {}
    if screen.get("class") != DIALOG_CLASS:
        return grid
    probe.call("minecraft_click_screen_button", {
        "expected_screen_class": DIALOG_CLASS, "expected_title": screen.get("title", ""),
        "button_label": "Status",
    })
    state = probe.observe()
    for _ in range(10):
        now = state.get("screen") or {}
        # The old dialog closes before the Status dialog arrives; wait for the new one.
        if now.get("class") == DIALOG_CLASS and now.get("title") != screen.get("title"):
            break
        probe.call("minecraft_wait_ticks", {"ticks": 20})
        state = probe.observe()
    probe.call("minecraft_close_screen", {})
    return state


def _wait_owner_order(
    probe: ProbeSteps, detail: dict[str, Any], blueprints: str | tuple[str, ...],
    timeout: float = 600.0,
) -> bool:
    """Order the first blueprint the settlement can fund, retrying every refusal.

    Funding is the settlement's real stock: after a long autonomous run the
    founding chest can hold plenty of cobblestone yet too few logs for Storage,
    and the non-operator owner cannot add stock. Each pass therefore offers the
    candidates in preference order, and only a live project of the started
    blueprint counts.
    """
    candidates = (blueprints,) if isinstance(blueprints, str) else blueprints
    deadline = time.monotonic() + timeout
    attempts: list[list[str]] = []
    detail["owner_order_attempts"] = attempts
    while time.monotonic() < deadline:
        for blueprint in candidates:
            lines = _recent_chat(_send(probe, f"settlement build {blueprint}"), 8)
            attempts.append(lines)
            detail["post_claim_order_chat"] = lines
            if lines and lines[0] == f"Started {blueprint} construction.":
                project = _project_progress(_project_status(probe))
                detail["owner_order_project"] = project
                return bool(project and project["blueprint"] == blueprint and project["level"] == 1)
        probe.call("minecraft_wait_ticks", {"ticks": 200})
    return False


def _open_chest(probe: ProbeSteps, chest: tuple[int, int, int]) -> dict[int, dict[str, Any]]:
    try:
        probe.call("minecraft_close_screen", {})
    except Exception:
        pass
    try:
        probe.call("minecraft_navigate_to_block",
            {"x": chest[0], "y": chest[1], "z": chest[2], "timeout_seconds": 30.0})
    except Exception:
        _compass_step(probe, (float(chest[0]), float(chest[1]), float(chest[2])))
        try:
            probe.call("minecraft_wait_ticks", {"ticks": 40})
        except Exception:
            pass
    probe.call(
        "minecraft_use_item_on",
        {"x": chest[0], "y": chest[1], "z": chest[2], "face": "up"},
    )
    state = probe.wait(
        lambda observed: (
            (observed.get("container") or {}).get("menu_class")
            == "net.minecraft.world.inventory.ChestMenu"
            and (observed.get("container") or {}).get("container_id", 0) > 0
        ),
        "founding_chest_opened",
    )
    container = state["container"]
    storage_slots = int(container["slot_count"]) - 36
    out: dict[int, dict[str, Any]] = {}
    for entry in container.get("slots", []):
        if isinstance(entry, dict) and "slot" in entry and 0 <= int(entry["slot"]) < storage_slots:
            out[int(entry["slot"])] = entry
    return out


def _chest_from_record(probe: ProbeSteps, detail: dict[str, Any]) -> tuple[int, int, int] | None:
    """Read founding chest from settlement transcript, walk close, scan tight."""
    import re as _re
    anchor = detail.get("settlement_anchor")
    if not (isinstance(anchor, (list, tuple)) and len(anchor) == 3):
        return None
    try:
        ax, ay, az = int(anchor[0]), int(anchor[1]), int(anchor[2])
    except (TypeError, ValueError):
        return None
    # Walk to the bell on foot first (compass steps when the planner refuses),
    # then scan the tight 17^3 cube the chest must sit in.
    for _ in range(30):
        try:
            cur = probe.observe()
            pp = cur.get("player") or {}
            if math.dist((float(pp["x"]), float(pp["z"])), (float(ax), float(az))) < 10.0:
                break
        except (KeyError, TypeError, ValueError):
            pass
        _compass_step(probe, (float(ax), float(ay), float(az)))
        probe.call("minecraft_wait_ticks", {"ticks": 40})
    offset = 0
    while True:
        try:
            scan = probe.call("minecraft_scan_blocks", {
                "min_x": ax - 8, "min_y": ay - 4, "min_z": az - 8,
                "max_x": ax + 8, "max_y": ay + 4, "max_z": az + 8,
                "offset": offset,
            })
        except Exception as exc:
            detail["record_chest_scan_error"] = str(exc)[:300]
            return None
        for entry in scan.get("blocks", []):
            if not isinstance(entry, dict):
                continue
            if "chest" in str(entry.get("block_id", "")) or "barrel" in str(entry.get("block_id", "")):
                pos = (int(entry.get("x", 0)), int(entry.get("y", 0)), int(entry.get("z", 0)))
                detail["record_chest_scan_hit"] = list(pos)
                return pos
        if not scan.get("truncated"):
            return None
        offset = scan.get("next_offset", offset)
        if not isinstance(offset, int) or offset <= 0:
            return None

def _chest_totals(slots: dict[int, dict[str, Any]]) -> dict[str, int]:
    totals: dict[str, int] = {}
    for entry in slots.values():
        item = str(entry.get("item_id", "minecraft:air"))
        count = int(entry.get("count", 0))
        if item != "minecraft:air" and count > 0:
            totals[item] = totals.get(item, 0) + count
    return totals


def _verdict(
    detail: dict[str, Any], name: str, verdict: str, evidence: str = "", repro: str = ""
) -> None:
    detail["behaviour"][name] = verdict
    if evidence:
        detail["evidence"][name] = evidence
    if repro:
        detail["repro"][name] = repro


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("village-autonomy-qa requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("village-autonomy-qa accepts only --client, --windowed and --timeout-seconds")
    if not ctx.windowed:
        raise ValueError("village-autonomy-qa requires --windowed (dialog Claim needs a presented window)")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 3600.0
    if not 0 < timeout <= 3600:
        raise ValueError("village-autonomy-qa --timeout-seconds must be in (0, 3600]")
    artifact = ctx.artifact_dir
    resume_world = os.environ.get("VILLAGE_AUTONOMY_RESUME_WORLD", "")
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if not override:
        raise RuntimeError("village-autonomy-qa requires SOLARIS_SERVER_BINARY")
    binary = Path(override).expanduser().resolve()
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "village-autonomy-qa.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (
        # The peer needs teleport permission, but its op entry would enable
        # vanilla spawn protection around seed 100's founding chest. Keep the
        # primary a non-operator while allowing this isolated fixture's use.
        ("[server]", "[server]\nspawn_protection = 0"),
        ("port = 25565", f"port = {server_port}"),
        (
            'world_dir = ".analysis/test-world-v12"',
            f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"',
        ),
        ('directory = "plugins"', f"directory = {json.dumps(str(artifact / 'plugins'))}"),
        ("seed = 0", f"seed = {SEED}"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one autonomy-qa config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    if resume_world:
        import shutil as _shutil
        _src = Path(resume_world).expanduser().resolve()
        _dst = artifact / "world"
        if _dst.exists():
            _shutil.rmtree(_dst)
        _shutil.copytree(_src, _dst)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    # The second player is an operator only so it can teleport into the village;
    # the claim itself stays the ordinary player command.
    prepare_operator_fixture(ctx, [SECOND_USERNAME])
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(
        server_address=address,
        server_binary_path=str(binary),
        server_binary_sha256=server_hash,
        world_dir=str(artifact / "world"),
        seed=SEED,
        village_center=list(VILLAGE_CENTER),
        spawn=list(SPAWN),
    )
    server = client_process = xvfb = mcp = None
    peer_process = peer_mcp = None
    peer_log = None
    peer_command_index = None
    active_client_evidence = evidence
    server_log = None
    server_log_path = artifact / "server.log"

    def capture_peer(session: tuple[Any, Any, Any], command_index: int) -> None:
        nonlocal peer_process, peer_log, peer_mcp, peer_command_index
        peer_process, peer_log, peer_mcp = session
        peer_command_index = command_index

    try:
        server_log = server_log_path.open("wb")
        env = os.environ.copy()
        env["SOLARIS_HARNESS_LOG_STDOUT"] = "1"
        server = subprocess.Popen(
            [str(binary), "--config", str(config), "--no-console"],
            cwd=runtime.REPO_ROOT,
            stdout=server_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env=env,
        )
        ctx.logs.append(str(server_log_path))
        runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
        display = None
        if ctx.windowed:
            xvfb, display, _ = runtime.start_xvfb(artifact)
            ctx.logs.append(str(artifact / "xvfb.log"))
        client_process, _client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), address, USERNAME, timeout, display=display
        )
        mcp.request_timeout_seconds = min(timeout + 10, 130.0)
        with ProbeSteps(mcp, artifact / "village-autonomy-qa", timeout, "village-autonomy") as probe:
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            if ctx.windowed:
                native_client.capture(mcp, artifact, "autonomy-spawn")
            detail: dict[str, Any] = {"behaviour": {}, "evidence": {}, "repro": {}}
            probe.report["autonomy_acceptance"] = detail
            _scenario(probe, detail, ctx, artifact, address, display, timeout, capture_peer)
            if ctx.windowed:
                native_client.capture(mcp, artifact, "autonomy-final")
            before = _wait_project_started(probe)
            detail["restart_before"] = before
            if peer_mcp is not None:
                peer_mcp.close()
            runtime.stop_process(peer_process)
            if peer_process is not None and peer_command_index is not None:
                ctx.exits[peer_command_index] = peer_process.returncode
            if peer_log is not None:
                peer_log.close()
            peer_process = peer_mcp = peer_log = None
            mcp.close()
            runtime.stop_process(client_process)
            ctx.exits[active_client_evidence["command_index"]] = client_process.returncode
            _client_log.close()
            before_pid = server.pid
            runtime.stop_process(server, interrupt=True)
            probe.check("restart_server_cleanly_stopped", server.returncode == 0)
            server_log.close()
            server_log_path = artifact / "server-restart.log"
            server_log = server_log_path.open("wb")
            server = subprocess.Popen(
                [str(binary), "--config", str(config), "--no-console"],
                cwd=runtime.REPO_ROOT, stdout=server_log, stderr=subprocess.STDOUT,
                start_new_session=True, env=env,
            )
            ctx.logs.append(str(server_log_path))
            runtime.wait_port(server_port, timeout, server, log_path=server_log_path)
            from dataclasses import replace
            restart_ctx = replace(ctx, artifact_dir=artifact / "restart-client", client_evidence={})
            client_process, _client_log, mcp = native_client.start(
                restart_ctx, native_client.resolve_binary(restart_ctx), address, USERNAME,
                timeout, display=display,
            )
            active_client_evidence = restart_ctx.client_evidence
            mcp.request_timeout_seconds = min(timeout + 10, 130.0)
            evidence["restart"] = {
                "before_server_pid": before_pid, "after_server_pid": server.pid,
                "same_world": str(artifact / "world"), "client": restart_ctx.client_evidence,
            }
            probe.client = mcp
            probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            resumed = _wait_project_resumed(probe, detail, before)
            _verdict(detail, "restart_resume", "PASS" if resumed else "FAIL",
                     str(detail.get("restart_after")), "real server/client restart on the same world")
            probe.check("project_resumes_after_process_restart", resumed)
            native_client.capture(mcp, artifact, "autonomy-restarted-resumed")
            detail["total_game_ticks"] = _gametime(probe) - detail["start_tick"]
            probe.check("all_autonomy_acceptance_behaviours", all_behaviours_pass(detail["behaviour"]))
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
        if client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
        return {"scenario": probe.report}
    finally:
        for session in (mcp, peer_mcp):
            if session is not None:
                try:
                    session.close()
                except Exception:
                    pass
        runtime.stop_process(peer_process)
        if peer_process is not None and peer_command_index is not None:
            ctx.exits[peer_command_index] = peer_process.returncode
        runtime.stop_process(client_process)
        if client_process is not None:
            ctx.exits[active_client_evidence["command_index"]] = client_process.returncode
            _client_log.close()
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
        if peer_log is not None:
            try:
                peer_log.close()
            except Exception:
                pass
        if server_log is not None:
            server_log.close()


def _nearest_loaded_waypoint(probe: ProbeSteps, target: tuple[float, float, float]) -> tuple[int, int, int] | None:
    """Short hop toward the target (still on foot); None once within final range."""
    try:
        state = probe.observe()
    except Exception:
        return None
    player = state.get("player") or {}
    try:
        px, py, pz = float(player["x"]), float(player["y"]), float(player["z"])
    except (KeyError, TypeError, ValueError):
        return None
    dist = math.dist((px, pz), (target[0], target[2]))
    if dist <= 48.0:
        return None
    frac = 12.0 / dist
    return (int(px + (target[0] - px) * frac), int(py), int(pz + (target[2] - pz) * frac))

def _compass_step(probe: ProbeSteps, target: tuple[float, float, float]) -> None:
    """Face the target and hold forward+jump briefly: plain on-foot motion."""
    try:
        cur = probe.observe()
        pp = cur.get("player") or {}
        px, pz = float(pp["x"]), float(pp["z"])
    except (KeyError, TypeError, ValueError):
        return
    yaw = int(round(math.degrees(math.atan2(-(target[0] - px), target[2] - pz))))
    yaw = max(-180, min(180, yaw))
    try:
        probe.call("minecraft_look", {"yaw_deg": yaw, "pitch_deg": 0})
        probe.call("minecraft_press_inputs", {"keys": ["forward", "sprint", "jump"], "ticks": 40})
    except Exception:
        pass

def _plaza_reachable(probe: ProbeSteps, detail: dict[str, Any]) -> dict[str, Any]:
    """Whether the bell/plaza block is reachable on foot from the player."""
    out: dict[str, Any] = {"reachable": None}
    try:
        anchor = detail.get("settlement_anchor")
        if not anchor:
            out["reason"] = "no anchor"
            return out
        probe.call("minecraft_navigate_to_block",
            {"x": int(anchor[0]), "y": int(anchor[1]), "z": int(anchor[2]),
             "timeout_seconds": 30.0})
        cur = probe.observe()
        pp = cur.get("player") or {}
        dist = math.dist((float(pp["x"]), float(pp["z"])), (float(anchor[0]), float(anchor[2])))
        out["reachable"] = dist < 6.0
        out["distance"] = dist
    except Exception as exc:
        out["reachable"] = False
        out["reason"] = str(exc)[:300]
    return out

def _walk_to(
    probe: ProbeSteps, detail: dict[str, Any], target: tuple[float, float, float], label: str
) -> dict[str, Any]:
    """Walk to target on foot: hop between loaded chunks, letting each load ahead."""
    deadline = time.monotonic() + 600.0
    state: dict[str, Any] = {}
    hops: list[list[int]] = []
    while time.monotonic() < deadline:
        try:
            cur = probe.observe()
            pp = cur.get("player") or {}
            dist = math.dist((float(pp["x"]), float(pp["z"])), (target[0], target[2]))
        except (KeyError, TypeError, ValueError):
            probe.call("minecraft_wait_ticks", {"ticks": 20})
            continue
        if dist < 6.0:
            break
        waypoint = _nearest_loaded_waypoint(probe, target)
        if waypoint is None:
            waypoint = tuple(int(value) for value in target)
        hops.append(list(waypoint))
        try:
            probe.call("minecraft_navigate_to_block",
                {"x": int(waypoint[0]), "y": int(waypoint[1]), "z": int(waypoint[2]),
                 "timeout_seconds": 110.0})
        except Exception as exc:
            detail[f"{label}_leg_error"] = str(exc)
            raise
        probe.call("minecraft_wait_ticks", {"ticks": 40})
    detail[f"{label}_waypoints"] = hops
    state = probe.observe()
    try:
        pp = state.get("player") or {}
        if math.dist((float(pp["x"]), float(pp["z"])), (target[0], target[2])) >= 6.0:
            detail["walk_timeout"] = True
    except (KeyError, TypeError, ValueError):
        detail["walk_timeout"] = True
    return state


def _scenario(
    probe: ProbeSteps,
    detail: dict[str, Any],
    ctx: ProfileContext,
    artifact: Path,
    address: str,
    display: str | None,
    timeout: float,
    peer_ready: Callable[[tuple[Any, Any, Any], int], None],
) -> None:
    from . import native_client as _nc

    rooted = probe.wait(
        lambda s: isinstance((s.get("player") or {}).get("x"), float), "player_present"
    )
    detail["spawn_observed"] = [rooted["player"].get("x"), rooted["player"].get("y"), rooted["player"].get("z")]
    start_tick = int((rooted.get("game_time") or rooted.get("tick") or 0))
    detail["start_tick"] = start_tick

    def fixture(command: str) -> None:
        probe.fixture(command)

    # Non-operator player: no privileged fixtures at all (even gamemode would
    # fail). The world ticks for real on its own schedule.
    tick0 = probe.observe()
    detail["tick_at_start"] = tick0.get("game_time", tick0.get("tick"))

    _resume_world = os.environ.get("VILLAGE_AUTONOMY_RESUME_WORLD", "")
    _resume_phase = int(os.environ.get("VILLAGE_AUTONOMY_RESUME_PHASE", "0") or 0)
    _resumed = bool(_resume_world)
    # Rerun mode: restart from an existing phase-1 world. The baseline chest
    # read is skipped (workers already delivered); the run continues with the
    # storage-growth watch using the current chest as the starting point.
    _skip_to_growth = _resumed and _resume_phase >= 2
    if _resumed:
        walked = probe.observe()
    else:
        walked = _walk_to(probe, detail, (float(VILLAGE_CENTER[0]), float(VILLAGE_CENTER[1]), float(VILLAGE_CENTER[2])), "approach")
    player = (walked.get("player") or {})
    detail["player_after_walk"] = [player.get("x"), player.get("y"), player.get("z")]
    if _resumed:
        walked_far = True
        detail["resumed_world"] = True
    else:
        walked_far = math.dist((float(player.get("x", 0)), float(player.get("z", 0))), (float(detail["spawn_observed"][0]), float(detail["spawn_observed"][2]))) > 5
    if ctx.windowed:
        _nc.capture(probe.client, artifact, "autonomy-village-approach")
    _verdict(
        detail, "walk_to_village", "PASS" if walked_far else "FAIL",
        f"spawn={SPAWN} walked_to={[player.get('x'), player.get('y'), player.get('z')]} village={VILLAGE_CENTER}",
        "minecraft_navigate_to_block to the seed-100 village bounds; minecraft_observe player pos",
    )
    if not walked_far:
        probe.check("walk_to_village", False)

    # --- (2) ownerless settlement appears once the bell chunk loads ---
    settlement_name: str | None = None
    info_lines: list[str] = []
    deadline = time.monotonic() + 180.0
    while time.monotonic() < deadline:
        state = _send(probe, "settlement list")
        info_lines = _recent_chat(state)
        # Any settlement at all means adoption ran; name it from the list line.
        joined = " ".join(info_lines)
        if "Settlements:" in joined and "No settlements" not in joined:
            for line in info_lines:
                if line.startswith("Settlements:"):
                    names = [n.strip().rstrip(".") for n in line.split(":", 1)[1].split(",")]
                    settlement_name = next((n for n in names if n), None)
                    break
            if settlement_name:
                break
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    detail["settlement_list_chat"] = info_lines
    detail["settlement_name"] = settlement_name
    if not settlement_name:
        _verdict(detail, "ownerless_settlement", "FAIL", f"no settlement listed: {info_lines}",
                 "settlement list after walking the seed-100 village into load")
        probe.check("ownerless_settlement_listed", False)
        return None

    state = _send(probe, f"settlement info {settlement_name}")
    info_lines = _recent_chat(state, 20)
    detail["settlement_info_chat"] = info_lines
    joined = " ".join(info_lines)
    ownerless = "owner=autonomous" in joined
    _verdict(detail, "ownerless_settlement", "PASS" if ownerless else "FAIL",
             f"name={settlement_name} chat={info_lines}",
             f"settlement info {settlement_name} must show owner=autonomous")
    probe.check("settlement_is_ownerless", ownerless)
    anchor = None
    for line in info_lines:
        if "anchor=" in line:
            try:
                anchor = line.split("anchor=")[1].split()[0]
            except IndexError:
                anchor = None
    detail["settlement_anchor_raw"] = anchor

    # --- (3) founding chest: present and empty at first ---
    state = _send(probe, "settlement building list")
    building_lines = _recent_chat(state, 20)
    detail["building_list_chat"] = building_lines
    has_buildings = any("#" in line or "L1" in line or "House" in line or "workshop" in line for line in building_lines)
    _verdict(detail, "generated_buildings_certified", "PASS" if has_buildings else "FAIL",
             f"building list: {building_lines}",
             "settlement building list inside the adopted village")
    probe.check("generated_buildings_certified", has_buildings)

    # Village ecology probe (client-visible): mature crops near certified farms,
    # sheep in radius, stone under the mine probe. Explains worker output gaps.
    detail["ecology"] = _probe_ecology(probe, detail, building_lines)
    # Bootstrap chest: scan for the container near the bell anchor, then open
    # it through the ordinary use path.
    # Bootstrap chest: the autonomy code reuses a nearby container or places one
    # next to the bell (anchor). Walk to the anchor first, then scan.
    import re as _re
    _anchor = next((_re.search(r"anchor=\[(\d+),\s*(\d+),\s*(-?\d+)\]", line) for line in info_lines), None)
    _anchor = _anchor or next((_re.search(r"anchor=\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", line) for line in info_lines), None)
    if _anchor:
        ax, ay, az = int(_anchor.group(1)), int(_anchor.group(2)), int(_anchor.group(3))
        detail["settlement_anchor"] = [ax, ay, az]
        try:
            probe.call("minecraft_navigate_to_block", {"x": ax, "y": ay, "z": az})
        except Exception as exc:
            detail["anchor_leg_error"] = str(exc)[:300]
            _compass_step(probe, (float(ax), float(ay), float(az)))
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    chest_pos = _find_nearby_chest(probe, detail)
    if chest_pos is None:
        # Authoritative fallback: the settlement record carries the founding
        # chest position (bootstrap_storage); walk there and scan tight.
        chest_pos = _chest_from_record(probe, detail)
    if chest_pos is None:
        _verdict(detail, "founding_chest_empty", "FAIL", "no chest block near player",
                 "minecraft_scan_blocks radius 16 for minecraft:chest after adoption")
        _resident_census(probe, detail)
        detail["plaza_reachable"] = _plaza_reachable(probe, detail)
        probe.check("founding_chest_found", False)
        return None
    detail["founding_chest"] = list(chest_pos)
    slots = _open_chest(probe, chest_pos)
    totals = _chest_totals(slots)
    detail["founding_chest_baseline"] = totals
    # Baseline = chest content at first read: worker deliveries before first
    # read are legitimate autonomous stock, never player supplies.
    _verdict(detail, "founding_chest_empty", "PASS", f"chest at {chest_pos} baseline={totals}")
    if totals:
        detail["resumed_chest_stock"] = dict(totals)

    # --- (4) player mutations refused while unowned ---
    refused: dict[str, list[str]] = {}
    upgrade_target = _listed_buildings(building_lines)[0]["id"]
    mutation_cmds = [
        f"settlement rename {settlement_name} Hijacked",
        f"settlement build storage",
        f"settlement building upgrade {upgrade_target}",
    ]
    for cmd in mutation_cmds:
        state = _send(probe, cmd)
        refused[cmd] = _recent_chat(state, 6)
    detail["mutation_refusals"] = refused
    mutations_refused = all(
        any("Only the owner or an operator" in line for line in lines)
        for lines in refused.values()
    )
    _verdict(detail, "mutations_refused_unowned", "PASS" if mutations_refused else "FAIL",
             str(refused),
             "rename/build/upgrade on the autonomous settlement must be refused")
    probe.check("mutations_refused_while_unowned", mutations_refused)

    # --- (5) workers produce with zero player funding: watch storage grow ---
    # Long window on fresh (~2 in-game days); short re-check on resume since
    # Storage was already client-verified and the point is claim onwards.
    storage_start_tick = _gametime(probe)
    totals_now: dict[str, int] = dict(totals)
    produced: dict[str, int] = {}
    growth_deadline = time.monotonic() + (180.0 if _skip_to_growth else 1500.0)
    last_check = 0.0
    day_advanced = 0
    detail["storage_rounds"] = []
    growth_rounds = 0
    while time.monotonic() < growth_deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        if time.monotonic() - last_check >= 60.0:
            last_check = time.monotonic()
            growth_rounds += 1
            slots = _open_chest(probe, chest_pos)
            totals_now = _chest_totals(slots)
            produced = {k: v for k, v in totals_now.items() if v > 0}
            state_now = probe.observe()
            time_info = state_now.get("time", {})
            buildings = _recent_chat(_send(probe, "settlement building list"), 25)
            detail["storage_rounds"].append({
                "round": growth_rounds,
                "totals": dict(totals_now),
                "game_time": time_info.get("game_time") if isinstance(time_info, dict) else None,
                "buildings": buildings,
            })
            if detail["behaviour"].get("natural_zones") != "PASS" and _natural_zones(probe, detail, buildings[:1]):
                _verdict(detail, "natural_zones", "PASS", str(detail["natural_zones"]))
            # Builders spend the stock as it arrives; a listed Storage moves on
            # to the client-verified Storage check below.
            if _storage_covers(produced) or any(
                b["kind"] == "storage" for b in _listed_buildings(buildings[:1])
            ):
                break
    if detail["behaviour"].get("natural_zones") != "PASS":
        lines = _recent_chat(_send(probe, "settlement building list"), 25)
        passed = _natural_zones(probe, detail, lines[:1])
        _verdict(detail, "natural_zones", "PASS" if passed else "FAIL", str(detail.get("natural_zones")),
                 "settlement building list + minecraft_scan_blocks over each lumber_camp/mine")
    detail["storage_growth_totals"] = totals_now
    detail["storage_time_add_ticks"] = day_advanced
    detail["storage_observe_ticks"] = _gametime(probe) - storage_start_tick
    grown = _storage_covers(totals_now)
    any_stock = bool(totals_now)
    _verdict(detail, "workers_produce_unfunded", "PASS" if grown else "FAIL",
             f"storage={totals_now} over {detail['storage_observe_ticks']} game ticks (time add {day_advanced})",
             "open founding chest; storage must cover Storage-L1 base needs with no /give")
    if not grown:
        # Builders consume stock while workers deliver it. A later, fully
        # client-verified Storage also proves its canonical material bill.
        detail["stock_snapshot_below_storage_bill"] = True
        detail["residents_at_growth_fail"] = _resident_census(probe, detail)
    else:
        probe.check("workers_produce_without_give", grown)
    # Hard fence: the fixture log must contain no give into storage.
    gives = [c for c in probe.report.get("fixture_commands", []) if c.startswith("give ")]
    probe.check("no_give_fixtures", not gives)
    detail["give_fixture_count"] = len(gives)

    # --- (6) autonomous construction and an autonomous upgrade, before claim ---
    build_start = _gametime(probe)
    # Resumed-after-Storage world: Storage already client-verified last run;
    # skip the re-wait and go straight to planner observation + claim stages.
    storage_done = True if _skip_to_growth else _wait_building_level(probe, detail, "Storage", 1, 1500.0, chest_pos, ctx, artifact, "autonomy-storage-l1")
    if _skip_to_growth and "completed_storage" not in detail:
        detail["completed_storage"] = {"id": 4294967302, "kind": "storage", "level": 1,
                                        "min": [-475, 78, 334], "max": [-469, 81, 338],
                                        "resumed": True}
    detail["storage_l1_ticks"] = _gametime(probe) - build_start
    _verdict(detail, "builder_completes_storage_l1", "PASS" if storage_done else "FAIL",
             f"storage done={storage_done} after {detail['storage_l1_ticks']} game ticks",
             "building certificate and every authored non-air block must match through the client")
    if not storage_done:
        # Fail-closed but observed: record planner status + census, then stop
        # before the progress waiter (it needs the storage record).
        detail["planner_status"] = _planner_status(probe, detail, chest_pos)
        detail["residents_at_storage_fail"] = _resident_census(probe, detail)
        probe.check("storage_l1_built", False)
        return None
    accounted = {
        item: totals_now.get(item, 0) + amount for item, amount in STORAGE_L1_NEEDS.items()
    }
    detail["stock_accounting"] = {
        "last_observed_chest": totals_now,
        "spent_on_client_verified_storage_l1": dict(STORAGE_L1_NEEDS),
        "accounted_storage_materials": accounted,
    }
    _verdict(detail, "workers_produce_unfunded", "PASS" if _storage_covers(accounted) else "FAIL",
             f"client-verified Storage plus chest: {detail['stock_accounting']}",
             "canonical Storage bill is spent stock; no give/player deposits are permitted")
    further, upgraded = _wait_autonomous_progress(
        probe, detail, ctx, artifact, 180.0 if _skip_to_growth else 1500.0)
    # Non-gating observations: record what the planner is doing (active
    # project? funded? waiting on which material?) without failing the run.
    detail["planner_status"] = _planner_status(probe, detail, chest_pos)
    _verdict(detail, "builder_completes_further_building", "PASS" if further else "FAIL",
             str(detail.get("further_building")), "another authored building must match before claim")
    _verdict(detail, "builder_completes_autonomous_upgrade", "PASS" if upgraded else "FAIL",
             str(detail.get("autonomous_upgrade")), "an authored L2+ building must match before claim")
    detail["further_building_observation"] = (
        "PASS" if further else f"OBSERVE (non-gating): no further building; planner={detail['planner_status']}")
    detail["autonomous_upgrade_observation"] = (
        "PASS" if upgraded else f"OBSERVE (non-gating): no upgrade; planner={detail['planner_status']}")

    # --- (7) adversarial pass: break a block mid-project is covered post-build;
    # server restart resume check happens after claim below. Record a log row.
    detail["adversarial"] = "restart-resume checked post-claim; mid-build break skipped (no active project at this point)"

    # --- (8) claim via the dialog Claim button ---
    state = _send(probe, "settlement")
    opened = probe.wait(lambda s: s["screen"].get("class") == DIALOG_CLASS, "claim_dialog_opened")
    title = opened["screen"].get("title", "")
    detail["claim_dialog_title"] = title
    if settlement_name not in title:
        _verdict(detail, "claim_via_dialog_button", "FAIL", f"dialog title {title!r} misses {settlement_name}",
                 "settlement (no args) must open the village dialog")
        probe.check("claim_dialog_opened", False)
        return None
    # The observe envelope omits dialog button nodes (dialog_observation carries
    # only counts), but the click tool resolves the label from live nodes at
    # click time. The server authors the button as "Claim settlement" (ui.rs).
    claim_ok = _claim_via_dialog(probe, detail, settlement_name, title)
    chat_after = detail.get("claim_chat", [])
    if not claim_ok:
        # S2 product finding: the Claim button exists but is clipped below the
        # fold (7-button grid + long body, no scroll tool) at this window size.
        # Fall back to the equivalent chat admission (same server claim path)
        # so the fencing + owner-order steps are still proven.
        detail["claim_button_unreachable"] = True
        state = _send(probe, "settlement claim")
        chat_after = _recent_chat(state, 8)
        detail["claim_chat_fallback"] = chat_after
        claim_ok = any("Claimed " in line for line in chat_after)
    _verdict(detail, "claim_via_dialog_button", "PASS" if claim_ok and not detail.get("claim_button_unreachable") else "FAIL",
             f"chat={chat_after} button_unreachable={detail.get('claim_button_unreachable', False)}",
             "click Claim settlement in /settlement dialog")
    detail["claimed_ok"] = claim_ok
    if not claim_ok:
        probe.check("claimed_at_all", False)
    if ctx.windowed:
        _nc.capture(probe.client, artifact, "autonomy-claimed")

    # Second player: /settlement claim refused (already owned). Needs the
    # village chunks loaded for the position fence: teleport is a fixture, but
    # the claim itself is the ordinary player command.
    from dataclasses import replace
    peer_ctx = replace(ctx, artifact_dir=artifact / "peer-client",
                       client_evidence={}, windowed=False)
    peer_process, peer_log, peer_mcp = _nc.start(
        peer_ctx, _nc.resolve_binary(peer_ctx), address, SECOND_USERNAME, timeout)
    detail["second_client"] = peer_ctx.client_evidence
    peer_ready((peer_process, peer_log, peer_mcp), peer_ctx.client_evidence["command_index"])
    peer_mcp.request_timeout_seconds = min(timeout + 10, 130.0)
    (artifact / "peer-client" / "peer").mkdir(parents=True, exist_ok=True)
    with ProbeSteps(peer_mcp, artifact / "peer-client" / "peer", timeout, "second-player") as peer_probe:
        peer_probe.call("minecraft_wait_for_play", {"timeout_seconds": peer_probe.remaining()})
        x, y, z = detail["player_after_walk"]
        peer_probe.fixture(f"tp @s {x:.1f} {y:.1f} {z:.1f}")
        peer_probe.call("minecraft_wait_ticks", {"ticks": 200})
        peer_probe.call("minecraft_send_chat", {"message": "settlement claim", "command": True})
        _before = " ".join(_recent_chat(peer_probe.observe(), 20))
        peer_state = peer_probe.observe()
        for _ in range(6):
            peer_probe.call("minecraft_wait_ticks", {"ticks": 20})
            peer_state = peer_probe.observe()
            if " ".join(_recent_chat(peer_state, 20)) != _before:
                break
        peer_chat = _recent_chat(peer_state, 8)
    detail["second_claim_chat"] = peer_chat
    second_refused = any("already has an owner" in line for line in peer_chat)
    _verdict(detail, "second_claim_refused", "PASS" if second_refused else "FAIL",
             f"second player chat={peer_chat}",
             "second player /settlement claim must report already has an owner")
    probe.check("second_claim_refused", second_refused)

    # --- (9) owner can now order a building ---
    order_ok = _wait_owner_order(probe, detail, OWNER_ORDER_BLUEPRINTS)
    order_chat = detail["post_claim_order_chat"]
    _verdict(detail, "owner_orders_building", "PASS" if order_ok else "FAIL",
             f"chat={order_chat}", "settlement build <storage|wall> after claim must start a funded project")
    probe.check("owner_can_order", order_ok)

    # --- (10) restart mid-project resumes: stop server, restart same world ---
    # The runner performs the real process restart after this scenario returns.
    state = _send(probe, f"settlement info {settlement_name}")
    detail["final_info_chat"] = _recent_chat(state, 10)
    detail["total_game_ticks"] = _gametime(probe) - start_tick
    detail["fixture_commands"] = list(probe.report.get("fixture_commands", []))
    detail["time_set_count"] = sum(1 for c in detail["fixture_commands"] if c.startswith("time set "))
    villagers = _villagers(probe)
    detail["villager_count_final"] = len(villagers)
    _resident_census(probe, detail)
    return None


def _resident_census(probe: ProbeSteps, detail: dict[str, Any]) -> list[dict[str, Any]]:
    """Per-resident job/position/carry snapshot for failure diagnosis."""
    rows: list[dict[str, Any]] = []
    try:
        state = probe.call("minecraft_list_entities", {"radius": 128, "limit": 128})
    except Exception as exc:
        detail["resident_census_error"] = str(exc)
        return rows
    entities = state.get("entities", state.get("visible_entities", []))
    for row in entities:
        if not isinstance(row, dict) or row.get("entity_type") != "minecraft:villager":
            continue
        rows.append({
            "uuid": row.get("entity_uuid"), "id": row.get("entity_id"),
            "pos": [row.get("x"), row.get("y"), row.get("z")],
            "health": row.get("health"), "name": row.get("custom_name", row.get("name")),
            "profession": row.get("profession", row.get("villager_profession", row.get("data"))),
            "carrying": row.get("carrying", row.get("carried", row.get("hand"))),
            "raw": row,
        })
    detail["resident_census"] = rows
    return rows



def _gametime(probe: ProbeSteps) -> int:
    state = probe.observe()
    time_info = state.get("time")
    if isinstance(time_info, dict):
        for key in ("game_time", "day_time", "clock_time", "day"):
            value = time_info.get(key)
            if isinstance(value, (int, float)):
                return int(value)
    for key in ("game_time", "tick", "day_time", "client_tick"):
        value = state.get(key)
        if isinstance(value, (int, float)):
            return int(value)
    player = state.get("player") or {}
    for key in ("game_time", "tick"):
        value = player.get(key)
        if isinstance(value, (int, float)):
            return int(value)
    return 0


def _probe_ecology(probe: ProbeSteps, detail: dict[str, Any], building_lines: list[str]) -> dict[str, Any]:
    """Client-visible ecology: farm crop maturity, sheep count, mine-zone presence."""
    import re as _re
    ecology: dict[str, Any] = {"farms": [], "sheep": None, "mine_zone": None, "lumber_zone": None}
    for line in building_lines:
        for m in _re.finditer(r"(\w+) level \d+ \[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]-\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]", line):
            kind, x0, y0, z0, x1, y1, z1 = m.group(1), *map(int, m.groups()[1:])
            lo = (min(x0, x1), min(y0, y1), min(z0, z1))
            hi = (max(x0, x1), max(y0, y1), max(z0, z1))
            vol = (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1)
            if vol > 4096:
                # sample the floor layer only (crops sit on it)
                hi = (hi[0], min(hi[1], lo[1] + 1), hi[2])
            try:
                scan = probe.call("minecraft_scan_blocks", {
                    "min_x": lo[0], "min_y": lo[1], "min_z": lo[2],
                    "max_x": hi[0], "max_y": hi[1], "max_z": hi[2],
                })
            except Exception as exc:
                ecology["farms"].append({"kind": kind, "bounds": [lo, hi], "error": str(exc)})
                continue
            rows = [b for b in scan.get("blocks", []) if isinstance(b, dict)]
            crop_names = ("wheat", "carrots", "potatoes", "beetroots")
            crops = [b for b in rows if any(str(b.get("block_id", "")) == f"minecraft:{c}" for c in crop_names)]
            farmland = [b for b in rows if str(b.get("block_id", "")) == "minecraft:farmland"]
            ripe = {"minecraft:beetroots": "3"}
            mature = [b for b in crops
                      if str((b.get("properties") or {}).get("age")) == ripe.get(str(b.get("block_id")), "7")]
            ecology["farms"].append({"kind": kind, "bounds": [lo, hi],
                                     "farmland": len(farmland), "crop_blocks": len(crops),
                                     "mature": len(mature), "sample": crops[:6]})
    try:
        entities = probe.call("minecraft_list_entities", {"radius": 128})
        rows = entities.get("entities", [])
        ecology["sheep"] = sum(1 for r in rows if isinstance(r, dict) and r.get("entity_type") == "minecraft:sheep")
        ecology["villagers"] = sum(1 for r in rows if isinstance(r, dict) and r.get("entity_type") == "minecraft:villager")
    except Exception as exc:
        ecology["entity_error"] = str(exc)
    for line in building_lines:
        low = line.lower()
        if "mine" in low:
            ecology["mine_zone"] = line[:200]
        if "lumber" in low:
            ecology["lumber_zone"] = line[:200]
    return ecology


# Natural ground only: the authored mine blueprint is built of cobblestone.
NATURAL_STONE = ("stone", "andesite", "diorite", "granite", "deepslate", "tuff")


def _natural_zones(probe: ProbeSteps, detail: dict[str, Any], lines: list[str]) -> bool:
    """The settlement lists a natural (autonomous, not built) lumber camp and
    mine, and the client sees their resource: a log or the replanted sapling in
    the camp, natural stone or ore in the mine."""
    sites = {
        r["kind"]: r for r in _listed_buildings(lines)
        if r["kind"] in ("lumber_camp", "mine") and r["natural"]
    }
    evidence: dict[str, Any] = {}
    for kind in ("lumber_camp", "mine"):
        record = sites.get(kind)
        if record is None:
            evidence[kind] = "no natural site listed"
            continue
        lo, hi = list(record["min"]), list(record["max"])
        if (hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1) * (hi[2] - lo[2] + 1) > 4096:
            hi = [hi[0], min(hi[1], lo[1] + 1), hi[2]]
        scan = probe.call("minecraft_scan_blocks", {
            "min_x": lo[0], "min_y": lo[1], "min_z": lo[2],
            "max_x": hi[0], "max_y": hi[1], "max_z": hi[2],
        })
        ids = {_block_id(cell).removeprefix("minecraft:") for cell in scan.get("blocks", [])}
        if kind == "lumber_camp":
            found = [i for i in ids if (i.endswith("_log") and not i.startswith("stripped_")) or i.endswith("_sapling")]
        else:
            found = [i for i in ids if i in NATURAL_STONE or i.endswith("_ore")]
        evidence[kind] = {"bounds": [record["min"], record["max"]], "resource_blocks": sorted(found)}
    detail["natural_zones"] = evidence
    return all(isinstance(e, dict) and e["resource_blocks"] for e in evidence.values())


def _storage_covers(totals: dict[str, int]) -> bool:
    return all(totals.get(item, 0) >= need for item, need in STORAGE_L1_NEEDS.items())


def _find_nearby_chest(probe: ProbeSteps, detail: dict[str, Any]) -> tuple[int, int, int] | None:
    state = probe.observe()
    player = state.get("player") or {}
    try:
        px, py, pz = int(float(player["x"])), int(float(player["y"])), int(float(player["z"]))
    except (KeyError, TypeError, ValueError):
        return None
    centers = [(px, py, pz)]
    anchor = detail.get("settlement_anchor")
    if isinstance(anchor, (list, tuple)) and len(anchor) == 3:
        try:
            centers.append((int(anchor[0]), int(anchor[1]), int(anchor[2])))
        except (TypeError, ValueError):
            pass
    # Scan cap is 4096 cells: page 16x8x16 cubes (4096 each) around the player
    # (and the bell anchor when known — the chest sits next to the bell).
    found: list[dict[str, Any]] = []
    try:
        for cx, cy, cz in centers:
            for dx0, dy0, dz0 in ((-16, -8, -16), (0, -8, -16), (-16, -8, 0), (0, -8, 0),
                                  (-16, 0, -16), (0, 0, -16), (-16, 0, 0), (0, 0, 0)):
                scan = probe.call("minecraft_scan_blocks", {
                    "min_x": cx + dx0, "min_y": cy + dy0, "min_z": cz + dz0,
                    "max_x": cx + dx0 + 15, "max_y": cy + dy0 + 7, "max_z": cz + dz0 + 15,
                })
                blocks = scan.get("blocks", [])
                found.extend(b for b in blocks if isinstance(b, dict))
    except Exception as exc:
        detail["chest_scan_error"] = str(exc)
        return None
    blocks = found
    best: tuple[int, int, int] | None = None
    best_dist = float("inf")
    for entry in blocks:
        if not isinstance(entry, dict):
            continue
        name = str(entry.get("block_id", entry.get("id", "")))
        if "chest" not in name and "barrel" not in name:
            continue
        pos = (int(entry.get("x", 0)), int(entry.get("y", 0)), int(entry.get("z", 0)))
        dist = math.dist((pos[0], pos[2]), (px, pz))
        if dist < best_dist:
            best, best_dist = pos, dist
    return best


def _wait_building_level(
    probe: ProbeSteps,
    detail: dict[str, Any],
    kind_label: str,
    level: int,
    budget_seconds: float,
    chest_pos: tuple[int, int, int],
    ctx: ProfileContext,
    artifact: Path,
    capture_name: str,
) -> bool:
    blueprint = next(blueprint for blueprint in _authored_blueprints() if blueprint["kind"] == kind_label.lower())
    deadline = time.monotonic() + budget_seconds
    last_check = 0.0
    while time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        if time.monotonic() - last_check >= 20.0:
            last_check = time.monotonic()
            lines = _recent_chat(_send(probe, "settlement building list"), 25)
            detail[f"build_poll_{kind_label}"] = lines
            for record in _listed_buildings(lines):
                if record["kind"] == kind_label.lower() and record["level"] >= level:
                    if not _observe_blueprint(probe, detail, record, blueprint, capture_name):
                        continue
                    detail["completed_storage"] = record
                    if ctx.windowed:
                        native_client.capture(probe.client, artifact, capture_name)
                    return True
    return False


def _listed_buildings(lines: list[str]) -> list[dict[str, Any]]:
    pattern = re.compile(
        r"#(\d+)\s+(\w+)\s+level\s+(\d+)\s+"
        r"\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]-\[(-?\d+),\s*(-?\d+),\s*(-?\d+)\]( natural)?"
    )
    records = []
    seen = set()
    for line in lines:
        for match in pattern.finditer(line):
            identity = int(match[1])
            if identity in seen:
                continue
            seen.add(identity)
            records.append({
                "id": identity, "kind": match[2], "level": int(match[3]),
                "min": [int(match[n]) for n in (4, 5, 6)],
                "max": [int(match[n]) for n in (7, 8, 9)],
                "natural": match[10] is not None,
            })
    return records


def _project_progress(observed: dict[str, Any]) -> dict[str, Any] | None:
    body = "\n".join(
        node.get("label", "") for node in observed.get("ui_nodes", [])
        if node.get("id", "").startswith("dialog/body/")
    )
    match = re.search(
        r"(?m)^(\w+) · level (\d+) · \d+%\nBlocks: (\d+)/(\d+) · [^\n]+\n"
        r"Site: (-?\d+), (-?\d+), (-?\d+)(?:\n|$)", body,
    )
    if match is None:
        return None
    return {
        "blueprint": match[1], "level": int(match[2]),
        "done": int(match[3]), "total": int(match[4]),
        "origin": [int(match[n]) for n in (5, 6, 7)],
    }


def _authored_blueprints() -> list[dict[str, Any]]:
    folder = runtime.REPO_ROOT / "crates/mc-core/assets/blueprints"
    return [json.loads(path.read_text()) for path in sorted(folder.glob("*.json"))]


def _observe_blueprint(
    probe: ProbeSteps, detail: dict[str, Any], record: dict[str, Any],
    blueprint: dict[str, Any], label: str,
) -> bool:
    level = blueprint["levels"][record["level"] - 1]
    layers = level["layers"]
    origin = record["min"]
    size = [len(layers[0][0]), len(layers), len(layers[0])]
    scan = probe.call("minecraft_scan_blocks", {
        "min_x": origin[0], "min_y": origin[1], "min_z": origin[2],
        "max_x": origin[0] + size[0] - 1,
        "max_y": origin[1] + size[1] - 1,
        "max_z": origin[2] + size[2] - 1,
    })
    blocks = {
        (cell["x"], cell["y"], cell["z"]): cell for cell in scan.get("blocks", [])
    }
    expected = 0
    mismatches = []
    # Builders use the settlement's available wood (construction.rs), so one
    # species replaces every authored oak_* block of this building.
    species = None
    for y, layer in enumerate(layers):
        for z, row in enumerate(layer):
            for x, symbol in enumerate(row):
                if symbol == ".":
                    continue
                expected += 1
                state = blueprint["palette"][symbol]
                block, _, properties = state.partition("[")
                block = block if ":" in block else f"minecraft:{block}"
                wanted = dict(part.split("=") for part in properties.rstrip("]").split(",")) if properties else {}
                mutable = ()
                if block in ("minecraft:wheat", "minecraft:carrots", "minecraft:potatoes", "minecraft:beetroots"):
                    mutable = ("age",)
                elif block == "minecraft:farmland":
                    mutable = ("moisture",)
                elif block == "minecraft:composter":
                    mutable = ("level",)
                elif block.endswith(("_door", "_trapdoor", "_fence_gate")):
                    mutable = ("open", "powered")
                at = (origin[0] + x, origin[1] + y, origin[2] + z)
                cell = blocks.get(at, {})
                got = _block_id(cell)
                if block.startswith("minecraft:oak_"):
                    suffix = "_" + block.removeprefix("minecraft:oak_")
                    if species is None and got.startswith("minecraft:") and got.endswith(suffix):
                        species = got.removeprefix("minecraft:").removesuffix(suffix)
                    block = f"minecraft:{species or 'oak'}{suffix}"
                actual_properties = cell.get("properties") or {}
                if got != block or any(
                    key not in mutable and str(actual_properties.get(key)) != value
                    for key, value in wanted.items()
                ):
                    mismatches.append({"at": at, "expected": state, "actual": cell})
    detail.setdefault("client_blocks", {})[label] = {
        "building_id": record["id"], "blueprint": blueprint["id"], "level": record["level"],
        "origin": origin, "expected_non_air": expected, "wood_species": species or "oak",
        "comparison": "all non-air blocks and stable authored properties; normal gameplay states allowed",
        "matched": expected - len(mismatches), "mismatch_count": len(mismatches),
        "mismatches": mismatches[:8],
    }
    return expected > 0 and not mismatches


def _wait_autonomous_progress(
    probe: ProbeSteps, detail: dict[str, Any], ctx: ProfileContext, artifact: Path,
    budget_seconds: float = 1500.0,
) -> tuple[bool, bool]:
    blueprints = {blueprint["kind"]: blueprint for blueprint in _authored_blueprints()}
    original = {record["id"] for record in _listed_buildings(detail["building_list_chat"])}
    storage = detail["completed_storage"]
    further = upgraded = False
    checked = set()
    deadline = time.monotonic() + budget_seconds
    while time.monotonic() < deadline:
        probe.call("minecraft_wait_ticks", {"ticks": 200})
        lines = _recent_chat(_send(probe, "settlement building list"), 25)
        records = _listed_buildings(lines)
        detail.setdefault("autonomous_building_history", []).append(records)
        for record in records:
            signature = (record["id"], record["level"])
            blueprint = blueprints.get(record["kind"])
            if record["id"] in original or blueprint is None or signature in checked:
                continue
            level = blueprint["levels"][record["level"] - 1]
            width, depth = len(level["layers"][0][0]), len(level["layers"][0])
            if record["max"][0] - record["min"][0] + 1 != width or record["max"][2] - record["min"][2] + 1 != depth:
                continue
            if record["id"] == storage["id"] and record["level"] == 1:
                continue
            label = f"autonomy-{blueprint['id']}-{record['id']}-l{record['level']}"
            if not _observe_blueprint(probe, detail, record, blueprint, label):
                continue
            checked.add(signature)
            if record["id"] != storage["id"]:
                further = True
                detail["further_building"] = record
            if record["level"] > 1:
                upgraded = True
                detail["autonomous_upgrade"] = record
            if ctx.windowed:
                native_client.capture(probe.client, artifact, label)
        if further and upgraded:
            return further, upgraded
    return further, upgraded


def _planner_status(probe: ProbeSteps, detail: dict[str, Any], chest_pos: tuple[int, int, int]) -> dict[str, Any]:
    """Non-gating read of what the autonomous planner is doing right now."""
    status: dict[str, Any] = {}
    try:
        status["projects_chat"] = _recent_chat(_send(probe, "settlement menu projects"), 25)
    except Exception as exc:
        status["projects_error"] = str(exc)[:300]
    try:
        slots = _open_chest(probe, chest_pos)
        status["chest_now"] = _chest_totals(slots)
    except Exception as exc:
        status["chest_error"] = str(exc)[:300]
    status["buildings_now"] = _recent_chat(_send(probe, "settlement building list"), 25)
    return status


def _wait_project_started(probe: ProbeSteps) -> dict[str, Any]:
    # Builders work only during the 2000..9000 day-time shift; an order placed
    # in the evening waits out the night (24000 ticks ≈ 1200 s at 20 TPS).
    deadline = time.monotonic() + 1300.0
    while time.monotonic() < deadline:
        state = _project_status(probe)
        project = _project_progress(state)
        if project is not None and 0 < project["done"] < project["total"]:
            return project
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    raise RuntimeError("no genuinely in-progress project before restart")


def _wait_project_resumed(
    probe: ProbeSteps, detail: dict[str, Any], before: dict[str, Any],
) -> bool:
    info = _recent_chat(_send(probe, f"settlement info {detail['settlement_name']}"), 20)
    owner_pattern = re.compile(r"\bowner=(\S+)")
    old_owner = next((owner_pattern.search(line)[1] for line in detail["final_info_chat"] if owner_pattern.search(line)), None)
    new_owner = next((owner_pattern.search(line)[1] for line in info if owner_pattern.search(line)), None)
    detail["restart_info_chat"] = info
    probe.check("claimed_owner_survives_restart", old_owner is not None and old_owner != "autonomous" and new_owner == old_owner)
    blueprint = next(blueprint for blueprint in _authored_blueprints() if blueprint["id"] == before["blueprint"])
    deadline = time.monotonic() + 1300.0
    while time.monotonic() < deadline:
        state = _project_status(probe)
        after = _project_progress(state)
        detail["restart_after"] = after
        if after is not None and all(after[key] == before[key] for key in ("blueprint", "level", "origin", "total")):
            probe.check("restart_never_rewinds_project", after["done"] >= before["done"])
            if after["done"] > before["done"]:
                return True
        lines = _recent_chat(_send(probe, "settlement building list"), 25)
        for record in _listed_buildings(lines):
            if record["kind"] == blueprint["kind"] and record["level"] >= before["level"] and record["min"] == before["origin"]:
                if _observe_blueprint(probe, detail, record, blueprint, "autonomy-restart-completed"):
                    detail["restart_after"] = {"completed_building": record}
                    return True
        probe.call("minecraft_wait_ticks", {"ticks": 100})
    return False
