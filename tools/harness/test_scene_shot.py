import json
import socket
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.harness.backends import scene_shot


class SceneShotTests(unittest.TestCase):
    def test_scenario_username(self):
        self.assertEqual(scene_shot.scene_username({}), "SceneShot")
        self.assertEqual(scene_shot.scene_username({"username": "Other_Player"}), "Other_Player")
        for name in ("", "bad name", "a" * 17, 42):
            with self.subTest(name=name), self.assertRaises(ValueError):
                scene_shot.scene_username({"username": name})

    def test_latency_proxy_delays_both_directions(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        def echo():
            with listener:
                connection, _ = listener.accept()
                with connection:
                    connection.sendall(connection.recv(32))
        thread = threading.Thread(target=echo)
        thread.start()
        proxy = scene_shot.LatencyProxy(listener.getsockname()[1], 75)
        try:
            port = proxy.start()
            with socket.create_connection(("127.0.0.1", port), timeout=5) as client:
                started = time.monotonic()
                client.sendall(b"ping")
                self.assertEqual(client.recv(4), b"ping")
                self.assertGreaterEqual(time.monotonic() - started, 0.14)
        finally:
            proxy.close()
            thread.join()

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

    def test_screenshot_holds_and_releases_tab_even_on_capture_failure(self):
        events = []
        class Probe:
            client = object()
            def call(self, name, args):
                events.append(args["hold"])
            def wait(self, predicate, label):
                self.assert_visible = predicate({"ui_nodes": [{"id": "tab/player/1"}]})
                assert self.assert_visible
        probe = Probe()
        with tempfile.TemporaryDirectory() as tmp, patch.object(scene_shot.native_client, "capture", side_effect=RuntimeError("capture failed")):
            with self.assertRaisesRegex(RuntimeError, "capture failed"):
                scene_shot.execute(probe, Path(tmp), [{"shot": "tab", "hold_keys": ["key.keyboard.tab"]}])
        self.assertEqual(events, [True, False])

    def test_java_shot_uses_minecraft_screenshot(self):
        class Probe:
            client = object()
        with tempfile.TemporaryDirectory() as tmp, patch.object(scene_shot.java_p5, "capture", return_value="/x/a.png") as java_capture, \
                patch.object(scene_shot, "png_size", return_value=scene_shot.WINDOW), \
                patch.object(scene_shot.native_client, "capture") as native_capture:
            probe = Probe()
            self.assertEqual(scene_shot.execute(probe, Path(tmp), [{"shot": "a"}], "java"), ["/x/a.png"])
            java_capture.assert_called_once_with(probe, "a")
            native_capture.assert_not_called()

    def test_java_shot_with_wrong_window_size_fails(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(scene_shot.java_p5, "capture", return_value="/x/a.png"), \
                patch.object(scene_shot, "png_size", return_value=(854, 480)):
            with self.assertRaisesRegex(RuntimeError, "expected"):
                scene_shot.execute(object(), Path(tmp), [{"shot": "a"}], "java")

    def test_java_rejects_native_only_held_screenshot(self):
        with self.assertRaisesRegex(ValueError, "unsupported for --client java"):
            scene_shot.execute(None, Path("/tmp"), [{"shot": "tab", "hold_keys": ["key.keyboard.tab"]}], "java")

    def test_java_options_pin_window_fov_gui_and_distance(self):
        options = dict(line.split(":", 1) for line in scene_shot.java_options(32, 70).splitlines())
        self.assertEqual((options["overrideWidth"], options["overrideHeight"]), ("1280", "720"))
        self.assertEqual((options["fov"], options["guiScale"], options["renderDistance"]), ("0.0", "3", "32"))
        self.assertEqual(options["graphicsPreset"], '"custom"')
        self.assertEqual(scene_shot.java_options(8, 110).count("fov:1.0"), 1)

    def test_java_effective_render_distance_is_checked(self):
        with tempfile.TemporaryDirectory() as tmp:
            options = Path(tmp) / "options.txt"
            options.write_text("fov:0.0\nrenderDistance:32\n")
            scene_shot.check_java_render_distance(options, 32)
            options.write_text("renderDistance:16\n")
            with self.assertRaisesRegex(RuntimeError, "renderDistance.*16.*32"):
                scene_shot.check_java_render_distance(options, 32)
            options.write_text("fov:0.0\n")
            with self.assertRaisesRegex(RuntimeError, "renderDistance"):
                scene_shot.check_java_render_distance(options, 32)
