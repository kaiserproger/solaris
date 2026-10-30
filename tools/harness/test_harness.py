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
from types import SimpleNamespace


class ProcessStopTests(unittest.TestCase):
    def test_server_restart_can_wait_for_the_production_stop_deadline(self) -> None:
        process = mock.Mock(pid=123, returncode=0)
        process.poll.return_value = None
        with mock.patch.object(runtime.os, "killpg") as kill:
            runtime.stop_process(process, interrupt=True, timeout_seconds=90.0)
        kill.assert_called_once_with(123, signal.SIGINT)
        process.wait.assert_called_once_with(timeout=90.0)

    def test_server_stop_deadline_still_kills_a_process_that_does_not_exit(self) -> None:
        process = mock.Mock(pid=123)
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired("server", 90.0), -9]
        with mock.patch.object(runtime.os, "killpg") as kill:
            runtime.stop_process(process, interrupt=True, timeout_seconds=90.0)
        self.assertEqual(kill.call_args_list, [
            mock.call(123, signal.SIGINT), mock.call(123, signal.SIGKILL),
        ])
        self.assertEqual(process.wait.call_args_list, [
            mock.call(timeout=90.0), mock.call(timeout=5),
        ])

    def test_other_processes_keep_the_short_cleanup_deadline(self) -> None:
        process = mock.Mock(pid=123, returncode=0)
        process.poll.return_value = None
        with mock.patch.object(runtime.os, "killpg") as kill:
            runtime.stop_process(process)
        kill.assert_called_once_with(123, signal.SIGTERM)
        process.wait.assert_called_once_with(timeout=8.0)


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

class DriverLabelTests(unittest.TestCase):
    def test_missing_manifest_defaults_to_draft(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(driver.run_quality_label(Path(tmp)), "draft")



class RunTestPolicyTests(unittest.TestCase):
    def test_default_nextest_gate_keeps_all_targets_and_cpu_parallelism(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            env = {key: value for key, value in os.environ.items()
                   if key not in {"RUST_TEST_THREADS", "NEXTEST_TEST_THREADS"}}
            with mock.patch.dict(os.environ, env, clear=True):
                with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                    with mock.patch.object(profiles, "_test_scope_memory", return_value={}):
                        details = profiles.run_test(ctx)
                self.assertNotIn("RUST_TEST_THREADS", os.environ)
            self.assertEqual(run.call_args.args[0], [
                "cargo", "nextest", "run",
                "--profile", "default", "--ignore-default-filter", "--no-fail-fast",
                "--no-tests=fail", "--workspace", "--all-targets",
            ])
            self.assertNotIn("RUST_TEST_THREADS", run.call_args.kwargs["env"])
            self.assertEqual(details["test_runner"], "nextest")
            self.assertEqual(details["nextest_test_threads_env"], "num-cpus")
            self.assertIsNone(details["rust_test_threads_env"])

    def test_full_gate_preserves_explicit_concurrency(self) -> None:
        for overrides, expected in [
            ({"NEXTEST_TEST_THREADS": "6"}, "6"),
            ({"RUST_TEST_THREADS": "3", "NEXTEST_TEST_THREADS": "6"}, "3"),
        ]:
            with self.subTest(overrides=overrides), tempfile.TemporaryDirectory() as tmp:
                env = {key: value for key, value in os.environ.items()
                       if key not in {"RUST_TEST_THREADS", "NEXTEST_TEST_THREADS"}}
                env.update(overrides)
                with mock.patch.dict(os.environ, env, clear=True):
                    with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                        with mock.patch.object(profiles, "_test_scope_memory", return_value={}):
                            profiles.run_test(profiles.ProfileContext(Path(tmp)))
                self.assertEqual(run.call_args.kwargs["env"]["NEXTEST_TEST_THREADS"], expected)

    def test_nextest_failure_is_not_retried_with_cargo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(profiles, "_run_command", side_effect=RuntimeError("nextest failed")) as run:
                with self.assertRaisesRegex(RuntimeError, "nextest failed"):
                    profiles.run_test(profiles.ProfileContext(Path(tmp)))
            run.assert_called_once()

    def test_non_execution_flags_cannot_pass_the_gate(self) -> None:
        for flag in ["--no-run", "--help", "-h", "--version", "-V"]:
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as tmp:
                with mock.patch.object(profiles, "_run_command") as run:
                    with self.assertRaisesRegex(ValueError, "must execute tests"):
                        profiles.run_test(profiles.ProfileContext(Path(tmp), extra_args=[flag]))
                run.assert_not_called()

    def test_focused_selectors_and_nextest_threads_use_the_same_runner(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            args = ["-p", "mc-edge", "--lib", "--test-threads=2", "inventory"]
            ctx = profiles.ProfileContext(Path(tmp), extra_args=args)
            with mock.patch.dict(os.environ, {"RUST_TEST_THREADS": "3"}):
                with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                    with mock.patch.object(profiles, "_test_scope_memory", return_value={}):
                        profiles.run_test(ctx)
            self.assertEqual(run.call_args.args[0], [
                "cargo", "nextest", "run",
                "--profile", "default", "--ignore-default-filter", "--no-fail-fast",
                "--no-tests=fail", *args,
            ])
            self.assertEqual(run.call_args.kwargs["env"]["NEXTEST_TEST_THREADS"], "3")

    def test_ci_short_profile_applies_to_test_but_not_correctness(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            ctx = profiles.ProfileContext(Path(tmp))
            with mock.patch.dict(os.environ, {"SOLARIS_NEXTEST_PROFILE": "ci"}):
                with mock.patch.object(profiles, "_run_command", return_value={}) as run:
                    with mock.patch.object(profiles, "_test_scope_memory", return_value={}):
                        profiles.run_test(ctx)
                        short = run.call_args.args[0]
                        profiles.run_test(ctx, full=True)
                        full = run.call_args.args[0]
            self.assertEqual(short[3:5], ["--profile", "ci"])
            self.assertNotIn("--ignore-default-filter", short)
            self.assertEqual(full[3:6], ["--profile", "default", "--ignore-default-filter"])

    def test_selected_run_profile_reaches_scope(self) -> None:
        for profile in ["test", "correctness", "check"]:
            with self.subTest(profile=profile):
                with mock.patch.object(cli, "_exec_in_scope", return_value=0) as scope:
                    self.assertEqual(cli.main(["run", profile]), 0)
                scope.assert_called_once_with(profile)

    def test_profile_memory_defaults_match_admission_and_scope(self) -> None:
        env = {key: value for key, value in os.environ.items()
               if key not in {cli.CPU_SCOPE_ENV, cli.MEMORY_MAX_ENV}}
        for profile, expected in [("test", "12G"), ("correctness", "12G"), ("check", "4G"), (None, "4G")]:
            with self.subTest(profile=profile), mock.patch.dict(os.environ, env, clear=True):
                with mock.patch.object(cli, "_default_cpu_quota", return_value="600%"):
                    with mock.patch.object(cli, "_available_bytes", return_value=16 * 1024**3):
                        with mock.patch.object(cli, "_scope_command", return_value=["scope"]) as scope:
                            with mock.patch.object(cli.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
                                with contextlib.redirect_stdout(io.StringIO()):
                                    self.assertEqual(cli._exec_in_scope(profile), 0)
                scope.assert_called_once_with("600%", expected)

    def test_test_profile_admission_uses_twelve_gib_and_preserves_focused_override(self) -> None:
        env = {key: value for key, value in os.environ.items()
               if key not in {cli.CPU_SCOPE_ENV, cli.MEMORY_MAX_ENV}}
        with mock.patch.dict(os.environ, env, clear=True):
            with mock.patch.object(cli, "_default_cpu_quota", return_value="600%"):
                with mock.patch.object(cli, "_available_bytes", return_value=12 * 1024**3):
                    with mock.patch.object(cli, "_scope_command", return_value=["scope"]) as scope:
                        with contextlib.redirect_stderr(io.StringIO()):
                            self.assertEqual(cli._exec_in_scope("test"), 1)
                        scope.assert_not_called()
                with mock.patch.dict(os.environ, {cli.MEMORY_MAX_ENV: "8G"}):
                    with mock.patch.object(cli, "_available_bytes", return_value=16 * 1024**3):
                        with mock.patch.object(cli, "_scope_command", return_value=["scope"]) as scope:
                            with mock.patch.object(cli.subprocess, "run", return_value=SimpleNamespace(returncode=0)):
                                with contextlib.redirect_stdout(io.StringIO()):
                                    self.assertEqual(cli._exec_in_scope("correctness"), 0)
                        scope.assert_called_once_with("600%", "8G")

    def test_scope_command_applies_the_resolved_profile_cap(self) -> None:
        with mock.patch.object(cli.shutil, "which", return_value="/usr/bin/systemd-run"):
            with mock.patch.object(cli.subprocess, "run", return_value=SimpleNamespace(stdout="running")):
                command = cli._scope_command("600%", "12G")
        self.assertIn("MemoryMax=12G", command)

    def test_scope_peak_is_read_before_transient_scope_exits(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cgroup = root / "cgroup"
            cgroup.write_text("0::/solaris-heavy.slice/test.scope\n")
            scope = root / "solaris-heavy.slice/test.scope"
            scope.mkdir(parents=True)
            (scope / "memory.peak").write_text("512\n")
            (scope / "memory.max").write_text("1024\n")
            (scope / "memory.events").write_text("max 3\noom 0\noom_kill 0\n")
            (scope / "memory.swap.peak").write_text("128\n")
            self.assertEqual(profiles._test_scope_memory(cgroup, root), {
                "memory_peak_bytes": 512,
                "memory_limit_bytes": 1024,
                "memory_cgroup": "/solaris-heavy.slice/test.scope",
                "memory_events": {"max": 3, "oom": 0, "oom_kill": 0},
                "memory_swap_peak_bytes": 128,
            })

    def test_missing_scope_is_not_reported_as_zero_memory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            details = profiles._test_scope_memory(Path(tmp) / "missing", Path(tmp))
            self.assertIsNone(details["memory_peak_bytes"])
            self.assertIn("memory_peak_error", details)


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

    def test_native_missing_tools_fail_receipt_before_gameplay(self) -> None:
        from .backends.gameplay import require_tools

        def missing_tools(ctx):
            require_tools(
                [{"name": "minecraft_observe"}],
                {"minecraft_observe", "minecraft_break_block", "minecraft_use_item_on"},
                "survival",
            )
            self.fail("gameplay must not execute after missing tool discovery")

        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, {
                    "survival": {"scope": "real-client", "run": missing_tools},
                }):
                    code = cli.cmd_run(["survival", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertIn("minecraft_break_block", payload["error"])
        self.assertIn("minecraft_use_item_on", payload["error"])

    def test_native_never_falls_back_to_java_only_profile(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["loader-live", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertEqual(payload["commands"], [])


class GravesCustodyTests(unittest.TestCase):
    """The gate's own assertions must fail closed on duplication."""

    @staticmethod
    def receipt(**overrides):
        receipt = {
            "status": "PASS",
            "source_unchanged": True,
            "blocks": [{
                "dimension": "minecraft:overworld", "x": -574, "y": 71, "z": 320,
                "state": 1, "block": "minecraft:barrel",
                "items": [{"item": "minecraft:dirt", "count": 7, "place": "container", "slot": 0}],
            }],
            "players": [{"uuid": "u", "items": []}],
            "item_entities": [],
        }
        receipt.update(overrides)
        return receipt

    def test_custody_passes_with_exact_barrel_and_no_copies(self) -> None:
        from .backends.graves import check_custody

        checks: dict[str, object] = {}
        check_custody(checks, self.receipt(), item="minecraft:dirt", count=7)
        self.assertTrue(all(check["passed"] for check in checks.values()))

    def test_inventory_copy_fails_closed(self) -> None:
        from .backends.graves import check_custody

        receipt = self.receipt(players=[{"uuid": "u", "items": [
            {"item": "minecraft:dirt", "count": 7, "place": "inventory", "slot": 0}]}])
        with self.assertRaisesRegex(RuntimeError, "player_inventory_holds_no_copy"):
            check_custody({}, receipt, item="minecraft:dirt", count=7)

    def test_dropped_copy_fails_closed(self) -> None:
        from .backends.graves import check_custody

        receipt = self.receipt(item_entities=[
            {"type_id": 40, "position": [0.0, 0.0, 0.0], "item": "minecraft:dirt", "count": 7}])
        with self.assertRaisesRegex(RuntimeError, "no_dropped_copy"):
            check_custody({}, receipt, item="minecraft:dirt", count=7)

    def test_wrong_barrel_count_fails_closed(self) -> None:
        from .backends.graves import check_custody

        receipt = self.receipt()
        receipt["blocks"][0]["items"][0]["count"] = 6
        with self.assertRaisesRegex(RuntimeError, "barrel_holds_exactly_given_stacks"):
            check_custody({}, receipt, item="minecraft:dirt", count=7)

    def test_non_barrel_cell_fails_closed(self) -> None:
        from .backends.graves import check_custody

        receipt = self.receipt()
        receipt["blocks"][0]["block"] = "minecraft:air"
        with self.assertRaisesRegex(RuntimeError, "grave_is_barrel"):
            check_custody({}, receipt, item="minecraft:dirt", count=7)

    def test_probe_failure_receipt_never_passes(self) -> None:
        from .backends.graves import check_custody

        receipt = self.receipt(status="FAIL", source_unchanged=None)
        receipt["error"] = "journal tail"
        with self.assertRaisesRegex(RuntimeError, "probe_pass"):
            check_custody({}, receipt, item="minecraft:dirt", count=7)

    def test_grave_coordinates_parse_from_the_plugin_request(self) -> None:
        from .backends.graves import GRAVE_REQUEST

        match = GRAVE_REQUEST.search(
            "plugin.command_result: request=grave--574-71-320-cd88 outcome=\"committed\"")
        self.assertIsNotNone(match)
        self.assertEqual([int(match.group(i)) for i in (1, 2, 3)], [-574, 71, 320])


class SharedClientProbeTests(unittest.TestCase):
    def test_inventory_missing_click_tools_fails_before_fixture_actions(self) -> None:
        from . import compatibility

        class MissingClicks:
            def list_tools(self):
                return [{"name": name} for name in compatibility.INVENTORY_TOOLS
                        if name not in {"minecraft_click_container_slot", "minecraft_quick_move_container_slot"}]

            def call_tool(self, *args):
                raise AssertionError("a missing tool must fail before any fixture or action")

        def probe(ctx):
            return compatibility.run_inventory_probe(MissingClicks(), ctx.artifact_dir)

        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)), mock.patch.dict(profiles.PROFILES, {
                "inventory": {"scope": "real-client", "run": probe},
            }):
                code = cli.cmd_run(["inventory", "--client", "native"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
        self.assertEqual(code, 1)
        self.assertEqual(payload["status"], "failed")
        self.assertEqual(payload["client"], "native")
        self.assertIn("minecraft_click_container_slot", payload["error"])
        self.assertIn("minecraft_quick_move_container_slot", payload["error"])

    def test_entities_missing_attack_tool_fails_before_summon(self) -> None:
        from .backends import entities_probe

        class MissingAttack:
            def list_tools(self):
                return [{"name": name} for name in entities_probe.REQUIRED_TOOLS
                        if name != "minecraft_attack_entity_once"]

            def call_tool(self, *args):
                raise AssertionError("a missing attack tool must fail before summon")

        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(RuntimeError, "minecraft_attack_entity_once"):
                entities_probe.run(MissingAttack(), Path(tmp), 1.0)

    def test_unknown_container_slot_is_not_treated_as_empty(self) -> None:
        from .backends.probe_steps import container_slot

        with self.assertRaises(RuntimeError):
            container_slot({"container": {"slot_count": 9, "slots": []}}, 9)


class NativeDescriptorTests(unittest.TestCase):
    def test_descriptor_refuses_a_different_process(self) -> None:
        from .backends.native_client import _read_descriptor

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "native-test.json"
            path.write_text(json.dumps({"instance_id": "native-test", "pid": os.getpid() + 1}))
            with self.assertRaisesRegex(RuntimeError, "PID"):
                _read_descriptor(path, root, os.getpid(), {})

    def test_world_readable_token_is_not_used_for_attach(self) -> None:
        from .backends.native_client import _read_descriptor

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            token = root / "native-test.token"
            token.write_text("private-bearer-value")
            token.chmod(0o644)
            path = root / "native-test.json"
            path.write_text(json.dumps({
                "instance_id": "native-test", "pid": os.getpid(),
                "endpoint": "http://127.0.0.1:12345/mcp", "token_file": str(token),
            }))
            evidence = {}
            with self.assertRaisesRegex(RuntimeError, "0600"):
                _read_descriptor(path, root, os.getpid(), evidence)
            self.assertNotIn("private-bearer-value", json.dumps(evidence))


class SurvivalTargetTests(unittest.TestCase):
    def test_only_loaded_exposed_nonstanding_ground_is_targeted(self) -> None:
        from .backends.survival import _candidates

        blocks = []
        for x, z in ((0, 0), (1, 0), (2, 0), (0, 1)):
            blocks.append({"x": x, "y": 64, "z": z, "block_id": "minecraft:grass_block"})
            for y in (65, 66):
                blocks.append({"x": x, "y": y, "z": z, "block_id": "minecraft:air"})
        # The standing column, an occluded face and an unloaded air cell are unsafe.
        blocks.append({"x": 2, "y": 65, "z": 0, "block_id": "minecraft:stone"})
        blocks = [block for block in blocks
                  if not (block["x"] == 0 and block["z"] == 1 and block["y"] == 66)]
        targets = _candidates(blocks, {"x": 0.5, "y": 65.0, "z": 0.5})
        self.assertEqual([(block["x"], block["y"], block["z"]) for block in targets],
                         [(1, 64, 0)])


class ScenarioCatalogTests(unittest.TestCase):
    """The catalog must resolve to real checks and never report false green."""

    def _fake_scenario(self, tests, log_line):
        from . import scenarios as scenario_catalog

        def run(ctx):
            log_path = ctx.artifact_dir / "test.log"
            log_path.write_text(log_line)
            ctx.logs.append(str(log_path))
            return {}

        scenario = scenario_catalog.Scenario(
            id="fake-scenario",
            summary="fake",
            owner="test",
            evidence="native",
            profile="fake-profile",
            selector=("-p", "mc-edge", "--lib", "fake"),
            tests=tests,
            proves="nothing",
            limits="test fixture",
        )
        return scenario, {"fake-profile": {"scope": "harness", "run": run}}

    def test_unknown_scenario_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["scenario", "no-such-scenario"])
            self.assertEqual(code, 2)
            receipt = next(Path(tmp).glob("*/result.json"))
            payload = json.loads(receipt.read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertNotIn("named_scenario", payload["details"])

    def test_named_scenario_rejects_free_form_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                code = cli.cmd_run(["scenario", "inventory-client", "extra-arg"])
            self.assertEqual(code, 2)
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(payload["commands"], [])

    def test_explain_writes_no_artifact_and_rejects_unknown_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    self.assertEqual(cli.cmd_explain(["inventory-client"]), 0)
                    self.assertEqual(cli.cmd_explain(["no-such-scenario"]), 2)
            self.assertEqual(list(Path(tmp).iterdir()), [])

    def test_named_scenario_requires_every_expected_test_to_run(self) -> None:
        from . import scenarios as scenario_catalog

        passing = (
            "test native::fake_case ... ok\n\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2389 filtered out\n"
        )
        failing = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"
        scenario, profile = self._fake_scenario(("native::fake_case",), passing)
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                    scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                ):
                    code = cli.cmd_run(["scenario", "fake-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(code, 0)
            self.assertEqual(payload["status"], "passed")
            self.assertEqual(payload["details"]["named_scenario"]["missing_tests"], [])
            self.assertEqual(
                payload["details"]["named_scenario"]["engine_counts"],
                {"selected": 1, "executed": 1, "passed": 1, "failed": 0,
                 "ignored": 0, "filtered_out": 2389},
            )

        scenario, profile = self._fake_scenario(("native::fake_case",), failing)
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                    scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                ):
                    code = cli.cmd_run(["scenario", "fake-scenario"])
            payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
            self.assertEqual(code, 1)
            self.assertEqual(payload["status"], "failed")
            self.assertEqual(
                payload["details"]["named_scenario"]["missing_tests"],
                ["native::fake_case"],
            )

    def test_named_scenario_rejects_invalid_engine_counts(self) -> None:
        from . import scenarios as scenario_catalog

        passed_test = "test script::storage::settlement_tests::fake_case ... ok\n"
        summaries = {
            "executed": ("test result: ok. 3 passed; 0 failed; 1 ignored\n", 0, "passed"),
            "zero-selected": ("test result: ok. 0 passed; 0 failed; 0 ignored\n", 1, "failed"),
            "ignored-only": ("test result: ok. 0 passed; 0 failed; 2 ignored\n", 1, "failed"),
            "failed": ("test result: FAILED. 1 passed; 1 failed; 0 ignored\n", 1, "failed"),
            "missing-summary": ("", 1, "failed"),
        }
        for label, (summary, expected_exit, expected_status) in summaries.items():
            with self.subTest(label=label), tempfile.TemporaryDirectory() as tmp:
                scenario, profile = self._fake_scenario(
                    ("script::storage::settlement_tests::fake_case",), passed_test + summary
                )
                with mock.patch.object(cli, "ARTIFACT_ROOT", Path(tmp)):
                    with mock.patch.dict(profiles.PROFILES, profile), mock.patch.dict(
                        scenario_catalog.SCENARIOS, {"fake-scenario": scenario}
                    ):
                        code = cli.cmd_run(["scenario", "fake-scenario"])
                payload = json.loads(next(Path(tmp).glob("*/result.json")).read_text())
                self.assertEqual(code, expected_exit)
                self.assertEqual(payload["status"], expected_status)


    def test_catalog_entries_name_existing_profiles_and_sources(self) -> None:
        from . import scenarios as scenario_catalog

        for scenario in scenario_catalog.SCENARIOS.values():
            self.assertIn(scenario.profile, profiles.PROFILES, scenario.id)
            for source in scenario.sources:
                self.assertTrue((runtime.REPO_ROOT / source).exists(), f"{scenario.id}: {source}")


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
                    with self.assertRaises(RuntimeError):
                        runtime.wait_port(port, 10.0, process, log_path=log_path)
                    self.assertEqual(process.wait(timeout=10), 3)
                finally:
                    runtime.stop_process(process)

    def test_xvfb_closed_display_announcement_stops_process_before_fallback(self) -> None:
        popen = subprocess.Popen
        processes = []
        log_handles = []

        def launch(argv, **kwargs):
            if "-displayfd" not in argv:
                raise FileNotFoundError("fallback Xvfb unavailable")
            descriptor = argv[argv.index("-displayfd") + 1]
            process = popen(
                [sys.executable, "-c",
                 "import os,signal,sys; os.close(int(sys.argv[1])); signal.pause()",
                 descriptor],
                **kwargs,
            )
            processes.append(process)
            log_handles.append(kwargs["stdout"])
            return process

        with tempfile.TemporaryDirectory() as tmp:
            try:
                with mock.patch.object(runtime.subprocess, "Popen", launch):
                    with self.assertRaises(FileNotFoundError):
                        runtime.start_xvfb(Path(tmp))
                self.assertLess(processes[0].wait(timeout=10), 0)
                self.assertTrue(log_handles[0].closed)
            finally:
                for process in processes:
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
