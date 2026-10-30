"""No-`--server` Title/connect windowed proof for the native client.

Starts the server (same seed-81 survival config as gameplay), launches the
native client WITHOUT `--server`/`--username` so it opens the Title screen,
then through the ordinary UI: open Multiplayer → Direct Connect, type the
live server address and username, press Join Server, assert in-game state,
then disconnect back to Title. The remembered values are also used to rejoin.

Failure anywhere fails the run. No privileged fixtures; typing + Connect are
the ordinary human path.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path
from typing import Any

from .. import runtime
from ..mcp import McpClient
from ..profiles import ProfileContext
from . import native_client
from .gameplay import peak_rss_kib
from .probe_steps import ProbeSteps

TITLE_CLASS = "net.minecraft.client.gui.screens.TitleScreen"
MULTIPLAYER_CLASS = "net.minecraft.client.gui.screens.multiplayer.JoinMultiplayerScreen"
DIRECT_CLASS = "net.minecraft.client.gui.screens.DirectJoinServerScreen"
USERNAME = "TitleProof"


class _TitleProbeSteps(ProbeSteps):
    """Title observations are deliberately pre-play; opt-in waits stay strict."""

    def observe(self, *, wait_for_play: bool = False) -> dict[str, Any]:
        if wait_for_play:
            return super().observe(wait_for_play=True)
        return self.call("minecraft_observe")


def _open_direct_connect(probe: _TitleProbeSteps) -> dict[str, Any]:
    title = probe.call("minecraft_observe")
    probe.check("title_screen_open", title["screen"].get("class") == TITLE_CLASS)
    probe.call("minecraft_click_screen_button", {
        "expected_screen_class": TITLE_CLASS,
        "expected_title": title["screen"]["title"],
        "button_label": "Multiplayer",
    })
    multiplayer = probe.wait(lambda s: s["screen"].get("class") == MULTIPLAYER_CLASS,
                             "multiplayer_screen_open")
    probe.call("minecraft_click_screen_button", {
        "expected_screen_class": MULTIPLAYER_CLASS,
        "expected_title": multiplayer["screen"]["title"],
        "button_label": "Direct Connect",
    })
    return probe.wait(lambda s: s["screen"].get("class") == DIRECT_CLASS,
                      "direct_connect_screen_open")


def run(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.client != "native":
        raise ValueError("title-connect requires --client native")
    if ctx.extra_args or ctx.platform is not None:
        raise ValueError("title-connect accepts only --client and --timeout-seconds")
    timeout = ctx.timeout_seconds if ctx.timeout_seconds is not None else 120.0
    if not 0 < timeout <= 120:
        raise ValueError("title-connect --timeout-seconds must be in (0, 120]")
    artifact = ctx.artifact_dir
    override = __import__("os").environ.get("SOLARIS_SERVER_BINARY")
    if override:
        binary = Path(override).expanduser().resolve()
    else:
        raise RuntimeError("title-connect requires SOLARIS_SERVER_BINARY (reuse the survival-built server)")
    if not binary.is_file():
        raise RuntimeError(f"missing executable server binary: {binary}")
    server_port = runtime.reserve_port()
    address = f"127.0.0.1:{server_port}"
    config = artifact / "title.toml"
    source = (runtime.REPO_ROOT / "playable.toml").read_text()
    replacements = (
        ("port = 25565", f"port = {server_port}"),
        ('world_dir = ".analysis/test-world-v12"',
         f'world_dir = "{(artifact / "world").relative_to(runtime.REPO_ROOT).as_posix()}"'),
        ('directory = "plugins"',
         f"directory = {json.dumps(str(artifact / 'plugins'))}"),
        ("seed = 0", "seed = 81"),
    )
    for old_tok, new_tok in replacements:
        if source.count(old_tok) != 1:
            raise RuntimeError(f"expected one title config token: {old_tok}")
        source = source.replace(old_tok, new_tok, 1)
    config.write_text(source)
    (artifact / "plugins").mkdir(parents=True, exist_ok=True)
    evidence = ctx.client_evidence
    with binary.open("rb") as server_executable:
        server_hash = hashlib.file_digest(server_executable, "sha256").hexdigest()
    evidence.update(server_address=address, server_binary_path=str(binary),
                    server_binary_sha256=server_hash,
                    world_dir=str(artifact / "world"), seed=81)
    server = client_process = xvfb = mcp = None
    server_log = client_log = None
    try:
        server_log = (artifact / "server.log").open("wb")
        import subprocess as _sp
        server = _sp.Popen([str(binary), "--config", str(config), "--no-console"],
                           cwd=runtime.REPO_ROOT, stdout=server_log,
                           stderr=_sp.STDOUT, start_new_session=True)
        ctx.logs.append(str(artifact / "server.log"))
        runtime.wait_port(server_port, timeout, server, log_path=artifact / "server.log")
        display = None
        if ctx.windowed:
            xvfb, display, _ = runtime.start_xvfb(artifact)
            ctx.logs.append(str(artifact / "xvfb.log"))
        # Title-mode launch: no --server/--username → Title screen with prefill.
        client_process, client_log, mcp = native_client.start(
            ctx, native_client.resolve_binary(ctx), None, None, timeout,
            display=display)
        mcp.request_timeout_seconds = timeout + 10
        with _TitleProbeSteps(mcp, artifact / "title-connect", timeout,
                              "native-title-connect") as probe:
            direct = _open_direct_connect(probe)
            probe.call("minecraft_set_screen_input", {
                "expected_screen_class": DIRECT_CLASS,
                "expected_title": direct["screen"]["title"],
                "input_label": "Server address",
                "value": address,
            })
            probe.call("minecraft_set_screen_input", {
                "expected_screen_class": DIRECT_CLASS,
                "expected_title": direct["screen"]["title"],
                "input_label": "Offline username",
                "value": USERNAME,
            })
            native_client.capture(mcp, artifact, "step-title-filled")
            # Title observation exposes the same backing fields, unlike Direct Connect.
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": DIRECT_CLASS,
                "expected_title": direct["screen"]["title"],
                "button_label": "Cancel",
            })
            multiplayer = probe.wait(lambda s: s["screen"].get("class") == MULTIPLAYER_CLASS,
                                     "multiplayer_after_input")
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": MULTIPLAYER_CLASS,
                "expected_title": multiplayer["screen"]["title"],
                "button_label": "Back",
            })
            filled = probe.wait(lambda s: s["screen"].get("class") == TITLE_CLASS,
                                "title_after_input")
            probe.check("title_server_typed", filled.get("title_server") == address)
            probe.check("title_username_typed", filled.get("title_username") == USERNAME)
            direct = _open_direct_connect(probe)
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": DIRECT_CLASS,
                "expected_title": direct["screen"]["title"],
                "button_label": "Join Server",
            })
            play = probe.call("minecraft_wait_for_play", {
                "timeout_seconds": probe.remaining(),
            })
            probe.check("connect_entered_play", play.get("in_play") is True)
            # In Play now: wait WITHOUT the strict in_play assert (the
            # disconnect leg below returns to Title, which is also not in_play;
            # both directions use raw calls + explicit checks).
            ingame = probe.wait(lambda s: s["player"].get("health") is not None,
                                "title_connect_health")
            probe.check("title_connect_username",
                        ingame["player"].get("entity_uuid") is not None)
            native_client.capture(mcp, artifact, "step-title-ingame")
            # Disconnect → back to Title with the reason shown.
            probe.call("minecraft_disconnect", {})
            back = probe.wait(
                lambda s: s["screen"].get("class") == TITLE_CLASS,
                "disconnect_back_to_title")
            reason = back.get("title_error") or ""
            probe.check("title_disconnect_reason_shown", len(reason) > 0)
            probe.check("title_remembers_server", back.get("title_server") == address)
            probe.check("title_remembers_username", back.get("title_username") == USERNAME)
            native_client.capture(mcp, artifact, "step-title-disconnected")
            # Rejoin without retyping: the remembered values drive a real connection.
            direct = _open_direct_connect(probe)
            probe.call("minecraft_click_screen_button", {
                "expected_screen_class": DIRECT_CLASS,
                "expected_title": direct["screen"]["title"],
                "button_label": "Join Server",
            })
            rejoined = probe.call("minecraft_wait_for_play", {"timeout_seconds": probe.remaining()})
            probe.check("remembered_connection_rejoins", rejoined.get("in_play") is True)
            native_client.capture(mcp, artifact, "step-title-rejoined")
            probe.call("minecraft_disconnect", {})
            probe.report.update(title_error=reason, address=address, username=USERNAME)
        if ctx.windowed:
            evidence["capture"] = native_client.capture(mcp, artifact)
        if client_process is not None:
            rss = peak_rss_kib(client_process.pid)
            if rss is not None:
                evidence["peak_client_rss_kib"] = rss
        return {"scenario": probe.report}
    finally:
        if mcp is not None:
            try:
                mcp.close()
            except Exception:
                pass
        runtime.stop_process(client_process)
        runtime.stop_process(server, interrupt=True)
        runtime.stop_process(xvfb)
