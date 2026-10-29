"""Received-state native reconnect, death/respawn and bounded memory soak."""
from __future__ import annotations

import time
import uuid
from pathlib import Path
from typing import Any

from .gameplay import cpu_usage, peak_rss_kib
from .probe_steps import ProbeSteps

REQUIRED_TOOLS = frozenset({
    "minecraft_observe", "minecraft_send_chat", "minecraft_wait_for_state_change",
    "minecraft_connect", "minecraft_disconnect", "minecraft_wait_for_play",
    "minecraft_wait_for_health_below", "minecraft_respawn", "minecraft_wait_ticks",
})


def _sample(pid: int, started: float) -> dict[str, Any]:
    status = Path(f"/proc/{pid}/status").read_text()
    rss = next((int(line.split()[1]) for line in status.splitlines()
                if line.startswith("VmRSS:")), None)
    cpu, peak = cpu_usage(pid), peak_rss_kib(pid)
    if rss is None or cpu is None or peak is None:
        raise RuntimeError("native lifecycle process metrics unavailable")
    return {"elapsed_seconds": time.monotonic() - started,
            "rss_kib": rss, "peak_rss_kib": peak, "cpu_seconds": cpu["cpu_seconds"]}

def _difficulty(steps: ProbeSteps) -> str:
    marker = f"lifecycle difficulty {uuid.uuid4().hex}"
    steps.fixture("difficulty")
    steps.call("minecraft_send_chat", {"message": marker, "command": False})
    state = steps.wait(lambda state: any(marker in line for line in state["recent_chat"]),
                       "difficulty_query_echo")
    lines = state["recent_chat"]
    marker_index = next(i for i, line in enumerate(lines) if marker in line)
    for line in lines[marker_index + 1:]:
        if "difficulty" in line.lower():
            for difficulty in ("peaceful", "easy", "normal", "hard"):
                if difficulty in line.lower():
                    return difficulty
            break
    raise RuntimeError("difficulty query did not produce a recognizable response")


def _set_difficulty(steps: ProbeSteps, difficulty: str) -> None:
    steps.fixture(f"difficulty {difficulty}")
    steps.check(f"difficulty_{difficulty}_confirmed", _difficulty(steps) == difficulty)



def run(client: Any, artifact: Path, timeout: float, address: str,
        pid: int, soak_seconds: float) -> dict[str, Any]:
    with ProbeSteps(client, artifact / "lifecycle-probe", timeout, "native-client-lifecycle") as steps:
        before = steps.wait(lambda state: state["player"]["health"] is not None,
                            "initial_health_received")
        identity = before["player"]["entity_uuid"]
        dimension = before["dimension"]
        steps.call("minecraft_disconnect")
        disconnected = steps.call("minecraft_observe")
        steps.check("disconnect_left_play", disconnected.get("in_play") is False)
        steps.call("minecraft_connect", {"server_addr": address})
        connected = steps.call("minecraft_wait_for_play", {"timeout_seconds": steps.remaining()})
        steps.check("reconnect_entered_play", connected.get("in_play") is True)
        after = steps.wait(lambda state: state["player"]["health"] is not None,
                           "reconnect_health_received")
        steps.check("reconnect_preserved_identity", after["player"]["entity_uuid"] == identity)
        steps.check("reconnect_preserved_dimension", after["dimension"] == dimension)
        marker = f"native reconnect {uuid.uuid4().hex}"
        steps.call("minecraft_send_chat", {"message": marker, "command": False})
        steps.wait(lambda state: any(marker in line for line in state["recent_chat"]),
                   "reconnected_server_chat_echo")
        steps.fixture("kill")
        dead = steps.call("minecraft_wait_for_health_below", {
            "health": 0.001, "timeout_seconds": steps.remaining(),
        })
        steps.report["death_wait"] = dead
        # Health 0 and the death screen arrive in separate packets.
        death_state = steps.wait(lambda state: state["player"]["health"] == 0
                                 and state["screen"]["open"] is True,
                                 "death_health_received")
        steps.check("death_screen_open", death_state["screen"]["open"] is True)
        respawn = steps.call("minecraft_respawn", {"timeout_seconds": steps.remaining()})
        steps.report["respawn_result"] = respawn
        # The client reloads its world after Respawn (vanilla: loading screen
        # until the player's chunk is ready), so play resumes on a later
        # state; a world that never returns fails at the probe deadline.
        steps.call("minecraft_wait_for_play", {"timeout_seconds": steps.remaining()})
        alive = steps.call("minecraft_observe")
        while not (alive.get("in_play") is True
                   and alive["player"]["health"] is not None
                   and alive["player"]["health"] > 0
                   and alive["screen"]["open"] is False):
            steps.call("minecraft_wait_for_state_change", {
                "observed_version": alive["state_version"], "timeout_seconds": steps.remaining(),
            })
            alive = steps.call("minecraft_observe")
        steps.check("respawn_alive_screen_closed", True)
        steps.check("respawn_preserved_identity", alive["player"]["entity_uuid"] == identity)
        steps.check("respawn_preserved_dimension", alive["dimension"] == dimension)
        original_difficulty = _difficulty(steps)
        steps.report["soak_original_difficulty"] = original_difficulty
        _set_difficulty(steps, "peaceful")
        # Waiting uses acknowledged native client ticks, never a wall-clock sleep.
        # The elapsed-duration fence prevents an accelerated client clock from
        # turning a short run into a thirty-minute stability claim.
        started = time.monotonic()
        samples = [_sample(pid, started)]
        soak: dict[str, Any] = {
            "requested_seconds": soak_seconds, "actual_seconds": samples[0]["elapsed_seconds"],
            "samples": samples, "long_session_proven": False,
            "warmup_seconds": 60, "rss_growth_budget_kib": 4096,
            "scope": "stationary received-state session, after reconnect and death/respawn; not chunk travel or dimension-transition proof",
            "dimension_transition": {"status": "unverified", "reason": "This lifecycle scenario exercises same-dimension respawn only; native-v2-dimensions owns explicit cross-dimension command travel/respawn evidence."},
        }
        steps.report["soak"] = soak
        try:
            while time.monotonic() - started < soak_seconds:
                steps.call("minecraft_wait_ticks", {"ticks": 255})
                samples.append(_sample(pid, started))
                soak["actual_seconds"] = samples[-1]["elapsed_seconds"]
                state = steps.observe()
                steps.check("soak_identity_stable", state["player"]["entity_uuid"] == identity)
                steps.check("soak_dimension_stable", state["dimension"] == dimension)
                steps.check("soak_player_alive", state["player"]["health"] is not None
                            and state["player"]["health"] > 0)
        finally:
            _set_difficulty(steps, original_difficulty)
        duration = samples[-1]["elapsed_seconds"]
        stable_samples = [sample for sample in samples if sample["elapsed_seconds"] >= 60]
        stability_proven = soak_seconds >= 1800 and duration >= soak_seconds and len(stable_samples) >= 2
        soak["long_session_proven"] = stability_proven
        if len(samples) >= 2:
            elapsed = samples[-1]["elapsed_seconds"] - samples[0]["elapsed_seconds"]
            soak["average_cpu_cores"] = (samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]) / elapsed
            soak["peak_sampled_cpu_cores"] = max(
                (last["cpu_seconds"] - first["cpu_seconds"]) /
                (last["elapsed_seconds"] - first["elapsed_seconds"])
                for first, last in zip(samples, samples[1:]))
            soak["cpu_sample_scope"] = "process user+system CPU deltas over native 255-tick intervals; not instantaneous peak"
        if stability_proven:
            first, last = stable_samples[0], stable_samples[-1]
            soak["post_warmup_rss_growth_kib"] = last["rss_kib"] - first["rss_kib"]
            soak["post_warmup_peak_rss_growth_kib"] = max(s["rss_kib"] for s in stable_samples) - first["rss_kib"]
            steps.check("thirty_minute_elapsed_session", duration >= 1800)
            steps.check("post_warmup_rss_bounded", soak["post_warmup_peak_rss_growth_kib"] <= 4096)
    return steps.report
