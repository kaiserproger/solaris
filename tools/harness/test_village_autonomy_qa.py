"""Causal client observations and fail-closed autonomous-village receipts."""
from __future__ import annotations

import io
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import Mock, patch

from .backends import village_autonomy_qa as qa
from .backends import probe_steps


class VillageAutonomyQaTests(unittest.TestCase):
    def test_walk_uses_path_planner_for_the_final_loaded_leg(self) -> None:
        probe = Mock()
        initial = {"player": {"x": 128.5, "y": 70.0, "z": 0.5}}
        arrived = {"player": {"x": 120.5, "y": 70.0, "z": 7.5}}
        probe.observe.return_value = initial

        def act(tool, arguments):
            if tool == "minecraft_navigate_to_block":
                probe.observe.return_value = arrived
            return {}

        probe.call.side_effect = act
        with patch.object(qa.time, "monotonic", side_effect=[0.0, 0.0, 601.0]):
            state = qa._walk_to(probe, {}, (120.0, 70.0, 7.0), "approach")
        self.assertEqual(state, arrived)
        probe.call.assert_any_call("minecraft_navigate_to_block", {
            "x": 120, "y": 70, "z": 7, "timeout_seconds": 110.0,
        })
        self.assertFalse(any(call.args[0] == "minecraft_press_inputs"
                             for call in probe.call.call_args_list))

    def test_build_completion_accepts_actual_building_list_level_format(self) -> None:
        probe = Mock()
        probe.observe.return_value = {
            "recent_chat": ["Buildings: #123 storage level 1 [12, 70, 20]-[18, 75, 26]."]
        }
        with patch.object(qa, "_observe_blueprint", return_value=True), patch.object(
            qa.time, "monotonic", side_effect=[0.0, 0.0, 100.0, 100.0, 2.0],
        ):
            completed = qa._wait_building_level(
                probe, {}, "Storage", 1, 1.0, (1, 2, 3),
                SimpleNamespace(windowed=False), Path("unused"), "storage",
            )
        self.assertTrue(completed, "CLI reports 'storage level 1', not 'Storage L1'")

    def test_blueprint_observation_requires_every_non_air_cell(self) -> None:
        probe = Mock()
        record = {"id": 123, "kind": "storage", "level": 1, "min": [2, 70, 3], "max": [2, 70, 3]}
        blueprint = {"id": "test", "palette": {"p": "cobblestone"}, "levels": [{"layers": [["p"]]}]}
        probe.call.return_value = {"blocks": [{"x": 2, "y": 70, "z": 3, "block_id": "minecraft:cobblestone"}]}
        self.assertTrue(qa._observe_blueprint(probe, {}, record, blueprint, "complete"))
        probe.call.return_value = {"blocks": []}
        detail = {}
        self.assertFalse(qa._observe_blueprint(probe, detail, record, blueprint, "missing"))
        self.assertEqual(detail["client_blocks"]["missing"]["mismatch_count"], 1)

    def test_blueprint_observation_allows_gameplay_states_but_not_wrong_orientation(self) -> None:
        probe = Mock()
        record = {"id": 123, "kind": "farmer", "level": 1, "min": [0, 70, 0], "max": [0, 70, 0]}
        cases = [
            ("wheat[age=0]", "wheat", {"age": "3"}),
            ("farmland[moisture=7]", "farmland", {"moisture": "5"}),
            ("composter[level=0]", "composter", {"level": "1"}),
            ("oak_door[facing=north,half=lower,open=false]", "oak_door",
             {"facing": "north", "half": "lower", "open": "true"}),
        ]
        for authored, block, properties in cases:
            with self.subTest(block=block):
                blueprint = {"id": "test", "palette": {"p": authored}, "levels": [{"layers": [["p"]]}]}
                probe.call.return_value = {"blocks": [{
                    "x": 0, "y": 70, "z": 0, "block_id": f"minecraft:{block}", "properties": properties,
                }]}
                self.assertTrue(qa._observe_blueprint(probe, {}, record, blueprint, "live"))
        properties["facing"] = "south"
        self.assertFalse(qa._observe_blueprint(probe, {}, record, blueprint, "wrong-facing"))

    def test_chest_waits_for_server_menu_and_excludes_player_inventory(self) -> None:
        probe = Mock()
        probe.observe.return_value = {
            "container": {"container_id": 0, "menu_class": "net.minecraft.world.inventory.InventoryMenu",
                          "slot_count": 46, "slots": [{"slot": 9, "item_id": "minecraft:coal", "count": 64}]}
        }
        stock = {"slot": 0, "item_id": "minecraft:oak_log", "count": 2}
        probe.wait.return_value = {
            "container": {"container_id": 1, "menu_class": "net.minecraft.world.inventory.ChestMenu",
                          "slot_count": 63, "slots": [stock, {"slot": 27, "item_id": "minecraft:coal", "count": 64}]}
        }
        self.assertEqual(qa._open_chest(probe, (1, 2, 3)), {0: stock})
        probe.wait.assert_called_once()
        predicate = probe.wait.call_args.args[0]
        self.assertFalse(predicate(probe.observe.return_value))
        self.assertTrue(predicate(probe.wait.return_value))
        self.assertEqual(probe.wait.call_args.kwargs["timeout_seconds"], 15.0)

    def test_container_wait_expires_even_when_state_keeps_changing(self) -> None:
        clock = Mock(return_value=0.0)
        with patch.object(probe_steps.time, "monotonic", clock):
            probe = qa.ProbeSteps(Mock(), Path("unused"), 3600.0, "bounded-menu")
            probe.observe = Mock(return_value={"state_version": 1})

            def changed(*args):
                clock.return_value += 1.0
                return {}

            probe.call = Mock(side_effect=changed)
            with self.assertRaisesRegex(TimeoutError, "missing_menu"):
                probe.wait(lambda state: False, "missing_menu", timeout_seconds=15.0)
        self.assertFalse(probe.report["assertions"]["missing_menu"])
        self.assertEqual(probe.call.call_count, 15)
        self.assertTrue(all(0 < call.args[1]["timeout_seconds"] <= 15.0
                            for call in probe.call.call_args_list))

    def test_receipt_cannot_pass_with_failed_or_missing_required_behaviour(self) -> None:
        required = (
            "walk_to_village", "ownerless_settlement", "founding_chest_empty",
            "generated_buildings_certified", "natural_zones", "workers_produce_unfunded",
            "mutations_refused_unowned", "builder_completes_storage_l1",
            "claim_via_dialog_button", "second_claim_refused", "owner_orders_building", "restart_resume",
        )
        good = dict.fromkeys(required, "PASS")
        self.assertTrue(qa.all_behaviours_pass(good))
        for name in required:
            with self.subTest(name=name):
                self.assertFalse(qa.all_behaviours_pass(dict(good, **{name: "FAIL"})))
                self.assertFalse(qa.all_behaviours_pass({k: v for k, v in good.items() if k != name}))
        self.assertFalse(qa.all_behaviours_pass(dict(good, unexpected_failure="FAIL")))

    def test_recent_chat_keeps_newest_claim_response(self) -> None:
        state = {"recent_chat": ["Claimed village_c31fe52b.", *[f"older {n}" for n in range(12)]]}
        self.assertEqual(qa._recent_chat(state, 1), ["Claimed village_c31fe52b."])

    def test_project_progress_comes_from_live_projects_dialog(self) -> None:
        observed = {"ui_nodes": [{
            "id": "dialog/body/0",
            "label": "Mason's workshop · level 1 · 5/100\nSite: -12, 70, 24",
        }]}
        self.assertEqual(qa._project_progress(observed), {
            "blueprint": "mason", "level": 1, "done": 5, "total": 100, "origin": [-12, 70, 24],
        })
        self.assertIsNone(qa._project_progress({"ui_nodes": []}))
        self.assertIsNone(qa._project_progress({
            "ui_nodes": [{"id": "dialog/body/0", "label": "No active project: no residents."}],
        }))

    def test_planner_diagnostic_reads_status_instead_of_stale_chat(self) -> None:
        observed = {"ui_nodes": [{
            "id": "dialog/body/0",
            "label": "Mason's workshop · level 1 · 5/100\nSite: -12, 70, 24",
        }], "recent_chat": ["Buildings: stale list"]}
        with patch.object(qa, "_project_status", return_value=observed) as project, patch.object(
            qa, "_send", return_value={"recent_chat": ["Buildings: current list"]},
        ), patch.object(qa, "_settlement_stock", return_value={"minecraft:coal": 4}):
            status = qa._planner_status(Mock(), {}, (1, 2, 3), Mock())
        project.assert_called_once()
        self.assertEqual(status["project"], {
            "blueprint": "mason", "level": 1, "done": 5, "total": 100, "origin": [-12, 70, 24],
        })
        self.assertEqual(status["project_ui"], observed["ui_nodes"])

    def test_owner_order_waits_for_started_blueprint(self) -> None:
        probe = Mock()
        detail = {}
        other = {"ui_nodes": [{"id": "dialog/body/0", "label":
                 "House · level 1 · 0/100\nSite: 12, 70, 24"}]}
        wrong_level = {"ui_nodes": [{"id": "dialog/body/0", "label":
                       "Storage · level 2 · 0/100\nSite: 12, 70, 24"}]}
        target = {"ui_nodes": [{"id": "dialog/body/0", "label":
                  "Storage · level 1 · 0/100\nSite: 12, 70, 24"}]}
        with patch.object(qa, "_send", return_value={
            "recent_chat": ["Started storage construction."],
        }) as send, patch.object(
            qa, "_project_status", side_effect=[other, wrong_level, target],
        ), patch.object(qa.time, "monotonic", side_effect=[0.0, 0.0, 1.0, 2.0]):
            self.assertTrue(qa._wait_owner_order(probe, detail, "storage", 5.0))
        send.assert_called_once_with(probe, "settlement build storage")
        self.assertEqual(detail["owner_order_project"]["blueprint"], "storage")
        self.assertEqual(detail["owner_order_project"]["level"], 1)
        self.assertEqual(probe.call.call_count, 2)
        probe.call.assert_called_with("minecraft_wait_ticks", {"ticks": 100})

    def test_queued_order_rejects_matching_incumbent_without_fresh_started(self) -> None:
        probe = Mock()
        old_chat = ["Queued storage: busy", "Started storage construction."]
        probe.observe.return_value = {"recent_chat": old_chat}
        target = {"ui_nodes": [{"id": "dialog/body/0", "label":
                  "Storage · level 1 · 0/100\nSite: 12, 70, 24"}]}
        with patch.object(qa, "_send", return_value={"recent_chat": old_chat}) as send, patch.object(
            qa, "_project_status", return_value=target,
        ), patch.object(qa.time, "monotonic", side_effect=[0.0, 0.0, 1.0, 2.0]):
            self.assertFalse(qa._wait_owner_order(probe, {}, "storage", 2.0))
        send.assert_called_once_with(probe, "settlement build storage")

    def test_queued_order_requires_fresh_started_before_matching_project(self) -> None:
        probe = Mock()
        probe.observe.side_effect = [
            {"recent_chat": ["Queued storage: busy", "Started storage construction."]},
            {"recent_chat": ["Started storage construction.", "Queued storage: busy",
                             "Started storage construction."]},
        ]
        target = {"ui_nodes": [{"id": "dialog/body/0", "label":
                  "Storage · level 1 · 0/100\nSite: 12, 70, 24"}]}
        with patch.object(qa, "_send", return_value={
            "recent_chat": ["Queued storage: busy", "Started storage construction."],
        }) as send, patch.object(qa, "_project_status", return_value=target), patch.object(
            qa.time, "monotonic", side_effect=[0.0, 0.0, 1.0, 2.0],
        ):
            self.assertTrue(qa._wait_owner_order(probe, {}, "storage", 5.0))
        send.assert_called_once_with(probe, "settlement build storage")
        self.assertEqual(probe.observe.call_count, 2)

    def test_owner_order_refusal_cannot_reuse_older_acceptance(self) -> None:
        probe = Mock()
        with patch.object(qa, "_send", return_value={
            "recent_chat": ["Not enough materials.", "Started storage construction."],
        }) as send, patch.object(qa, "_project_status") as status:
            self.assertFalse(qa._wait_owner_order(probe, {}, "storage", 2.0))
        send.assert_called_once_with(probe, "settlement build storage")
        status.assert_not_called()

    def test_owner_order_times_out_without_matching_level_one_project(self) -> None:
        probe = Mock()
        detail = {}
        wrong = {"ui_nodes": [{"id": "dialog/body/0", "label":
                 "Storage · level 2 · 0/100\nSite: 12, 70, 24"}]}
        with patch.object(qa, "_send", return_value={
            "recent_chat": ["Started storage construction."],
        }) as send, patch.object(qa, "_project_status", return_value=wrong), patch.object(
            qa.time, "monotonic", side_effect=[0.0, 0.0, 1.0, 2.0],
        ):
            self.assertFalse(qa._wait_owner_order(probe, detail, "storage", 2.0))
        send.assert_called_once_with(probe, "settlement build storage")
        self.assertEqual(detail["owner_order_project"]["level"], 2)
        self.assertEqual(probe.call.call_count, 2)

    def test_failed_second_player_check_still_closes_owned_clients(self) -> None:
        with TemporaryDirectory(prefix="autonomy-lifecycle-", dir=qa.runtime.REPO_ROOT / ".analysis") as folder:
            artifact = Path(folder)
            binary = artifact / "server"
            binary.write_bytes(b"unit-test binary identity")
            ctx = SimpleNamespace(
                client="native", extra_args=[], platform=None, windowed=True,
                timeout_seconds=10, artifact_dir=artifact, client_evidence={},
                commands=[], cwds=[], exits=[], logs=[],
            )
            server = Mock(pid=101, returncode=0)
            primary = Mock(pid=102, returncode=0)
            peer = Mock(pid=103, returncode=0)
            primary_log, peer_log = Mock(), Mock()
            primary_mcp, peer_mcp = Mock(token=""), Mock(token="")
            primary_mcp.call_tool.return_value = {"in_play": True}

            def start_primary(*args, **kwargs):
                ctx.client_evidence["command_index"] = 0
                ctx.commands.append(["unit-test native client"])
                ctx.exits.append(None)
                return primary, primary_log, primary_mcp

            def start_server(*args, **kwargs):
                config = (artifact / "village-autonomy-qa.toml").read_text()
                self.assertIn("spawn_protection = 0", config,
                              "the peer operator must not lock the non-op out of the founding chest")
                return server

            def fail_after_peer(*args):
                ctx.exits.append(None)
                args[-1]((peer, peer_log, peer_mcp), 1)
                raise RuntimeError("second-player check failed")

            with (
                patch.dict(qa.os.environ, {"SOLARIS_SERVER_BINARY": str(binary)}),
                patch.object(qa.subprocess, "Popen", side_effect=start_server),
                patch.object(qa.runtime, "reserve_port", return_value=25566),
                patch.object(qa.runtime, "wait_port"),
                patch.object(qa.runtime, "start_xvfb", return_value=(Mock(), ":100", Mock())),
                patch.object(qa.runtime, "stop_process") as stop,
                patch.object(qa.native_client, "resolve_binary", return_value=Path("unit-test-client")),
                patch.object(qa.native_client, "start", side_effect=start_primary),
                patch.object(qa.native_client, "capture"),
                patch.object(qa, "_scenario", side_effect=fail_after_peer),
                self.assertRaisesRegex(RuntimeError, "second-player check failed"),
            ):
                qa.run(ctx)
            self.assertIn(peer, [call.args[0] for call in stop.call_args_list])
            self.assertIn(primary, [call.args[0] for call in stop.call_args_list])
            peer_mcp.close.assert_called_once()
            peer_log.close.assert_called_once()
            primary_log.close.assert_called_once()
            self.assertEqual(ctx.exits, [0, 0])


    def test_dialog_claim_is_confirmed_by_owner_not_by_chat_or_closed_screen(self) -> None:
        # The server answers a dialog Claim click by re-presenting the same
        # dialog (note inside the dialog body), so neither a "Claimed " chat
        # line nor a closed screen ever appears; ownership is the evidence.
        probe = Mock()
        probe.call.return_value = {"status": "ok"}
        owned = {"recent_chat": [
            "village_7 | owner=2f1c anchor=[0, 70, 0] radius=100 members=8 revision=2",
            "village_7 | owner=autonomous anchor=[0, 70, 0] radius=100 members=8 revision=1",
        ]}
        detail: dict = {}
        with patch.object(qa, "_send", return_value=owned) as send:
            claimed = qa._claim_via_dialog(probe, detail, "village_7", "Settlement village_7")
        self.assertTrue(claimed)
        send.assert_called_once_with(probe, "settlement info village_7")
        probe.call.assert_any_call("minecraft_click_screen_button", {
            "expected_screen_class": qa.DIALOG_CLASS,
            "expected_title": "Settlement village_7",
            "button_label": "Claim settlement",
        })
        # No unbounded state-change wait may follow the click.
        self.assertFalse(probe.wait.called)

    def test_dialog_claim_that_leaves_settlement_ownerless_is_not_claimed(self) -> None:
        probe = Mock()
        probe.call.return_value = {"status": "ok"}
        still_ownerless = {"recent_chat": [
            "village_7 | owner=autonomous anchor=[0, 70, 0] radius=100 members=8 revision=1",
        ]}
        detail: dict = {}
        with patch.object(qa, "_send", return_value=still_ownerless):
            claimed = qa._claim_via_dialog(probe, detail, "village_7", "Settlement village_7")
        self.assertFalse(claimed)
        self.assertEqual(detail["claim_owner_after_click"], "autonomous")
        self.assertFalse(probe.wait.called)

    def test_dialog_claim_click_error_is_recorded_without_waiting(self) -> None:
        probe = Mock()
        probe.call.side_effect = RuntimeError("button not found")
        detail: dict = {}
        with patch.object(qa, "_send") as send:
            claimed = qa._claim_via_dialog(probe, detail, "village_7", "Settlement village_7")
        self.assertFalse(claimed)
        self.assertTrue(detail["claim_click_failed"])
        self.assertEqual(detail["claim_click_error"], "button not found")
        send.assert_not_called()
        self.assertFalse(probe.wait.called)


    def test_project_status_is_read_from_status_dialog_not_projects_grid(self) -> None:
        # The Projects grid carries only order buttons; live progress is on the
        # Status page reached by its button.
        grid = {"screen": {"class": qa.DIALOG_CLASS, "title": "Projects - village_7"}, "ui_nodes": []}
        status = {
            "screen": {"class": qa.DIALOG_CLASS, "title": "Project status - village_7"},
            "ui_nodes": [{"id": "dialog/body/0", "label": (
                "Storage · level 1 · 3/25\nSite: 1, 70, 2")}],
        }
        probe = Mock()
        probe.call.return_value = {"status": "ok"}
        # The Projects dialog closes first; the Status dialog arrives a beat later.
        probe.observe.side_effect = [
            {"screen": {"class": "none", "title": ""}, "ui_nodes": []}, status,
        ]
        with patch.object(qa, "_send", return_value=grid):
            observed = qa._project_status(probe)
        probe.call.assert_any_call("minecraft_click_screen_button", {
            "expected_screen_class": qa.DIALOG_CLASS,
            "expected_title": "Projects - village_7",
            "button_label": "Status",
        })
        progress = qa._project_progress(observed)
        self.assertEqual(
            (progress["blueprint"], progress["level"], progress["done"], progress["total"]),
            ("storage", 1, 3, 25),
        )
        probe.call.assert_any_call("minecraft_close_screen", {})


class OwnerOrderScenarioTests(unittest.TestCase):
    def test_pending_dialog_requires_order_and_records_material_labels_and_counts(self) -> None:
        state = {"ui_nodes": [
            {"id": "dialog/body/0", "label": "Pending: House L1 — waiting for materials"},
            {"id": "dialog/body/1", "label": "House:\nWhite Wool 0/3 — no source\nOak Log 2/12 — awaiting production"},
            {"id": "other", "label": "Ignored 1/2"},
        ]}
        pending = qa._pending_dialog(state)
        self.assertTrue(pending["pending"])
        self.assertEqual(pending["missing"], [
            "White Wool 0/3 — no source", "Oak Log 2/12 — awaiting production",
        ])
        self.assertFalse(qa._pending_dialog({"ui_nodes": []})["pending"])

    def test_only_fresh_started_chat_can_certify_admission(self) -> None:
        old = ["Queued house: not enough: wool", "Started house construction."]
        self.assertEqual(qa._new_chat_lines(old, old), [])
        self.assertEqual(qa._new_chat_lines(old, ["Started house construction.", *old]),
                         ["Started house construction."])
        self.assertEqual(qa._new_chat_lines(old, ["Started house construction."]), [])
        self.assertEqual(qa._new_chat_lines([], ["Started house construction."]),
                         ["Started house construction."])

    def test_founding_storage_uses_nearest_then_coordinate_tiebreak(self) -> None:
        probe = Mock()
        probe.call.side_effect = [
            {"blocks": [{"block_id": "minecraft:chest", "x": 2, "y": 64, "z": 0}],
             "truncated": True, "next_offset": 1},
            {"blocks": [{"block_id": "minecraft:barrel", "x": -1, "y": 64, "z": 0},
                        {"block_id": "minecraft:chest", "x": 1, "y": 64, "z": 0}],
             "truncated": False},
        ]
        self.assertEqual(qa._founding_storage(probe, (0, 64, 0)), (-1, 64, 0))


class SettlementStockObservationTests(unittest.TestCase):
    def test_sums_storage_without_navigating_or_opening_its_containers(self) -> None:
        probe = Mock()
        buildings = [
            "Buildings: #1 storage level 1 [10, 64, 0]-[13, 66, 3], "
            "#2 farmer level 1 [20, 64, 0]-[25, 68, 5].",
        ]
        probe.call.return_value = {"blocks": [
            {"x": 10, "y": 64, "z": 1, "block_id": "minecraft:chest"},
            {"x": 12, "y": 64, "z": 1, "block_id": "minecraft:barrel"},
            {"x": 99, "y": 64, "z": 1, "block_id": "minecraft:chest"},
        ]}
        contents = {
            (10, 64, 1): {"minecraft:cobblestone": 64},
            (12, 64, 1): {"minecraft:birch_log": 16},
        }
        reader = Mock(side_effect=contents.__getitem__)
        detail = {}
        with patch.object(qa, "_read", return_value={"block_id": "minecraft:chest"}), patch.object(
            qa, "_open_chest", return_value={0: {"item_id": "minecraft:cobblestone", "count": 2}},
        ) as opened:
            totals = qa._settlement_stock(probe, detail, (1, 64, 1), buildings, reader)
        self.assertEqual(totals, {"minecraft:cobblestone": 66, "minecraft:birch_log": 16})
        opened.assert_called_once_with(probe, (1, 64, 1))
        self.assertEqual({call.args[0] for call in reader.call_args_list}, set(contents))
        self.assertTrue(detail["storage_observation"]["complete"])
        self.assertEqual(len(detail["storage_observation"]["containers"]), 3)
        probe.call.assert_called_once_with("minecraft_scan_blocks", {
            "min_x": 10, "min_y": 64, "min_z": 0,
            "max_x": 13, "max_y": 66, "max_z": 3, "offset": 0,
        })

    def test_double_chest_and_overlapping_founding_position_are_counted_once(self) -> None:
        probe = Mock()
        probe.call.return_value = {"blocks": [
            {"x": 10, "y": 64, "z": 1, "block_id": "minecraft:chest",
             "properties": {"type": "left", "facing": "north"}},
            {"x": 11, "y": 64, "z": 1, "block_id": "minecraft:chest",
             "properties": {"type": "right", "facing": "north"}},
        ]}
        detail = {}
        reader = Mock()
        with patch.object(qa, "_open_chest", return_value={
            0: {"item_id": "minecraft:coal", "count": 4},
            27: {"item_id": "minecraft:birch_log", "count": 16},
        }) as opened:
            totals = qa._settlement_stock(probe, detail, (10, 64, 1), [
                "Buildings: #1 storage level 1 [10, 64, 0]-[12, 66, 3].",
                "Buildings: #1 storage level 1 [10, 64, 0]-[12, 66, 3].",
            ], reader)
        self.assertEqual(totals, {"minecraft:coal": 4, "minecraft:birch_log": 16})
        opened.assert_called_once_with(probe, (10, 64, 1))
        reader.assert_not_called()
        self.assertEqual(detail["storage_observation"]["containers"][0]["positions"],
                         [[10, 64, 1], [11, 64, 1]])

    def test_storage_scans_page_until_complete_and_reject_stalled_pagination(self) -> None:
        probe = Mock()
        buildings = ["Buildings: #1 storage level 1 [10, 64, 0]-[12, 66, 3]."]
        first = {"blocks": [], "truncated": True, "next_offset": 128}
        last = {"blocks": [{"x": 12, "y": 64, "z": 1, "block_id": "minecraft:barrel"}]}
        probe.call.side_effect = [first, last]
        reader = Mock(return_value={"minecraft:coal": 1})
        with patch.object(qa, "_read", return_value={"block_id": "minecraft:chest"}), patch.object(
            qa, "_open_chest", return_value={0: {"item_id": "minecraft:coal", "count": 1}},
        ):
            self.assertEqual(qa._settlement_stock(probe, {}, (1, 64, 1), buildings, reader),
                             {"minecraft:coal": 2})
        self.assertEqual(probe.call.call_args_list[-1].args[1]["offset"], 128)
        probe.call.side_effect = None
        probe.call.return_value = {"blocks": [], "truncated": True, "next_offset": 0}
        with self.assertRaisesRegex(RuntimeError, "pagination"):
            qa._settlement_stock(probe, {}, (1, 64, 1), buildings, reader)

    def test_adjacent_large_chests_sum_each_physical_half_once(self) -> None:
        probe = Mock()
        probe.call.return_value = {"blocks": [
            {"x": x, "y": 64, "z": 1, "block_id": "minecraft:chest",
             "properties": {"type": "left" if x % 2 == 0 else "right", "facing": "north"}}
            for x in range(10, 14)
        ]}
        reader = Mock(return_value={"minecraft:coal": 2})
        with patch.object(qa, "_open_chest", return_value={
            0: {"item_id": "minecraft:coal", "count": 4},
        }) as opened:
            totals = qa._settlement_stock(probe, {}, (10, 64, 1), [
                "Buildings: #1 storage level 1 [10, 64, 0]-[13, 66, 3].",
            ], reader)
        self.assertEqual(totals, {"minecraft:coal": 8})
        opened.assert_called_once_with(probe, (10, 64, 1))
        self.assertEqual([call.args[0] for call in reader.call_args_list], [(12, 64, 1), (13, 64, 1)])

    def test_storage_read_failure_records_reason_without_partial_total(self) -> None:
        probe = Mock()
        probe.call.return_value = {"blocks": [
            {"x": 12, "y": 64, "z": 1, "block_id": "minecraft:barrel"},
        ]}
        detail = {}
        with patch.object(qa, "_read", return_value={"block_id": "minecraft:chest"}), patch.object(
            qa, "_open_chest", return_value={},
        ), self.assertRaisesRegex(TimeoutError, "storage read"):
            qa._settlement_stock(probe, detail, (1, 64, 1), [
                "Buildings: #1 storage level 1 [10, 64, 0]-[13, 66, 3].",
            ], Mock(side_effect=TimeoutError("storage read timed out")))
        observation = detail["storage_observation"]
        self.assertFalse(observation["complete"])
        self.assertNotIn("totals", observation)
        self.assertIn("storage read timed out", observation["containers"][-1]["error"])

    def test_planner_receipt_uses_the_aggregate_storage_observation(self) -> None:
        detail = {}
        probe, reader = Mock(), Mock()
        buildings = ["Buildings: #1 storage level 1 [10, 64, 0]-[13, 66, 3]."]
        with patch.object(qa, "_send", side_effect=[
            {"recent_chat": ["Projects"]}, {"recent_chat": buildings},
        ]), patch.object(qa, "_settlement_stock", return_value={
            "minecraft:cobblestone": 190, "minecraft:acacia_log": 37,
        }) as stock:
            status = qa._planner_status(probe, detail, (1, 64, 1), reader)
        stock.assert_called_once_with(probe, detail, (1, 64, 1), buildings, reader)
        self.assertEqual(status["storage_now"]["minecraft:cobblestone"], 190)


class ServerStorageReadTests(unittest.TestCase):
    def test_snbt_reads_only_direct_item_fields_not_nested_component_decoys(self) -> None:
        source = ('{Items: [{Slot: 0b, components: {"example:metadata": '
                  '{id: "minecraft:diamond", count: 999, text: "escaped \\"[,]}" }}, '
                  'count: 4, id: "minecraft:coal"}, '
                  '{Slot: 1b, count: 2, id: "minecraft:coal"}], id: "minecraft:chest", x: 12}')
        self.assertEqual(qa._container_totals(source), {"minecraft:coal": 6})
        self.assertEqual(qa._container_totals('{id: "minecraft:chest"}'), {})
        self.assertEqual(qa._container_totals('{Items: []}'), {})
        for malformed in ('{Items: [', '{Items: [{id: "minecraft:coal"}]}',
                          '{Items: [{id: "minecraft:coal", count: -1}]}'):
            with self.subTest(source=malformed), self.assertRaises(ValueError):
                qa._container_totals(malformed)

    def test_console_reads_fresh_live_block_response_without_player_tools(self) -> None:
        with TemporaryDirectory() as folder:
            log = Path(folder) / "server.log"
            prefix = "12, 64, 1 has the following block data: "
            log.write_text(prefix + '{Items: [{count: 999, id: "minecraft:diamond"}]}\n')
            server = SimpleNamespace(stdin=io.BytesIO())

            def event(path, timeout, condition):
                self.assertEqual(path, log)
                self.assertEqual(timeout, 15.0)
                self.assertFalse(condition()[0], "a stale answer must not satisfy this read")
                with log.open("a") as stream:
                    stream.write(prefix + '{Items: [{count: 4, id: "minecraft:coal"}]}\n')
                reached, value = condition()
                self.assertTrue(reached)
                return value

            with patch.object(qa.runtime, "wait_for_path_condition", side_effect=event):
                self.assertEqual(qa._read_storage_container(server, log, (12, 64, 1), 15.0),
                                 {"minecraft:coal": 4})
            self.assertEqual(server.stdin.getvalue(), b"data get block 12 64 1\n")

    def test_console_timeout_and_refusal_fail_closed_with_bounded_wait(self) -> None:
        with TemporaryDirectory() as folder:
            log = Path(folder) / "server.log"
            log.touch()
            server = SimpleNamespace(stdin=io.BytesIO())
            with patch.object(qa.runtime, "wait_for_path_condition",
                              side_effect=TimeoutError("storage read timed out")) as wait:
                with self.assertRaisesRegex(TimeoutError, "storage read"):
                    qa._read_storage_container(server, log, (12, 64, 1), 15.0)
            self.assertEqual(wait.call_args.args[1], 15.0)

            def refused(path, timeout, condition):
                log.write_text("That position is not loaded\n")
                reached, value = condition()
                self.assertTrue(reached)
                return value

            with patch.object(qa.runtime, "wait_for_path_condition", side_effect=refused):
                with self.assertRaisesRegex(RuntimeError, "That position is not loaded"):
                    qa._read_storage_container(server, log, (12, 64, 1), 15.0)


if __name__ == "__main__":
    unittest.main()
