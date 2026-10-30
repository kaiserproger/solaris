"""Step-screenshot human playthrough on the windowed native client.

Runs INSIDE the shared survival world driver: Main wires it as the
`--windowed` capture leg of `survival --client native` (no new profile, no
`__main__` gate change needed). After the ordinary survival scenario passes,
walks the human path with real captures per step: HUD, inventory open,
chat echo, death + Death-screen Respawn click (no fallback: a failed click
fails the gate), F3 debug overlay content, Pause -> Options change applied,
block-break crack mid-mining, final frame.

The Title/connect leg needs a disconnected client with the Title screen up,
which the shared survival driver cannot provide (it always passes --server
and asserts in_play first). Title typing + Connect-button identity is
therefore covered by a dedicated harness entry (`title_connect`, run by Main
as a separate windowed process without --server), while the unit seam
(`title_connect_and_quit_emit_commands`) pins validation fail-closed.

Death uses an explicitly armed privileged `kill` fixture (precedent:
native-lifecycle's privileged death fixture); every privileged grant is
recorded in fixture_commands by ProbeSteps.
"""
from __future__ import annotations

import math

from pathlib import Path
from typing import Any

from . import native_client

TITLE_CLASS = "net.minecraft.client.gui.screens.TitleScreen"
PAUSE_CLASS = "net.minecraft.client.gui.screens.PauseScreen"
OPTIONS_CLASS = "net.minecraft.client.gui.screens.options.OptionsScreen"


def _shot(client: Any, artifact: Path, name: str) -> dict[str, Any]:
    return native_client.capture(client, artifact, name)


def _button(probe: Any, label: str) -> None:
    screen = probe.observe()["screen"]
    probe.call("minecraft_click_screen_button", {
        "expected_screen_class": screen["class"], "expected_title": screen["title"],
        "button_label": label,
    })


def _physical_slot_click(owned: dict[str, Any], bounds: list[float]) -> None:
    """XTest input is confined to this harness process's focused Xvfb window."""
    import ctypes as c
    import ctypes.util
    x = c.CDLL(ctypes.util.find_library("X11"))
    xt = c.CDLL(ctypes.util.find_library("Xtst"))
    x.XOpenDisplay.restype = c.c_void_p
    x.XOpenDisplay.argtypes = [c.c_char_p]
    x.XWarpPointer.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_int, c.c_int,
                              c.c_uint, c.c_uint, c.c_int, c.c_int]
    x.XSync.argtypes = [c.c_void_p, c.c_int]
    x.XCloseDisplay.argtypes = [c.c_void_p]
    xt.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
    display = x.XOpenDisplay(owned["display"].encode())
    if not display:
        raise RuntimeError("cannot open owned Xvfb display for inventory click")
    try:
        px, py, width, height = bounds
        x.XWarpPointer(display, 0, owned["window_id"], 0, 0, 0, 0,
                       round(px + width / 2), round(py + height / 2))
        if not xt.XTestFakeButtonEvent(display, 1, 1, 0) or not xt.XTestFakeButtonEvent(display, 1, 0, 0):
            raise RuntimeError("owned Xvfb display refused inventory click")
        x.XSync(display, 0)
    finally:
        x.XCloseDisplay(display)


def run(client: Any, artifact: Path, timeout: float, *, owned_window: dict[str, Any]) -> dict[str, Any]:
    from .probe_steps import ProbeSteps

    steps: list[dict[str, Any]] = []

    def record(step: str, **kw: Any) -> None:
        entry: dict[str, Any] = {"step": step}
        entry.update(kw)
        steps.append(entry)

    with ProbeSteps(client, artifact / "playthrough", timeout,
                    "native-human-playthrough") as probe:
        probe.wait(lambda s: s["player"]["on_ground"], "grounded")
        before = probe.observe()
        record("hud", health=before["player"].get("health"),
               food=before["player"].get("food"))
        probe.check("hud_max_health_received", before["player"].get("max_health") == 20.0)
        hud_ids = [node.get("id", "") for node in before.get("ui_nodes", [])]
        probe.check("hud_hearts_present", any(key.startswith("hud/heart/") for key in hud_ids))
        probe.check("hud_zero_armor_hidden", not any(key.startswith("hud/armor/") for key in hud_ids))
        hud_shot = _shot(client, artifact, "step-hud")
        # F3 overlay via the ordinary toggle tool (same flag the physical F3
        # key flips; key.keyboard.f3 is NOT a gameplay key). Assert content
        # twice: the tool's immediate session-part lines AND the live
        # debug/line/* semantic nodes. The unit seam
        # (f3_debug_lines_render_above_hidden_hud) pins line layout.
        f3_toggle = probe.call("minecraft_toggle_debug_overlay", {
            "enabled": True,
        })
        probe.check("f3_toggle_reports_lines", len(f3_toggle.get("lines", [])) >= 4)
        f3 = probe.wait(lambda s: s.get("debug_overlay") is True,
                        "f3_overlay_on")
        overlay_nodes = [n for n in f3.get("ui_nodes", [])
                         if n.get("id", "").startswith("debug/line/")]
        probe.check("f3_overlay_lines", len(overlay_nodes) >= 4)
        record("f3", lines=len(overlay_nodes),
               sample=[n.get("label", "") for n in overlay_nodes[:3]])
        f3_shot = _shot(client, artifact, "step-f3")
        probe.call("minecraft_toggle_debug_overlay", {"enabled": False})
        probe.wait(lambda s: s.get("debug_overlay") is not True,
                   "f3_overlay_off")
        probe.call("minecraft_open_inventory", {})
        inv = probe.wait(lambda s: s["screen"]["open"], "inventory_open")
        record("inventory", screen_class=inv["screen"].get("class"))
        inv_shot = _shot(client, artifact, "step-inventory")
        probe.call("minecraft_close_screen", {})
        probe.wait(lambda s: not s["screen"]["open"], "inventory_closed")
        marker = "playthrough hello"
        probe.call("minecraft_send_chat", {"message": marker, "command": False})
        probe.wait(lambda s: any(marker in line for line in s["recent_chat"]),
                   "chat_echo")
        record("chat", echo=True)
        chat_shot = _shot(client, artifact, "step-chat")
        # Pause -> Options via the ordinary Game->Pause tool (same transition
        # the physical Escape key drives; key.keyboard.escape does NOT work
        # here: press_inputs holds gameplay keys, and Escape only routes on
        # dialog/title screens). Click Options, cycle FOV once via its row
        # button, assert the applied value changed in observation, screenshot,
        # then Done + Back to game.
        probe.call("minecraft_open_pause_menu", {})
        pause = probe.wait(
            lambda s: s["screen"].get("class") == PAUSE_CLASS, "pause_open")
        record("pause", screen_class=pause["screen"].get("class"))
        pause_shot = _shot(client, artifact, "step-pause")
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": PAUSE_CLASS,
            "expected_title": pause["screen"].get("title", ""),
            "button_label": "Options",
        })
        options = probe.wait(
            lambda s: s["screen"].get("class") == OPTIONS_CLASS,
            "options_open")
        # UiState.settings starts empty (defaults live on the session +
        # UiState consts, shown as "not configured" until first press), so a
        # missing key counts as the default FOV for the change assertion.
        fov_before = options.get("client_settings", {}).get("fov", 70.0)
        fov_label_before = next(
            (n.get("label", "") for n in options.get("ui_nodes", [])
             if n.get("id") == "settings/fov"), "")
        # With no saved settings the first press only persists the default
        # (null -> 70.0, no visible change); with saved settings it steps.
        # Press until the value moves (at most twice), then require one step.
        for _ in range(2):
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": OPTIONS_CLASS,
                "expected_title": options["screen"].get("title", ""),
                "button_label": fov_label_before,
            })
            options = probe.wait(
                lambda s: s.get("client_settings", {}).get("fov") is not None,
                "options_fov_persisted")
            fov_label_before = next(
                (n.get("label", "") for n in options.get("ui_nodes", [])
                 if n.get("id") == "settings/fov"), fov_label_before)
            if options["client_settings"]["fov"] != fov_before:
                break
        changed = options
        probe.check("options_fov_applied",
                    changed.get("client_settings", {}).get("fov") == fov_before + 5.0)
        record("options", fov_before=fov_before,
               fov_after=changed.get("client_settings", {}).get("fov"))
        # GUI scale lives on the Interface tab of the tabbed Options screen.
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": OPTIONS_CLASS,
            "expected_title": changed["screen"].get("title", ""),
            "button_label": "Interface",
        })
        changed = probe.wait(
            lambda s: any(n.get("id") == "settings/ui_scale" for n in s.get("ui_nodes", [])),
            "options_interface_tab")
        scale_node = next(n for n in changed.get("ui_nodes", []) if n.get("id") == "settings/ui_scale")
        scale_before = changed.get("client_settings", {}).get("ui_scale", 0.0)
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": OPTIONS_CLASS,
            "expected_title": changed["screen"].get("title", ""),
            "button_label": scale_node["label"],
        })
        scaled = probe.observe()
        scaled_node = next(n for n in scaled.get("ui_nodes", []) if n.get("id") == "settings/ui_scale")
        probe.check("gui_scale_value_applied", scaled["client_settings"]["ui_scale"] != scale_before)
        probe.check("gui_scale_layout_applied", scaled_node.get("bounds") != scale_node.get("bounds"))
        _shot(client, artifact, "step-options-scaled")
        for _ in range(5):
            if scaled["client_settings"]["ui_scale"] == 2.0:
                break
            _button(probe, scaled_node["label"])
            scaled = probe.observe()
            scaled_node = next(n for n in scaled["ui_nodes"] if n["id"] == "settings/ui_scale")
        probe.check("gui_scale_2_selected", scaled["client_settings"]["ui_scale"] == 2.0)
        _button(probe, "Done")
        _button(probe, "Back to game")
        # Explicit UI fixture: the ordinary scenario can leave the inventory
        # empty. A two-digit stack proves both physical hit testing and counts.
        probe.fixture("give @s minecraft:cobblestone 20")
        probe.call("minecraft_wait_for_inventory", {
            "item_id": "minecraft:cobblestone", "count": 20,
            "timeout_seconds": probe.remaining(),
        })
        probe.call("minecraft_open_inventory", {})
        inventory = probe.wait(lambda s: s["screen"]["open"], "physical_inventory_open")
        slots = {row["slot"]: row for row in inventory["container"]["slots"]}
        source = next(row for row in slots.values() if 9 <= row["slot"] <= 44 and (row.get("count") or 0) > 0)
        target = next(index for index in range(9, 45) if index not in slots)
        nodes = {node["id"]: node for node in inventory["ui_nodes"]}
        _physical_slot_click(owned_window, nodes[f"slot/{source['slot']}"]["bounds"])
        picked = probe.wait(
            lambda s: s["container"]["carried"].get("item_id") == source["item_id"]
            and s["container"]["carried"].get("count") == source["count"],
            "physical_slot_pickup_server_confirmed")
        probe.check("physical_pickup_server_observed", picked["container"]["carried"]["origin"] == "server_observed")
        _physical_slot_click(owned_window, nodes[f"slot/{target}"]["bounds"])
        moved = probe.wait(
            lambda s: s["container"]["carried"].get("count") == 0
            and any(row["slot"] == target and row.get("item_id") == source["item_id"]
                    and row.get("count") == source["count"] for row in s["container"]["slots"]),
            "physical_slot_move_server_confirmed")
        probe.check("physical_slot_move_at_gui_scale_2", moved["client_settings"]["ui_scale"] == 2.0)
        record("physical_inventory", mechanism="XTestFakeButtonEvent", gui_scale=2,
               fixture="privileged cobblestone stack (not earned)",
               owned_window=owned_window, source=source, target_slot=target,
               source_bounds=nodes[f"slot/{source['slot']}"]["bounds"],
               target_bounds=nodes[f"slot/{target}"]["bounds"],
               server_carried=picked["container"]["carried"],
               server_target=next(row for row in moved["container"]["slots"] if row["slot"] == target))
        _shot(client, artifact, "step-inventory-scale2-physical")
        probe.call("minecraft_close_screen", {})
        probe.call("minecraft_open_pause_menu", {})
        _button(probe, "Options")
        scaled = probe.observe()
        scaled_node = next(n for n in scaled["ui_nodes"] if n["id"] == "settings/ui_scale")
        # Restore the previous scale through the same ordinary option button.
        for _ in range(4):
            if scaled["client_settings"]["ui_scale"] == scale_before:
                break
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": OPTIONS_CLASS,
                "expected_title": scaled["screen"].get("title", ""),
                "button_label": scaled_node["label"],
            })
            scaled = probe.observe()
            scaled_node = next(n for n in scaled.get("ui_nodes", []) if n.get("id") == "settings/ui_scale")
        probe.check("gui_scale_restored", scaled["client_settings"]["ui_scale"] == scale_before)
        changed = scaled
        options_shot = _shot(client, artifact, "step-options")
        done_label = next(
            (n.get("label", "") for n in changed.get("ui_nodes", [])
             if n.get("id") == "settings/done"), "Done")
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": OPTIONS_CLASS,
            "expected_title": changed["screen"].get("title", ""),
            "button_label": done_label,
        })
        back = probe.wait(
            lambda s: s["screen"].get("class") == PAUSE_CLASS, "options_done")
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": PAUSE_CLASS,
            "expected_title": back["screen"].get("title", ""),
            "button_label": "Back to game",
        })
        probe.wait(lambda s: not s["screen"]["open"], "pause_closed")
        # Block-break crack: pick a loaded solid block in reach, hold mining
        # input WITHOUT breaking it (mine_until_crack seam), assert the
        # observed crack stage, screenshot the overlay mid-break.
        p = probe.observe()["player"]
        px, py, pz = (math.floor(p[axis]) for axis in ("x", "y", "z"))
        # A full 4096-cell scan used to exceed the MCP response limit.
        scan = {"min_x": px - 8, "max_x": px + 7,
                "min_y": py - 8, "max_y": py + 7,
                "min_z": pz - 8, "max_z": pz + 7, "max_blocks": 4096}
        offset = pages = scanned = 0
        while True:
            page = probe.call("minecraft_scan_blocks", {**scan, "offset": offset})
            probe.check("scan_page_count", page["count"] == len(page["blocks"]))
            scanned += page["count"]
            pages += 1
            next_offset = page.get("next_offset")
            if next_offset is None:
                break
            probe.check("scan_page_advances", next_offset > offset)
            offset = next_offset
        probe.check("large_scan_all_cells_returned", scanned == 4096)
        probe.check("large_scan_paginated", pages > 1)
        record("bounded_scan", cells=scanned, pages=pages)
        blocks = probe.call("minecraft_scan_blocks", {
            "min_x": px - 3, "max_x": px + 3,
            "min_y": py - 2, "max_y": py + 2,
            "min_z": pz - 3, "max_z": pz + 3, "max_blocks": 245,
        })["blocks"]
        air = {"minecraft:air", "minecraft:cave_air", "minecraft:void_air"}
        indexed = {(b["x"], b["y"], b["z"]): b for b in blocks}
        candidates = sorted(
            (b for b in blocks
             if b["block_id"] not in air
             and indexed.get((b["x"], b["y"] + 1, b["z"]), {}).get("block_id") in air
             and (b["x"] + 0.5 - p["x"]) ** 2 + (b["z"] + 0.5 - p["z"]) ** 2 <= 2.5 ** 2
             and (b["x"] + 0.5 - p["x"]) ** 2 + (b["y"] + 1 - (p["y"] + 1.62)) ** 2
                 + (b["z"] + 0.5 - p["z"]) ** 2 <= 3.5 ** 2),
            key=lambda b: ((b["x"] + 0.5 - p["x"]) ** 2
                           + (b["y"] + 1 - (p["y"] + 1.62)) ** 2
                           + (b["z"] + 0.5 - p["z"]) ** 2))
        target = None
        for candidate in candidates:
            position = {axis: candidate[axis] for axis in ("x", "y", "z")}
            probe.call("minecraft_look_at_block", {**position, "face": "up"})
            hit = probe.observe().get("target") or {}
            if hit.get("type") == "block" and all(hit.get(axis) == value for axis, value in position.items()):
                target = candidate
                break
        probe.check("crack_target_found", target is not None)
        crack = probe.call("minecraft_mine_until_crack", {
            "x": target["x"], "y": target["y"], "z": target["z"],
            "face": "up", "ticks": 12,
            "timeout_seconds": probe.remaining(),
        })
        probe.check("crack_stage_seen",
                    crack.get("status") == "cracking"
                    and 0 <= int(crack.get("stage", -1)) <= 9)
        record("crack", position=[target["x"], target["y"], target["z"]],
               stage=crack.get("stage"))
        crack_shot = _shot(client, artifact, "step-crack")
        # Death: explicitly armed privileged kill fixture (precedent:
        # native-lifecycle's privileged death fixture), ordinary Death-screen
        # Respawn click with NO protocol fallback (a failed click fails the
        # gate). A tower walk-off was tried first and exceeds the bounded
        # navigation range (48 horiz / 8 vert), so it is not used.
        probe.fixture("kill")
        probe.call("minecraft_wait_for_health_below", {
            "health": 0.001, "timeout_seconds": probe.remaining(),
        })
        get_hp = lambda s: (s["player"]["health"]
                            if s["player"]["health"] is not None else 99)
        dead = probe.wait(lambda s: get_hp(s) <= 0, "death_seen")
        record("death", achieved=True, fixture="privileged kill (not earned)")
        death_shot = _shot(client, artifact, "step-death")
        # Respawn only through the Death screen's Respawn button: a failed
        # click fails the playthrough (no protocol-level fallback).
        screen = probe.observe()["screen"]
        probe.call("minecraft_click_screen_button", {
            "expected_screen_class": screen["class"],
            "expected_title": screen["title"],
            "button_label": "Respawn",
        })
        # The actual UI request must restore a loaded Play world and live health;
        # transient respawn rebuilding is awaited without a protocol fallback.
        alive = probe.wait(lambda s: get_hp(s) > 0, "respawned", wait_for_play=True)
        record("respawn", health=alive["player"].get("health"))
        respawn_shot = _shot(client, artifact, "step-respawn")
        final_shot = _shot(client, artifact, "step-final")
    return {"scenario": "native-human-playthrough", "passed": True, "steps": steps,
            "evidence_domain": "windowed_native_screenshots",
            "captures": {
                "hud": hud_shot.get("retained_path"),
                "f3": f3_shot.get("retained_path"),
                "inventory": inv_shot.get("retained_path"),
                "chat": chat_shot.get("retained_path"),
                "pause": pause_shot.get("retained_path"),
                "options": options_shot.get("retained_path"),
                "crack": crack_shot.get("retained_path"),
                "death": (death_shot or {}).get("retained_path"),
                "respawn": (respawn_shot or {}).get("retained_path"),
                "final": final_shot.get("retained_path"),
            }}
