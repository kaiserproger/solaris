"""Static/fail-closed native-v2 probe tests; these are not real-client evidence."""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from . import scenarios
from .backends import client_probes, native_v2
from .backends.probe_steps import ProbeSteps


class NativeV2ProbeTests(unittest.TestCase):
    def test_lifecycle_difficulty_query_is_ordered_and_fail_closed(self) -> None:
        from .backends import native_lifecycle

        steps = mock.Mock()
        steps.wait.return_value = {
            "recent_chat": ["lifecycle difficulty abc", "The difficulty is Easy"]
        }
        with mock.patch.object(native_lifecycle.uuid, "uuid4") as identifier:
            identifier.return_value.hex = "abc"
            self.assertEqual(native_lifecycle._difficulty(steps), "easy")
            steps.fixture.assert_called_once_with("difficulty")
            steps.wait.return_value = {"recent_chat": ["lifecycle difficulty abc", "not authorized"]}
            with self.assertRaisesRegex(RuntimeError, "difficulty query"):
                native_lifecycle._difficulty(steps)

    def test_radius_rejects_missing_or_non_eight_live_render_setting(self) -> None:
        from .backends import native_radius

        for settings in ({}, {"render_distance": 4}, {"render_distance": "8"},
                         {"render_distance": True}):
            with self.subTest(settings=settings), tempfile.TemporaryDirectory() as directory:
                client = mock.Mock()
                client.call_tool.return_value = {"in_play": True, "client_settings": settings}
                steps = ProbeSteps(client, Path(directory), 1, native_radius.SCENARIO_ID)
                with self.assertRaisesRegex(RuntimeError, "client_render_distance_is_8"):
                    native_radius.run(steps, (8, 65, 8))
                self.assertEqual(client.call_tool.call_args_list,
                                 [mock.call("minecraft_observe", {})])

    def test_radius_records_live_render_setting_and_reads_all_unique_chunk_cells(self) -> None:
        from .backends import native_radius

        client = mock.Mock()
        reads_since_wait = 0

        def call(name: str, args: dict) -> dict:
            nonlocal reads_since_wait
            if name == "minecraft_observe":
                return {"in_play": True, "client_settings": {"render_distance": 8}}
            if name == "minecraft_wait_ticks":
                self.assertGreaterEqual(args["ticks"], 20)
                reads_since_wait = 0
                return {}
            self.assertEqual(name, "minecraft_read_block")
            reads_since_wait += 1
            self.assertLessEqual(reads_since_wait, 32, "native MCP request budget must refill")
            return {"block_id": "minecraft:air"}

        client.call_tool.side_effect = call
        client.light_wire.query.return_value = []
        with tempfile.TemporaryDirectory() as directory:
            steps = ProbeSteps(client, Path(directory), 1, native_radius.SCENARIO_ID)
            with mock.patch.object(native_radius, "_echo"):
                native_radius.run(steps, (8, 65, 8))
            self.assertEqual(steps.report["radii"]["client_render_distance"], 8)
            cells = {tuple(call.args[1][key] for key in ("x", "z"))
                     for call in client.call_tool.call_args_list
                     if call.args[0] == "minecraft_read_block"}
            self.assertEqual(cells, {(x * 16 + 8, z * 16 + 8)
                                     for x in range(-8, 9) for z in range(-8, 9)})
            self.assertTrue(steps.report["assertions"]["full_17x17_client_readable"])

    def test_ranged_target_fixture_uses_only_needed_no_ai_tag(self) -> None:
        steps = mock.Mock(report={})
        steps.call.side_effect = RuntimeError("stop before live target wait")
        chat = {"recent_chat": ["Summoned new Iron Golem"]}
        with mock.patch.object(native_v2, "_echo", return_value=chat):
            with self.assertRaisesRegex(RuntimeError, "stop before live target wait"):
                native_v2._ranged(steps, (1, 2, 3))
        self.assertEqual(steps.fixture.call_args.args[0],
                         "summon minecraft:iron_golem 1.5 2 11.5 {NoAI:1b}")
        self.assertEqual(steps.report["target_fixture_chat"], chat["recent_chat"])

    def test_standard_inventory_fixture_does_not_assume_auxiliary_equipment(self) -> None:
        auxiliary = [{"slot": 41, "count": None, "state": "unknown"},
                     {"slot": 42, "count": None, "state": "unknown"}]
        self.assertTrue(native_v2._standard_inventory_empty({"inventory": auxiliary}))
        for ordinary in ({"slot": 0, "count": None, "state": "unknown"},
                         {"slot": 40, "count": None, "state": "unknown"},
                         {"slot": 9, "count": 1, "item_id": "minecraft:stone"},
                         {"count": None, "state": "unknown"}):
            with self.subTest(ordinary=ordinary):
                self.assertFalse(native_v2._standard_inventory_empty(
                    {"inventory": [*auxiliary, ordinary]}))

    def test_named_scenarios_route_existing_native_profile(self) -> None:
        self.assertEqual(set(native_v2.RUNNERS), set(native_v2.NATIVE_SCENARIO_IDS))
        for name in native_v2.NATIVE_SCENARIO_IDS:
            entry = scenarios.SCENARIOS[name]
            self.assertEqual(entry.profile, "entities")
            self.assertEqual(entry.scenario_id, name)
            expected = ("graphical" if name in ("native-v2-p5-humanoid-armor", "native-v2-light-seam",
                                                "native-v2-radius-full-view")
                        else "tcp")
            self.assertEqual(entry.evidence, expected)

    def test_p1_catalog_covers_all_five_surfaces(self) -> None:
        expected = {f"native-v2-p1-{name}" for name in ("armor-stand", "signs", "piglin", "trader", "leaves")}
        self.assertTrue(expected <= native_v2.NATIVE_SCENARIO_IDS)
        self.assertTrue(expected <= native_v2.RUNNERS.keys())

    def test_sign_wire_varint_waits_for_complete_prefix(self) -> None:
        from .backends.native_p1 import _varint

        self.assertIsNone(_varint(b""))
        self.assertIsNone(_varint(b"\x80"))
        self.assertEqual(_varint(b"\xac\x02"), (300, 2))
        with self.assertRaisesRegex(ValueError, "oversized"):
            _varint(b"\xff" * 5)

    def test_p2_catalog_covers_all_four_surfaces(self) -> None:
        expected = {f"native-v2-p2-{name}" for name in ("slime", "enderman", "fishing", "creaking")}
        self.assertTrue(expected <= native_v2.NATIVE_SCENARIO_IDS)
        self.assertTrue(expected <= native_v2.RUNNERS.keys())

    def test_fishing_trace_requires_exact_pinned_boolean_tail(self) -> None:
        from .backends.native_p1 import _hook_biting

        self.assertEqual(_hook_biting(b"\xac\x02\x08\x01\x00\x09\x08\x01\xff"), (300, True))
        self.assertEqual(_hook_biting(b"\x02\x08\x01\x00\x09\x08\x00\xff"), (2, False))
        self.assertIsNone(_hook_biting(b"\x02\x09\x03\x00\x00\x80\x3f\xff"))
        self.assertIsNone(_hook_biting(b"\x02\x09\x08\x01"))

    def test_p4_catalog_covers_requested_surfaces(self) -> None:
        from .backends import native_p4

        expected = {f"native-v2-p4-{name}" for name in
                    ("nameplates", "advancements", "damage", "hopper", "statistics")}
        self.assertTrue(expected <= native_v2.NATIVE_SCENARIO_IDS)
        self.assertTrue(expected <= native_v2.RUNNERS.keys())
        self.assertEqual(native_p4.RESTART_SCENARIOS,
                         {"native-v2-p4-nameplates", "native-v2-p4-advancements",
                          "native-v2-p4-statistics"})

    def test_p4_advancement_labels_are_actual_visible_ui_nodes(self) -> None:
        from .backends.native_p4 import _advancement_labels

        self.assertEqual(_advancement_labels({"ui_nodes": [
            {"id": "advancement/minecraft:story/root", "label": "✓ Minecraft"},
            {"id": "advancement/minecraft:story/upgrade_tools", "label": "✓ Getting an Upgrade"},
            {"id": "advancements/done", "label": "Done"},
        ]}), {"advancement/minecraft:story/root": "✓ Minecraft",
              "advancement/minecraft:story/upgrade_tools": "✓ Getting an Upgrade"})

    def test_fixture_coordinates_preserve_negative_origin(self) -> None:
        self.assertEqual(native_v2._point((-12, 81, -8), dx=-2, dz=1),
                         {"x": -14, "y": 81, "z": -7})

    def test_missing_tool_fails_before_privileged_setup(self) -> None:
        client = mock.Mock()
        client.list_tools.return_value = [{"name": name} for name in native_v2.REQUIRED_TOOLS
                                         if name != "minecraft_use_item_on"]
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "minecraft_use_item_on"):
                native_v2.run(client, Path(directory), 1, "native-v2-redstone")
        client.call_tool.assert_not_called()

    def test_wrong_profile_fails_before_server_resolution(self) -> None:
        ctx = mock.Mock(scenario_id="native-v2-redstone", client="native")
        with mock.patch.object(client_probes, "_server_binary") as binary:
            with self.assertRaisesRegex(ValueError, "entities"):
                client_probes.run(ctx, "inventory")
        binary.assert_not_called()

    def test_wrong_client_fails_before_server_resolution(self) -> None:
        ctx = mock.Mock(scenario_id="native-v2-redstone", client="java")
        with mock.patch.object(client_probes, "_server_binary") as binary:
            with self.assertRaisesRegex(ValueError, "--client native"):
                client_probes.run(ctx, "entities")
        binary.assert_not_called()

    def test_insert_requires_unique_received_source_and_preserves_failure_receipt(self) -> None:
        client = mock.Mock(token="")
        client.call_tool.return_value = {
            "in_play": True, "container": {"slots": [], "slot_count": 63},
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(RuntimeError, "unique_source"):
                with ProbeSteps(client, root, 1, "missing-source") as steps:
                    native_v2._insert(steps, "minecraft:diamond", 0)
            report = json.loads((root / "scenario.json").read_text())
            self.assertFalse(report["passed"])
            self.assertFalse(report["assertions"]["minecraft:diamond_unique_source"])
            self.assertEqual([call.args[0] for call in client.call_tool.call_args_list],
                             ["minecraft_observe"])

    def test_shared_java_launcher_disables_reusable_gradle_daemon(self) -> None:
        from . import runtime

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.object(Path, "is_file", return_value=True), \
                    mock.patch.object(runtime.os, "access", return_value=True), \
                    mock.patch.object(runtime, "port_open", return_value=False), \
                    mock.patch.object(runtime, "wait_port"), \
                    mock.patch.object(runtime.subprocess, "Popen") as launch:
                process, handle = runtime.start_client(
                    game_dir=root / "game", log_path=root / "client.log",
                    token="unit-test-only", mcp_port=12345, username="HarnessUnit", display=":99",
                )
                try:
                    self.assertIs(process, launch.return_value)
                    self.assertIn("--no-daemon", launch.call_args.args[0])
                    self.assertIn("--no-configuration-cache", launch.call_args.args[0])
                    self.assertTrue(launch.call_args.kwargs["start_new_session"])
                finally:
                    handle.close()

    def test_v2_operator_fixture_uses_observed_offline_uuid(self) -> None:
        from .backends.gameplay import prepare_operator_fixture

        with tempfile.TemporaryDirectory() as directory:
            ctx = mock.Mock(artifact_dir=Path(directory), client_evidence={})
            prepare_operator_fixture(ctx, ["HarnessSurvival"])
            rows = json.loads((Path(directory) / "ops.json").read_text())
            self.assertEqual(rows, [{
                "uuid": "92fea5d4-988a-3331-8b06-2c9858d14b0d", "name": "HarnessSurvival",
                "level": 4, "bypassesPlayerLimit": False,
            }])
            self.assertEqual(ctx.client_evidence["operator_fixture"], rows)


    def test_default_probe_observe_stays_strict_outside_play(self) -> None:
        client = mock.Mock(token="")
        client.call_tool.return_value = {"in_play": False}
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "in_play"):
                with ProbeSteps(client, Path(directory), 1, "strict") as steps:
                    steps.observe()
        self.assertEqual([call.args[0] for call in client.call_tool.call_args_list],
                         ["minecraft_observe"])

    def test_transition_wait_awaits_presented_play_after_state_change(self) -> None:
        client = mock.Mock(token="")
        client.call_tool.side_effect = [
            {"in_play": True, "state_version": 1, "dimension": "minecraft:overworld"},
            {}, {"in_play": False, "state_version": 2}, {"in_play": True},
            {"in_play": True, "state_version": 3, "dimension": "minecraft:the_nether"},
        ]
        with tempfile.TemporaryDirectory() as directory:
            with ProbeSteps(client, Path(directory), 1, "transition") as steps:
                state = steps.wait(lambda value: value["dimension"] == "minecraft:the_nether",
                                   "arrived", wait_for_play=True)
                self.assertEqual(state["dimension"], "minecraft:the_nether")
        self.assertEqual([call.args[0] for call in client.call_tool.call_args_list], [
            "minecraft_observe", "minecraft_wait_for_state_change", "minecraft_observe",
            "minecraft_wait_for_play", "minecraft_observe",
        ])

    def test_transition_play_timeout_remains_failed_evidence(self) -> None:
        client = mock.Mock(token="")
        client.call_tool.side_effect = [{"in_play": False}, TimeoutError("world never rebuilt")]
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory)
            with self.assertRaisesRegex(TimeoutError, "world never rebuilt"):
                with ProbeSteps(client, artifact, 1, "transition-timeout") as steps:
                    steps.observe(wait_for_play=True)
            self.assertFalse(json.loads((artifact / "scenario.json").read_text())["passed"])

    def test_title_preplay_wait_and_opt_in_play_observer_share_signature(self) -> None:
        from .backends.title_connect import _TitleProbeSteps

        client = mock.Mock(token="")
        client.call_tool.side_effect = [
            {"in_play": False, "screen": {"class": "TitleScreen"}},
            {"in_play": False}, {"in_play": True}, {"in_play": True},
        ]
        with tempfile.TemporaryDirectory() as directory:
            with _TitleProbeSteps(client, Path(directory), 1, "title-observer") as steps:
                steps.wait(lambda state: state["screen"]["class"] == "TitleScreen", "title")
                self.assertTrue(steps.observe(wait_for_play=True)["in_play"])
        self.assertEqual([call.args[0] for call in client.call_tool.call_args_list], [
            "minecraft_observe", "minecraft_observe", "minecraft_wait_for_play", "minecraft_observe",
        ])


if __name__ == "__main__":
    unittest.main()
