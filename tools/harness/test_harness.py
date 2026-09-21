#!/usr/bin/env python3
"""Fail-closed receipts and real process lifecycle checks for the harness.

Run with: python3 -m unittest tools.harness.test_harness
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import select
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
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


    def test_test_profile_uses_cargo_test(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                profiles.run_test(ctx)
        run.assert_called_once_with(
            ["cargo", "test", "--workspace", "--all-targets"],
            ctx,
            "test.log",
        )

    def test_test_profile_forwards_focused_cargo_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(
                Path(tmp), extra_args=["-p", "mc-net", "--lib", "player_attack"]
            )
            with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                profiles.run_test(ctx)
        run.assert_called_once_with(
            ["cargo", "test", "-p", "mc-net", "--lib", "player_attack"],
            ctx,
            "test.log",
        )

    def test_standard_pack_profile_checks_explicit_sibling(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            plugins_root = Path(tmp) / "plugins"
            for package_id, _ in profiles.STANDARD_PACK:
                package = plugins_root / package_id
                package.mkdir(parents=True)
                for name in ("plugin.toml", "plugin.wasm", "config.toml"):
                    (package / name).write_text(name)
            with (
                mock.patch.dict(
                    os.environ,
                    {"SOLARIS_DEFAULT_PLUGINS_ROOT": str(plugins_root)},
                ),
                mock.patch.object(
                    profiles, "_backend_env", return_value={"validation": "run"}
                ),
                mock.patch.object(
                    profiles, "_build_mc_server", return_value={"server": "built"}
                ),
                mock.patch.object(profiles, "_run_command", return_value={}) as run,
            ):
                profiles.run_standard_pack(ctx)
            source_call = mock.call(
                [
                    sys.executable,
                    str(plugins_root / "tools" / "build_standard_pack.py"),
                    "--check",
                    "--core-root",
                    str(profiles.REPO_ROOT),
                ],
                ctx,
                "standard-pack-source.log",
                cwd=plugins_root,
                env={"validation": "run"},
            )
            behavior_call = mock.call(
                [
                    "cargo",
                    "test",
                    "-p",
                    "mc-test-harness",
                    "--test",
                    "plugin_standard_pack",
                    "--",
                    "--ignored",
                ],
                ctx,
                "standard-pack-behavior.log",
                env={"validation": "run"},
            )
            live_workload_call = mock.call(
                [
                    "cargo",
                    "test",
                    "-p",
                    "mc-test-harness",
                    "--test",
                    "load_scenarios",
                    "p7_first_party_components_run_on_live_server_tick_workload",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                ctx,
                "standard-pack-live-workload.log",
                env={"validation": "run"},
            )
            config = ctx.artifact_dir / "standard-pack.toml"
            startup_call = mock.call(
                [
                    str(profiles.REPO_ROOT / "target" / "debug" / "mc-server"),
                    "--check",
                    "--config",
                    str(config),
                ],
                ctx,
                "standard-pack-startup.log",
            )
            self.assertEqual(
                run.call_args_list,
                [source_call, behavior_call, live_workload_call, startup_call],
            )
            self.assertEqual(
                (
                    ctx.artifact_dir
                    / "standard-pack"
                    / "plugins"
                    / "solaris-audit"
                    / "plugin.wasm"
                ).read_text(),
                "plugin.wasm",
            )
            rendered = config.read_text()
            self.assertIn(
                'expected = ["solaris-permissions", "solaris-essentials", '
                '"solaris-economy", "solaris-towns", "solaris-audit"]',
                rendered,
            )
            self.assertIn(
                '[plugins.grants.solaris-towns]\n'
                'capabilities = ["storage", "zones", "player_queries"]',
                rendered,
            )

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
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                try:
                    runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertTrue(runtime.port_open(port))
                    self.assertIsNone(process.poll())
                finally:
                    runtime.stop_process(process)
            self.assertFalse(runtime.port_open(port))

    def test_command_reports_started_and_completed_status(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                profiles._run_command([sys.executable, "-c", ""], ctx, "stage.log")
        status = output.getvalue()
        self.assertIn("[harness] stage=stage starting pid=", status)
        self.assertIn("[harness] stage=stage exit=0", status)

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



if __name__ == "__main__":
    raise SystemExit(unittest.main())
