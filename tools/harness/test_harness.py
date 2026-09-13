#!/usr/bin/env python3
"""Fail-closed receipts and real process lifecycle checks for the harness.

Run with: python3 -m unittest tools.harness.test_harness
"""

from __future__ import annotations

import binascii
import json
import os
import select
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

from . import __main__ as cli
from . import profiles, runtime
from .backends import driver


class ClientSettingsTests(unittest.TestCase):
    def test_invalid_username_rejected(self) -> None:
        with mock.patch.dict(os.environ, {
            "SOLARIS_CLIENT_MCP_TOKEN": "t",
            "SOLARIS_CLIENT_MCP_USERNAME": "bad-name!",
        }):
            with self.assertRaises(RuntimeError):
                cli.resolve_client_settings(None)

    def test_missing_token_rejected(self) -> None:
        env = {key: value for key, value in os.environ.items()
               if key != "SOLARIS_CLIENT_MCP_TOKEN"}
        with mock.patch.dict(os.environ, env, clear=True):
            with self.assertRaises(RuntimeError):
                cli.resolve_client_settings(None)

    def test_unknown_platform_rejected(self) -> None:
        with mock.patch.dict(os.environ, {"SOLARIS_CLIENT_MCP_TOKEN": "t"}):
            with self.assertRaises(ValueError):
                cli.resolve_client_settings("quilt")


class ReceiptContractTests(unittest.TestCase):
    def test_unknown_profile_never_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["no-such-profile"])
            self.assertEqual(code, 2)
            receipts = list(Path(tmp).glob("*/result.json"))
            self.assertEqual(len(receipts), 1)
            payload = json.loads(receipts[0].read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertIn("unknown profile", payload["error"])

    def test_failed_command_keeps_prior_success_and_failure_evidence(self) -> None:
        commands = [
            [sys.executable, "-c", "print('first step completed')"],
            [sys.executable, "-c", "print('second step failed'); raise SystemExit(7)"],
        ]

        def run_commands(ctx):
            for index, command in enumerate(commands):
                profiles._run_command(command, ctx, f"step-{index}.log")
            return {}

        spec = {"scope": "harness", "run": run_commands}
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, {"failing-commands": spec}):
                    code = cli.cmd_run(["failing-commands"])
            self.assertEqual(code, 1)
            receipt = next(Path(tmp).glob("*/result.json"))
            payload = json.loads(receipt.read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(payload["commands"], commands)
            self.assertEqual(payload["exits"], [0, 7])
            self.assertEqual(
                [Path(path).read_text().strip() for path in payload["logs"]],
                ["first step completed", "second step failed"],
            )

    def test_missing_executable_records_attempt_without_a_fake_exit_code(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            command = [str(Path(tmp) / "missing-executable")]
            with self.assertRaises(FileNotFoundError):
                profiles._run_command(command, ctx, "missing.log")
            self.assertEqual(ctx.commands, [command])
            self.assertEqual(ctx.exits, [None])
            self.assertTrue(Path(ctx.logs[0]).is_file())


class ReadinessCleanupTests(unittest.TestCase):
    def test_wait_port_observes_real_server_startup(self) -> None:
        port = runtime.reserve_port()
        source = (
            "import signal,socket,sys; "
            "listener=socket.socket(); "
            "listener.bind(('127.0.0.1',int(sys.argv[1]))); "
            "listener.listen(); print('ready',flush=True); signal.pause()"
        )
        with tempfile.TemporaryDirectory() as tmp:
            log_path = Path(tmp) / "server.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [sys.executable, "-c", source, str(port)],
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                )
                try:
                    runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertTrue(runtime.port_open(port))
                    self.assertIsNone(process.poll())
                finally:
                    runtime.stop_process(process)
            self.assertFalse(runtime.port_open(port))

    def test_wait_port_reports_early_exit(self) -> None:
        port = runtime.reserve_port()
        with tempfile.TemporaryDirectory() as tmp:
            log_path = Path(tmp) / "early.log"
            with log_path.open("wb") as log:
                process = subprocess.Popen(
                    [sys.executable, "-c", "print('startup failed',flush=True); raise SystemExit(3)"],
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                )
                try:
                    with self.assertRaises(RuntimeError) as raised:
                        runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertIn("exited", str(raised.exception))
                    self.assertEqual(process.wait(timeout=10), 3)
                finally:
                    runtime.stop_process(process)

    def test_stop_process_terminates_descendants(self) -> None:
        source = (
            "import signal,subprocess,sys; "
            "child=subprocess.Popen([sys.executable,'-c','import signal; signal.pause()']); "
            "print(child.pid,flush=True); signal.pause()"
        )
        process = subprocess.Popen(
            [sys.executable, "-c", source], stdout=subprocess.PIPE,
            text=True, start_new_session=True,
        )
        child_pid = None
        child_fd = None
        try:
            ready, _, _ = select.select([process.stdout], [], [], 10.0)
            self.assertTrue(ready, "fixture did not publish its child PID")
            child_pid = int(process.stdout.readline())
            child_fd = os.pidfd_open(child_pid)
            runtime.stop_process(process)
            self.assertLess(process.wait(timeout=10), 0)
            exited, _, _ = select.select([child_fd], [], [], 10.0)
            self.assertTrue(exited, "descendant survived process-group cleanup")
        finally:
            runtime.stop_process(process)
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            if child_fd is not None:
                os.close(child_fd)
            process.stdout.close()


def _png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", binascii.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


# The screenshot check only validates PNG framing, so an empty IDAT is enough.
_MINIMAL_PNG = (
    b"\x89PNG\r\n\x1a\n"
    + _png_chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
    + _png_chunk(b"IDAT", b"")
    + _png_chunk(b"IEND", b"")
)


class _SettlementChainClient:
    """Scripted real-client stand-in for exactly the RPCs the chain issues."""

    def __init__(
        self,
        *,
        drop: set[str] | None = None,
        seed_chat: list[str] | None = None,
    ) -> None:
        self.drop = set(drop or ())
        self.chat = list(seed_chat or ())
        self.commands: list[str] = []
        self.version = 1
        self.houses = 0
        self.population = 0
        self.player = {"x": 10.0, "y": 250.0, "z": -4.0}

    def _observation(self) -> dict[str, Any]:
        return {
            "in_play": True,
            "current_screen": "",
            "state_version": self.version,
            "recent_chat": list(self.chat),
            "player": dict(self.player),
        }

    def _ack(self, message: str) -> str:
        words = message.split()
        if message == "gamemode creative":
            return "Set game mode to Creative"
        if words[0] == "give":
            return f"Gave {words[2]} of {words[1]}"
        if words[0] == "tp":
            self.player = {
                "x": float(words[1]),
                "y": float(words[2]),
                "z": float(words[3]),
            }
            return "Teleported to " + " ".join(words[1:])
        if words[0] == "settlement":
            action = words[1]
            if action == "create":
                return f"Founded {words[2]} (small hamlet)."
            if action == "site":
                return "site_aaa village origin 10,72,-4 size 11,7,9 buildings=3"
            if action == "adopt":
                return f"Adopted {words[3]} (village)"
            if action == "survey":
                return "Survey settlement: 1 plot chunks=loaded"
            if action == "project":
                return (
                    "house_small projected (solaris:house_small, 4 stages). "
                    f"Fund it: /settlement fund {words[2]} house_small"
                )
            if action == "fund":
                return f"Reserved real materials for {words[3]} (24 units)"
            if action == "build":
                self.houses = 1
                return f"{words[3]} committed (solaris:house_small) at revision 20."
            if action == "info":
                return (
                    f"regsville | small hamlet village tier=hamlet pop={self.population} "
                    f"houses={self.houses} jobs=0 food=0 money=0 specs=none"
                )
            if action == "populate":
                self.population = 1
                return "settled in regsville (Miller)"
            if action == "residents":
                return "Miller family=Miller job=none"
        raise AssertionError(f"unexpected settlement command {message!r}")

    def call(
        self, command: str, payload: dict[str, Any], timeout_seconds: float
    ) -> dict[str, Any]:
        if command == "ping":
            return {}
        if command in {"observe", "state", "wait_play"}:
            return self._observation()
        if command == "wait_state_change":
            self.version += 1
            return self._observation()
        if command == "read_block":
            return {"is_air": False}
        if command == "scan_blocks":
            return {
                "blocks": [
                    {"block_id": block_id}
                    for block_id in driver.SETTLEMENT_CHAIN_BLOCKS
                ]
            }
        if command == "screenshot":
            Path(payload["path"]).write_bytes(_MINIMAL_PNG)
            return {"saved": True}
        if command == "disconnect":
            return {}
        if command == "send_chat":
            message = payload["message"]
            self.commands.append(message)
            if message not in self.drop:
                self.chat.append(self._ack(message))
            return {"accepted": True}
        raise AssertionError(f"unexpected client command {command}")


class SettlementChainDriverTests(unittest.TestCase):
    def _run(
        self,
        client: _SettlementChainClient,
        run_dir: str,
        timeout_seconds: float = 5.0,
    ) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
        return driver.run_settlement_chain_scenario(
            client,
            Path(run_dir),
            driver.SETTLEMENT_CHAIN_SCENARIO,
            "127.0.0.1:25565",
            timeout_seconds,
            [],
        )

    def test_stage_does_not_advance_on_a_command_submission_alone(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            client = _SettlementChainClient(
                drop={"gamemode creative"},
                seed_chat=["Set game mode to Creative"],
            )
            with self.assertRaises(TimeoutError):
                self._run(client, tmp, timeout_seconds=0.5)
            self.assertEqual(client.commands, ["gamemode creative"])

    def test_absent_terminal_commit_event_fails_the_scenario_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            client = _SettlementChainClient(
                drop={"settlement build regsville house_small"},
            )
            with self.assertRaises(TimeoutError):
                self._run(client, tmp, timeout_seconds=1.0)
            self.assertEqual(
                client.commands.count("settlement build regsville house_small"), 1
            )

    def test_successful_chain_issues_exactly_one_build_and_completes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            client = _SettlementChainClient()
            result, _, screenshots, report = self._run(client, tmp)
            self.assertEqual(result, "passed")
            self.assertEqual(
                client.commands.count("settlement build regsville house_small"), 1
            )
            self.assertEqual(report["houses"], 1)
            self.assertEqual(report["population"], 1)
            self.assertEqual(report["committed_revision"], 20)
            self.assertEqual(len(screenshots), 1)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
