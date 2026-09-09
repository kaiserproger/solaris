"""Real audio-output checks for the Loader live gate; no synthetic playback acknowledgement."""
from __future__ import annotations

import array
import math
import os
from pathlib import Path
import subprocess
import threading
import time
from typing import Any, Callable


class SoundCapture:
    RATE = 48000
    BYTES_PER_FRAME = 8

    def __init__(self, artifact_dir: Path):
        self.path = artifact_dir / "audio.f32le"
        self.sink = f"solaris_loader_gate_{os.getpid()}"
        self.module = subprocess.check_output([
            "pactl", "load-module", "module-null-sink", f"sink_name={self.sink}",
            "rate=48000", "channels=2", "sink_properties=device.description=SolarisLoaderGate",
        ], text=True).strip()
        self.condition = threading.Condition()
        self.written = 0
        self.ended = False
        self.error: BaseException | None = None
        self.process: subprocess.Popen[bytes] | None = None
        self.thread: threading.Thread | None = None
        try:
            self.process = subprocess.Popen([
                "parec", f"--device={self.sink}.monitor", "--format=float32le",
                "--rate=48000", "--channels=2", "--latency-msec=20",
            ], stdout=subprocess.PIPE, stderr=(artifact_dir / "audio-capture.log").open("wb"))
            self.thread = threading.Thread(target=self._read, daemon=True)
            self.thread.start()
            self.sample(0.1)  # Readiness means actual monitor samples, not elapsed time.
        except BaseException:
            self.close()
            raise

    def _read(self) -> None:
        assert self.process is not None and self.process.stdout is not None
        try:
            with self.path.open("wb", buffering=0) as output:
                while data := self.process.stdout.read1(8192):
                    output.write(data)
                    with self.condition:
                        self.written += len(data)
                        self.condition.notify_all()
        except BaseException as error:
            self.error = error
        finally:
            with self.condition:
                self.ended = True
                self.condition.notify_all()

    def sample(self, seconds: float = 1.0) -> dict[str, Any]:
        size = int(seconds * self.RATE) * self.BYTES_PER_FRAME
        with self.condition:
            start = self.written - self.written % self.BYTES_PER_FRAME
            deadline = time.monotonic() + 15
            while self.written < start + size:
                if self.ended:
                    raise RuntimeError(f"audio monitor stopped: {self.error}")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError("audio monitor sample deadline elapsed")
                self.condition.wait(remaining)
        with self.path.open("rb") as source:
            source.seek(start)
            raw = source.read(size)
        samples = array.array("f", raw)
        mono = [(samples[i] + samples[i + 1]) * 0.5 for i in range(0, len(samples), 2)]
        amplitudes = {}
        for frequency in (440, 660):
            coefficient = 2 * math.cos(2 * math.pi * frequency / self.RATE)
            previous = before_previous = 0.0
            for value in mono:
                current = value + coefficient * previous - before_previous
                before_previous, previous = previous, current
            power = max(0.0, previous * previous + before_previous * before_previous
                        - coefficient * previous * before_previous)
            amplitudes[str(frequency)] = 2 * math.sqrt(power) / len(mono)
        return {"byte_offset": start, "byte_length": size, "frames": len(mono),
                "amplitude": amplitudes, "rms": math.sqrt(sum(value * value for value in mono) / len(mono))}

    def close(self) -> None:
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        if self.thread is not None:
            self.thread.join(timeout=5)
        subprocess.run(["pactl", "unload-module", self.module], check=True)


def exercise_sounds(
        capture: SoundCapture, request: Callable[[str, str], Any], player: dict[str, Any]
) -> dict[str, Any]:
    evidence: dict[str, Any] = {"format": "48000 Hz stereo float32le", "path": str(capture.path)}

    def record(name: str) -> dict[str, Any]:
        capture.sample(0.25)  # Consume actual pipeline samples before the measurement window.
        result = capture.sample()
        evidence[name] = result
        return result

    def amplitude(sample: dict[str, Any], frequency: int = 440) -> float:
        return sample["amplitude"][str(frequency)]

    request("ruby", "sound")
    local = record("personal")
    baseline = amplitude(local)
    if baseline < 0.005:
        raise RuntimeError("Loader personal sound produced no measurable 440 Hz audio")
    request("ruby", "sound_stop")
    stopped = record("stopped")
    if amplitude(stopped) > baseline * 0.03:
        raise RuntimeError("stop_client_sound left Ruby audio audible")

    request("ruby", "sound_quiet")
    quiet = record("volume_quarter")
    if not 0.15 < amplitude(quiet) / baseline < 0.4:
        raise RuntimeError("Loader volume did not scale actual audio output")
    request("ruby", "sound_stop")
    request("ruby", "sound_pitch")
    pitched = record("pitch_one_and_half")
    if amplitude(pitched, 660) < baseline * 0.5 or amplitude(pitched) > baseline * 0.03:
        raise RuntimeError("Loader pitch did not shift output from 440 Hz to 660 Hz")
    request("ruby", "sound_stop")

    x, y, z = player["x"], player["y"] + 1.62, player["z"]
    for name, distance in (("world_near", 0), ("world_middle", 8), ("world_far", 32)):
        request("ruby", f"sound_world {x + distance} {y} {z}")
        record(name)
        request("ruby", "sound_stop")
    near = amplitude(evidence["world_near"])
    middle = amplitude(evidence["world_middle"])
    far = amplitude(evidence["world_far"])
    if near < baseline * 0.5 or not near * 0.05 < middle < near * 0.8 or far > near * 0.03:
        raise RuntimeError("Loader world sound did not preserve measured vanilla distance attenuation")

    request("ruby", "sound")
    request("sapphire", "sound")
    both = record("both_owners")
    if amplitude(both) < baseline * 0.5 or amplitude(both, 660) < baseline * 0.5:
        raise RuntimeError("both owner sounds did not play concurrently")
    request("ruby", "sound_stop")
    isolated = record("ruby_stopped_sapphire_remains")
    if amplitude(isolated) > baseline * 0.03 or amplitude(isolated, 660) < baseline * 0.5:
        raise RuntimeError("stopping Ruby affected Sapphire or left Ruby audible")
    request("ruby", "sound_foreign_stop")
    fenced = record("foreign_stop_rejected")
    if amplitude(fenced, 660) < baseline * 0.5:
        raise RuntimeError("Ruby stopped another owner's sound")
    request("sapphire", "sound_stop")
    stopped = record("all_stopped")
    if any(amplitude(stopped, frequency) > baseline * 0.03 for frequency in (440, 660)):
        raise RuntimeError("stop_client_sound left owner audio audible")
    return evidence
