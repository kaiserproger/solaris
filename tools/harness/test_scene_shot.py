import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.harness.backends import scene_shot


class SceneShotTests(unittest.TestCase):
    def test_steps_dispatch_and_retain_evidence(self):
        class Probe:
            def __init__(self):
                self.client = object()
                self.calls = []
            def call(self, name, args=None):
                self.calls.append((name, args))
                return {"in_play": True}
            def observe(self):
                return {"in_play": True, "position": [1, 2, 3]}
            def wait(self, predicate, label, *, wait_for_play=False):
                assert wait_for_play
                assert predicate({"player": {"x": 1, "z": 3}})
            def remaining(self):
                return 50
        probe = Probe()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with patch.object(scene_shot.native_client, "capture", return_value={"retained_path": str(root / "a.png")}):
                shots = scene_shot.execute(probe, root, [{"wait_play": True}, {"cmd": "time set 18000"}, {"tp": [1, 2, 3, 90, 0]}, {"wait_ticks": 5}, {"observe": "state"}, {"shot": "a"}])
            self.assertEqual(shots, [str(root / "a.png")])
            self.assertEqual(json.loads((root / "state.json").read_text())["position"], [1, 2, 3])
            self.assertIn(("minecraft_send_chat", {"message": "tp @s 1 2 3 90 0", "command": True}), probe.calls)
            self.assertIn(("minecraft_wait_ticks", {"ticks": 5}), probe.calls)

    def test_unknown_or_unsafe_step_fails(self):
        for step in ({"shot": "../escape"}, {"unknown": True}, {"wait_play": False}, {"tp": [1, 2]}):
            with self.subTest(step=step), self.assertRaises(ValueError):
                scene_shot.execute(None, Path('/tmp'), [step])

    def test_wait_play_spans_mcp_wait_boundaries_and_detects_disconnect(self):
        class Probe:
            def __init__(self, results):
                self.results = iter(results)
                self.calls = 0
            def remaining(self):
                return 120
            def call(self, name, args):
                self.calls += 1
                return next(self.results)
        probe = Probe([{"in_play": False}, {"in_play": True}])
        scene_shot.wait_play(probe)
        self.assertEqual(probe.calls, 2)
        with self.assertRaisesRegex(RuntimeError, "kicked"):
            scene_shot.wait_play(Probe([{"in_play": False, "disconnect_reason": "kicked"}]))

    def test_long_waits_are_split_for_lease_renewal(self):
        class Probe:
            def __init__(self):
                self.ticks = []
            def call(self, name, args):
                self.ticks.append((name, args["ticks"]))
        probe = Probe()
        scene_shot.execute(probe, Path("/tmp"), [{"wait_ticks": 250}])
        self.assertEqual(probe.ticks, [("minecraft_wait_ticks", 100),
                                      ("minecraft_wait_ticks", 100),
                                      ("minecraft_wait_ticks", 50)])
