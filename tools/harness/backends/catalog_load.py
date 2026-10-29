"""Owner-catalog open-loop loads against the ordinary v2 server.

No builds or internal Core APIs: fixtures are authored through one operator wire
connection, saved by the real server, hashed, then reopened for measurement.
"""
from __future__ import annotations

import argparse
import asyncio
from collections import Counter
import copy
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import shutil
import sys
import time
import uuid
from typing import Any

from . import catalog_support as scale
from .. import runtime
from .. import catalog_controls, catalog_mixes

ROOT = Path(__file__).resolve().parents[3]
MAPPING = ROOT / "tools/harness/catalog_load_scenarios.json"
PLATFORM_COMMANDS = [
    "fill -32 63 -32 32 63 32 minecraft:grass_block",
    *(f"fill -32 {y} -32 32 {y + 3} 32 minecraft:air" for y in range(64, 96, 4)),
    "setworldspawn 0 64 0",
    "gamerule doDaylightCycle false",
    "gamerule doMobSpawning false",
    "clear @s",
    "spawnpoint @s 0 64 0",
    "tp @s 0.5 64 0.5",
    "gamemode survival @s",
    "save-all flush",
]


class ClassifiedFailure(RuntimeError):
    def __init__(self, outcome: str, reason: str):
        super().__init__(reason)
        self.outcome = outcome


def digest(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def tree_hash(path: Path) -> dict[str, Any]:
    files = {p.relative_to(path).as_posix(): scale.binary_sha256(p)
             for p in sorted(path.rglob("*")) if p.is_file()}
    return {"sha256": digest(files), "files": files}


def percentile(values: list[int], fraction: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def tick_metrics(samples: list[dict[str, Any]]) -> dict[str, Any]:
    # Full active iteration wall time, not the narrower TickOutput total_us.
    values = [s["wall_us"] for s in samples]
    return {"samples": len(values), "p50_us": percentile(values, .50),
            "p95_us": percentile(values, .95), "p99_us": percentile(values, .99),
            "max_us": max(values) if values else None,
            "deadline_misses_50ms": sum(v > 50_000 for v in values),
            "scope": "exact window core active-iteration wall_us; excludes pacing sleep",
            "p99_stability": "Short control/smoke windows are not stable tail evidence; repeat longer windows near the boundary."}


def sustained_tps(samples: list[dict[str, Any]], start_ns: int, end_ns: int,
                  seconds: int) -> float | None:
    bins = [0] * ((end_ns - start_ns) // 1_000_000_000)
    if len(bins) < seconds:
        return None
    for sample in samples:
        index = (sample["wall_start_unix_ns"] - start_ns) // 1_000_000_000
        if 0 <= index < len(bins):
            bins[index] += 1
    active = sum(bins[:seconds])
    minimum = active
    for index in range(seconds, len(bins)):
        active += bins[index] - bins[index - seconds]
        minimum = min(minimum, active)
    return minimum / seconds


def classify(*, capability: str | None = None, infra: list[str] = (),
             setup: list[str] = (), correctness: list[str] = (), performance: list[str] = ()) -> str:
    if capability == "UNSUPPORTED_MECHANIC":
        return capability
    if capability == "INFRA_BLOCKED" or infra:
        return "INFRA_BLOCKED"
    if setup:
        return "SETUP_FAILED"
    if correctness:
        return "CORRECTNESS_FAIL"
    if performance:
        return "PERF_FAIL"
    return "PASS"


class Session:
    """Bounded child ownership; raw data is streamed to disk, not retained in RAM."""
    def __init__(self, args: argparse.Namespace, directory: Path):
        self.args, self.directory = args, directory
        self.processes: dict[str, asyncio.subprocess.Process] = {}
        self.logs: list[Any] = []
        self.readers: list[asyncio.Task[Any]] = []
        self.events: asyncio.Queue[dict[str, Any]] = asyncio.Queue(maxsize=128)
        self.latest_bot: dict[str, Any] = {}
        self.login_release: dict[str, Any] | None = None
        self.shutdown: dict[str, Any] = {}
        self.failure: dict[str, Any] | None = None
        self.stopping = False
        self.fixture_commands_sent: dict[str, Any] | None = None
        self.checkpoints: list[dict[str, Any]] = []
        self.live_complete: dict[str, Any] | None = None
        self.completed_schedule: dict[str, Any] | None = None

    async def spawn(self, name: str, command: list[str], *, events: bool = False,
                    console: bool = False) -> None:
        log = (self.directory / f"{name}.log").open("wb")
        self.logs.append(log)
        process = await asyncio.create_subprocess_exec(
            *command, cwd=ROOT, start_new_session=True,
            stdin=asyncio.subprocess.PIPE if events or console else asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE if events else log, stderr=log, limit=64 * 1024 * 1024)
        self.processes[name] = process
        if events:
            self.readers.append(asyncio.create_task(self.read_events(process, log)))
        self.readers.append(asyncio.create_task(self.watch_exit(name, process)))

    def push_event(self, event: dict[str, Any]) -> None:
        if event.get("open_loop", {}).get("schedule_complete") is True:
            self.completed_schedule = event
        if not self.stopping and (event.get("event") == "failure" or event.get("disconnects", 0)):
            self.failure = event
        if self.events.full():
            self.events.get_nowait()
        self.events.put_nowait(event)

    async def watch_exit(self, name: str, process: asyncio.subprocess.Process) -> None:
        code = await process.wait()
        if not self.stopping:
            self.push_event({"event": "failure", "error": f"{name} exited: {code}"})

    async def read_events(self, process: asyncio.subprocess.Process, log: Any) -> None:
        assert process.stdout
        try:
            while line := await process.stdout.readline():
                log.write(line)
                log.flush()
                event = json.loads(line)
                if not isinstance(event, dict):
                    raise ValueError("bot event is not an object")
                self.latest_bot = event
                if event.get("event") == "schedule_complete":
                    self.live_complete = event
                if event.get("event") in {"fixture_commands_sent", "fixture_checkpoint_saved"}:
                    event["captured_unix_ns"] = time.time_ns()
                if event.get("event") == "fixture_commands_sent":
                    self.fixture_commands_sent = event
                if event.get("event") == "fixture_checkpoint_saved":
                    self.checkpoints.append(event)
                    if len(self.checkpoints) > 4096:
                        del self.checkpoints[0]
                if event.get("event") == "login_release":
                    self.login_release = event
                self.push_event(event)
        except (ValueError, UnicodeError, OSError) as error:
            self.push_event({"event": "failure", "error": str(error), "outcome": "INFRA_BLOCKED"})

    async def ready(self, requested: int, timeout: float) -> dict[str, Any]:
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.alive("SETUP_FAILED")
            try:
                event = await asyncio.wait_for(self.events.get(), end - time.monotonic())
            except asyncio.TimeoutError as error:
                raise ClassifiedFailure("SETUP_FAILED", "all-client readiness deadline expired") from error
            if event.get("event") == "failure":
                raise ClassifiedFailure("SETUP_FAILED", str(event.get("error", event)))
            if event.get("event") == "ready":
                if event.get("clients") != requested:
                    raise ClassifiedFailure("SETUP_FAILED", "not every requested bot became ready")
                return event
        raise ClassifiedFailure("SETUP_FAILED", "all-client readiness deadline expired")

    def alive(self, outcome: str = "CORRECTNESS_FAIL") -> None:
        if self.failure is not None:
            raise ClassifiedFailure(self.failure.get("outcome", outcome),
                str(self.failure.get("error", "bots reported disconnects")))
        for name, process in self.processes.items():
            if process.returncode is not None:
                raise ClassifiedFailure(outcome, f"{name} exited unexpectedly: {process.returncode}")
        if self.latest_bot.get("event") == "failure":
            raise ClassifiedFailure(outcome, str(self.latest_bot.get("error")))
        if self.latest_bot.get("disconnects", 0):
            raise ClassifiedFailure(outcome, "bot disconnected")


    async def http(self, url: str, timeout: float) -> dict[str, Any]:
        try:
            return await asyncio.to_thread(scale.http_json, url, timeout)
        except (OSError, ValueError) as error:
            self.alive()
            raise ClassifiedFailure("INFRA_BLOCKED", f"telemetry unavailable: {error}") from error

    def process_samples(self) -> dict[str, Any]:
        try:
            return {name: scale.proc_sample(p.pid) for name, p in self.processes.items()}
        except (OSError, ValueError) as error:
            self.alive()
            raise ClassifiedFailure("INFRA_BLOCKED", f"process metrics unavailable: {error}") from error

    async def stop(self, *, server_timeout: float = 10) -> None:
        self.stopping = True
        for name in ("bots", "server"):
            process = self.processes.get(name)
            if not process:
                continue
            forced = False
            if process.returncode is None:
                try:
                    os.killpg(process.pid, signal.SIGINT if name == "bots" else signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    await asyncio.wait_for(process.wait(), server_timeout if name == "server" else 10)
                except asyncio.TimeoutError:
                    forced = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    await process.wait()
            self.shutdown[name] = {"returncode": process.returncode, "forced": forced}
        for reader in self.readers:
            if not reader.done():
                reader.cancel()
        await asyncio.gather(*self.readers, return_exceptions=True)
        for log in self.logs:
            log.close()


def config(args: argparse.Namespace, directory: Path, world: Path, clients: int,
           *, operators: bool, name_prefix: str = "scale",
           plugins_dir: Path | None = None) -> tuple[Path, str, int]:
    game, dashboard = scale.reserve_ports()
    path = directory / "server.toml"
    path.write_text(f'''[server]
name = "catalog-load-isolated"
view_distance = {args.view_distance}
simulation_distance = {args.view_distance}
max_players = {max(10, clients)}
worldgen_gpu = {json.dumps(getattr(args, "worldgen_gpu", False))}
[network]
bind_address = "127.0.0.1"
port = {game}
[auth]
online_mode = false
[data]
world_dir = {json.dumps(str(world))}
seed = {args.seed}
vanilla_data_dir = {json.dumps(str(args.vanilla_data_dir))}
[dashboard]
enabled = true
bind_address = "127.0.0.1"
port = {dashboard}
allow_remote = false
''')
    if plugins_dir is not None:
        with path.open("a") as handle:
            handle.write(f"\n[plugins]\ndirectory = {json.dumps(str(plugins_dir))}\n")
    if operators:
        ops = []
        for index in range(clients):
            username = f"{name_prefix}_{index:04}"
            offline = uuid.UUID(bytes=hashlib.md5(("OfflinePlayer:" + username).encode()).digest(), version=3)
            ops.append({"uuid": str(offline), "name": username, "level": 4, "bypassesPlayerLimit": False})
        scale.dump(directory / "ops.json", ops)
    return path, f"http://127.0.0.1:{dashboard}", game


async def wait_dashboard(session: Session, url: str, timeout: float) -> dict[str, Any]:
    process = session.processes["server"]

    class ReadinessProcess:
        pid = process.pid

        @property
        def returncode(self) -> int | None:
            return process.returncode

        def poll(self) -> int | None:
            return process.returncode

    # Reuse canonical Linux pidfd/inotify/socket event readiness.
    port = int(url.rsplit(":", 1)[1])
    try:
        await asyncio.to_thread(runtime.wait_port, port, timeout, ReadinessProcess(),
            log_path=session.directory / "server.log")
        return await asyncio.to_thread(scale.http_json, url + "/stats", min(2, timeout))
    except (OSError, ValueError, RuntimeError) as error:
        raise ClassifiedFailure("INFRA_BLOCKED", f"v2 dashboard readiness failed: {error}") from error


def bot_command(args: argparse.Namespace, directory: Path, game: int, clients: int,
                recipe: dict[str, Any] | None = None) -> list[str]:
    group = min(64, recipe.get("group_size", 32)) if recipe else 32
    command = [str(args.bots_bin), "--addr", f"127.0.0.1:{game}", "--clients", str(clients),
               "--view-distance", str(args.view_distance), "--spacing", "256", "--group-size", str(group),
               "--report", str(directory / "bots-report.json"), "--prepare-seconds", str(args.readiness_seconds)]
    if getattr(args, "prepare_concurrency", None):
        command += ["--prepare-concurrency", str(args.prepare_concurrency)]
    traced = [index for index in (getattr(args, "wire_trace_bots", None) or []) if index < clients]
    if traced:
        command += ["--wire-trace-bots", ",".join(map(str, traced)),
                    "--wire-trace-file", str(directory / "bots-wire.jsonl")]
    return command


async def _await_fixture_checkpoint(session: Session, timeout: float) -> dict[str, Any]:
    """Wait for the ordered durable barrier, not one feedback packet per command."""
    end = time.monotonic() + timeout
    receipt: dict[str, Any] = {}
    if session.fixture_commands_sent is not None:
        receipt["commands_sent_event"] = session.fixture_commands_sent
    if session.checkpoints and "commands_sent_event" in receipt:
        receipt["checkpoint_event"] = session.checkpoints[-1]
        receipt["command_feedback"] = session.latest_bot.get("command_feedback_received", 0)
        return receipt
    while time.monotonic() < end:
        session.alive("SETUP_FAILED")
        try:
            event = await asyncio.wait_for(session.events.get(), max(.01, end - time.monotonic()))
        except asyncio.TimeoutError as error:
            raise ClassifiedFailure("SETUP_FAILED", "fixture checkpoint acknowledgement deadline expired") from error
        if event.get("event") == "fixture_commands_sent":
            receipt["commands_sent_event"] = event
        if event.get("event") == "fixture_checkpoint_saved" and "commands_sent_event" in receipt:
            receipt["checkpoint_event"] = event
            receipt["command_feedback"] = session.latest_bot.get("command_feedback_received", 0)
            return receipt
    raise ClassifiedFailure("SETUP_FAILED", "fixture checkpoint acknowledgement deadline expired")


async def _event(session: Session, name: str, timeout: float) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while True:
        session.alive("SETUP_FAILED")
        try:
            event = await asyncio.wait_for(session.events.get(), max(.01, deadline - time.monotonic()))
        except asyncio.TimeoutError as error:
            raise ClassifiedFailure("SETUP_FAILED", f"missing {name} receipt") from error
        if event.get("event") == name:
            return event
        if time.monotonic() >= deadline:
            raise ClassifiedFailure("SETUP_FAILED", f"missing {name} receipt")


async def _await_preparation_schedule(session: Session, timeout: float) -> dict[str, Any]:
    """Retain sample-based clock completion across a later save acknowledgement."""
    deadline = time.monotonic() + timeout
    while session.completed_schedule is None:
        session.alive("SETUP_FAILED")
        try:
            await asyncio.wait_for(session.events.get(), max(.01, deadline - time.monotonic()))
        except asyncio.TimeoutError as error:
            raise ClassifiedFailure("SETUP_FAILED", "missing completed preparation schedule") from error
        if session.completed_schedule is None and time.monotonic() >= deadline:
            raise ClassifiedFailure("SETUP_FAILED", "missing completed preparation schedule")
    return session.completed_schedule


def mechanic_spec(recipe: dict[str, Any], *, native: bool = False) -> dict[str, Any]:
    """Project declared wire-only menus separately from persistent inventories."""
    spec = copy.deepcopy(recipe["mechanic_spec"])
    if "containers" in spec:
        containers = []
        for probe in spec["containers"]:
            wire_only = probe.pop("wire_only", False)
            if not native or not wire_only:
                containers.append(probe)
        spec["containers"] = containers
    for action in spec.pop("timed_actions", []):
        action["window"] = action.pop("phase")
        action["at_ms"] = action.pop("offset_ms")
        spec.setdefault("actions", []).append(action)
    return spec


async def native_snapshot(args: argparse.Namespace, directory: Path, world: Path,
                          server_config: Path, spec: dict[str, Any],
                          initial: dict[str, Any] | None = None) -> dict[str, Any]:
    """Use the frozen server's read-only converter for native container images."""
    directory.mkdir()
    before = tree_hash(world)
    snapshot_spec = copy.deepcopy(spec)
    revision_source = ROOT / "crates/mc-worldgen/src/lib.rs"
    match = re.search(r"pub const WORLDGEN_REVISION: u32 = (\d+);", revision_source.read_text())
    if match is None:
        raise ClassifiedFailure("INFRA_BLOCKED", "native world identity revision contract unavailable")
    snapshot_spec.update(native_seed=args.seed, native_worldgen_revision=int(match.group(1)),
                         native_data_dir=str(args.vanilla_data_dir))
    if spec.get("containers") or spec.get("blocks"):
        destination = directory / "anvil"
        conversion = [str(args.server_bin), "--config", str(server_config), "world", "export-anvil",
                      "--source", str(world), "--destination", str(destination),
                      "--receipt", str(directory / "export-receipt.json")]
        with (directory / "export.log").open("wb") as log:
            process = await asyncio.create_subprocess_exec(*conversion, cwd=ROOT, stdout=log, stderr=log)
            try:
                await asyncio.wait_for(process.wait(), max(120, args.readiness_seconds * 4))
            except BaseException:
                if process.returncode is None:
                    process.kill()
                    await process.wait()
                raise
        if process.returncode:
            raise ClassifiedFailure("INFRA_BLOCKED", f"read-only native export failed; see {directory / 'export.log'}")
        snapshot_spec["native_anvil_snapshot"] = str(destination / "dimensions/minecraft/overworld")
    scale.dump(directory / "spec.json", snapshot_spec)
    command = [str(args.bots_bin), "--addr", "127.0.0.1:1", "--clients", "1",
               "--view-distance", "1", "--native-snapshot", str(world),
               "--mechanic-spec", str(directory / "spec.json"), "--report", str(directory / "report.json")]
    with (directory / "probe.log").open("wb") as log:
        process = await asyncio.create_subprocess_exec(*command, cwd=ROOT, stdout=log, stderr=log)
        try:
            await asyncio.wait_for(process.wait(), max(120, args.readiness_seconds * 4))
        except BaseException:
            if process.returncode is None:
                process.kill()
                await process.wait()
            raise
    if process.returncode:
        raise ClassifiedFailure("INFRA_BLOCKED", f"read-only native snapshot failed; see {directory / 'probe.log'}")
    raw = json.loads((directory / "report.json").read_text())["mechanics"]
    after = tree_hash(world)
    if before != after:
        raise ClassifiedFailure("CORRECTNESS_FAIL", "read-only native probe modified its source world")
    prior = {row["uuid"]: row for group in (initial or {}).get("entities", {}).values()
             for row in group.get("samples", [])}
    prior_absent = {row["uuid"]: row for row in (initial or {}).get("absence_probes", [])
                    if row.get("known_initial") is True and row.get("full_native_index_checked") is True}
    prior.update(prior_absent)
    groups: dict[str, Any] = {}
    for row in raw["entities"]:
        if not row["active"]:
            continue
        payload = row["payload"]
        kind = payload.get("id")
        if not isinstance(kind, str):
            raise ClassifiedFailure("INFRA_BLOCKED", "native entity lacks canonical type identity")
        position = payload.get("Pos")
        sample = {"uuid": row["uuid"], "initial_position": prior.get(row["uuid"], {}).get("final_position", position),
                  "final_position": position, "active": True, "payload": payload}
        group = groups.setdefault(kind, {"kind": kind, "samples": [], "active": 0})
        group["samples"].append(sample)
        group["active"] += 1
    active_ids = {sample["uuid"] for group in groups.values() for sample in group["samples"]}
    prior_kinds = {sample["uuid"]: kind for kind, group in (initial or {}).get("entities", {}).items()
                   for sample in group.get("samples", [])}
    prior_kinds.update({identity: row["kind"] for identity, row in prior_absent.items()})
    tombstones = {row["uuid"] for row in raw["entities"] if not row["active"]}
    raw["absence_probes"] = [{"uuid": identity, "kind": prior_kinds[identity], "active": False,
        "final_position": None, "known_initial": True, "full_native_index_checked": True,
        "durable_tombstone": identity in tombstones, "checkpoint_tick": raw["checkpoint_tick"]}
        for identity in sorted(prior.keys() - active_ids)]
    raw["entity_records"] = raw.pop("entities")
    raw["entities"] = groups
    raw["source_world"] = before
    raw["command"] = command
    raw["identity_revision_source"] = {"path": str(revision_source), "sha256": scale.binary_sha256(revision_source),
                                     "validated_against_native_header": True}
    scale.dump(directory / "result.json", raw)
    return raw


async def public_region(args: argparse.Namespace, directory: Path, game: int,
                        region: dict[str, Any], *, save_command: str) -> dict[str, Any]:
    directory.mkdir()
    phase_args = copy.copy(args)
    # The server streams its configured radius, not the client information hint.
    # Using a smaller regional radius discards cached columns that a subsequent
    # landing teleport will not cause the server to resend.
    phase_args.view_distance = args.view_distance
    commands = {"setup": [*region["commands"], save_command]}
    scale.dump(directory / "commands.json", commands)
    scale.dump(directory / "anchors.json", [region["anchor"]])
    # Open-loop setup runs inside the total readiness deadline, before Ready.
    # Its required schedule is never started: this phase issues no Run event.
    scale.dump(directory / "schedule.json", {"windows": [{"name": "steady", "duration_ms": 1000}]})
    phase = Session(phase_args, directory)
    row: dict[str, Any] = {"region": region["name"], "commands_sha256": digest(commands)}
    try:
        command = bot_command(phase_args, directory, game, 1, {"group_size": 1}) + [
            "--commands-file", str(directory / "commands.json"), "--anchors", str(directory / "anchors.json"),
            "--no-edits", "--name-prefix", "load_setup", "--open-loop",
            "--schedule-file", str(directory / "schedule.json")]
        if region.get("street_routes"):
            scale.dump(directory / "street-routes.json", region["street_routes"])
            command += ["--street-routes", str(directory / "street-routes.json")]
        row["bot_command"] = command
        await phase.spawn("bots", command, events=True)
        row["ready"] = await phase.ready(1, args.readiness_seconds)
        row.update(await _await_fixture_checkpoint(phase, max(args.readiness_seconds, len(commands["setup"]) / 10)))
        row["process_samples"] = phase.process_samples()
    finally:
        await phase.stop()
        row["shutdown"] = phase.shutdown
        scale.dump(directory / "receipt.json", row)
    if any(item["forced"] or item["returncode"] for item in phase.shutdown.values()):
        raise ClassifiedFailure("SETUP_FAILED", "public fixture region disconnected/failed")
    return row




async def prepare_control(args: argparse.Namespace, directory: Path,
                          recipe: dict[str, Any]) -> dict[str, Any]:
    directory.mkdir()
    setup_started = time.monotonic()
    world = directory.parent / "world"
    setup_args = copy.copy(args)
    regions = recipe.get("preparation_regions") or [{"name": "fixture", "anchor": recipe["anchors"][0],
                                                    "view_distance": recipe.get("view_distance", 6),
                                                    "commands": recipe["commands"]}]
    setup_args.view_distance = max(region["view_distance"] for region in regions)
    plugins = None
    if recipe.get("required_plugins"):
        source = getattr(args, "plugins_dir", None)
        if source is None or not source.is_dir():
            raise ClassifiedFailure("INFRA_BLOCKED", f"{recipe['id']} requires --plugins-dir with real current-WIT packages: {', '.join(recipe['required_plugins'])}")
        plugins = directory / "plugins"
        plugins.mkdir()
        for name in recipe["required_plugins"]:
            package = source / name
            if not (package / "plugin.toml").is_file() or not (package / "plugin.wasm").is_file():
                raise ClassifiedFailure("INFRA_BLOCKED", f"deployment lacks required package {name}/plugin.toml or plugin.wasm")
            # Other packages may require Loader; never rewrite their manifests
            # or accidentally negotiate unrelated bundles on this vanilla lane.
            shutil.copytree(package, plugins / name)
    preparation_clients = recipe.get("preparation_clients", 1)
    path, url, game = config(setup_args, directory, world, preparation_clients, operators=True,
                             name_prefix="load_setup", plugins_dir=plugins)
    server = Session(setup_args, directory)
    receipt: dict[str, Any] = {"recipe": recipe, "recipe_sha256": digest(recipe), "regions": [],
                              "server_config": str(path), "plugins_dir": str(plugins) if plugins else None}
    if plugins is not None:
        receipt["plugin_deployment"] = tree_hash(plugins)
    try:
        await server.spawn("server", [str(args.server_bin), "--config", str(path), "--no-console"])
        await wait_dashboard(server, url, args.readiness_seconds)
        scale.dump(directory / "anchors.json", recipe["anchors"])
        for index, region in enumerate(regions):
            server.alive("SETUP_FAILED")
            receipt["regions"].append(await public_region(
                setup_args, directory / f"region-{index}", game, region, save_command="save-all flush"))
        duration = recipe.get("preparation_duration_ms", 0)
        prep_spec = mechanic_spec({"mechanic_spec": recipe.get("preparation_spec", {})})
        if duration or prep_spec.get("actions"):
            phase_dir = directory / "stock"
            phase_dir.mkdir()
            prep_spec.setdefault("actions", []).append({"kind": "command", "bot": 0, "window": "steady",
                "at_ms": duration + 100, "command": "save-all flush", "expected_feedback": "Saved the game"})
            schedule = {"windows": [{"name": "steady", "duration_ms": duration + 1000},
                                    {"name": "recovery", "duration_ms": 2000, "recovery": True}]}
            scale.dump(phase_dir / "spec.json", prep_spec)
            scale.dump(phase_dir / "schedule.json", schedule)
            scale.dump(phase_dir / "commands.json", {"setup": ["clear @s"]})
            scale.dump(phase_dir / "anchors.json", recipe.get("preparation_anchors", [recipe["anchors"][0]]))
            phase = Session(setup_args, phase_dir)
            row = {"schedule": schedule, "spec_sha256": digest(prep_spec)}
            try:
                command = bot_command(setup_args, phase_dir, game, preparation_clients,
                    {"group_size": recipe.get("preparation_group_size", 1)}) + [
                    "--name-prefix", "load_setup", "--no-edits", "--anchors", str(phase_dir / "anchors.json"),
                    "--commands-file", str(phase_dir / "commands.json"), "--open-loop",
                    "--schedule-file", str(phase_dir / "schedule.json"), "--mechanic-spec", str(phase_dir / "spec.json")]
                row["bot_command"] = command
                await phase.spawn("bots", command, events=True)
                row["ready"] = await phase.ready(preparation_clients, args.readiness_seconds)
                assert phase.processes["bots"].stdin
                phase.processes["bots"].stdin.write(b'{"event":"run"}\n')
                await phase.processes["bots"].stdin.drain()
                row.update(await _await_fixture_checkpoint(phase, duration / 1000 + args.readiness_seconds))
                row["schedule_complete"] = await _await_preparation_schedule(phase, args.readiness_seconds + 3)
                row["process_samples"] = phase.process_samples()
            finally:
                await phase.stop()
                row["shutdown"] = phase.shutdown
            report = json.loads((phase_dir / "bots-report.json").read_text())
            if recipe["id"] in catalog_controls.RECIPE_IDS:
                row["proof"] = catalog_controls.validate_preparation(report, recipe)
            else:
                settled = report.get("ready_clients") == preparation_clients and report.get("open_loop", {}).get("schedule_complete") is True
                row["proof"] = {"passed": settled, "infra_errors": [],
                    "failures": [] if settled else ["finite natural acquisition preparation did not complete"]}
            receipt["preparation"] = row
            scale.dump(phase_dir / "receipt.json", row)
            if row["proof"]["infra_errors"]:
                raise ClassifiedFailure("INFRA_BLOCKED", "; ".join(row["proof"]["infra_errors"]))
            if row["proof"]["failures"]:
                raise ClassifiedFailure("SETUP_FAILED", "; ".join(row["proof"]["failures"]))
        for index, region in enumerate(recipe.get("post_preparation_regions", [])):
            server.alive("SETUP_FAILED")
            receipt["regions"].append(await public_region(
                setup_args, directory / f"post-region-{index}", game, region, save_command="save-all flush"))
        receipt["process_samples"] = server.process_samples()
    finally:
        await server.stop()
        receipt["shutdown"] = server.shutdown
        scale.dump(directory / "receipt.json", receipt)
    if any(row["forced"] or row["returncode"] for row in server.shutdown.values()):
        raise ClassifiedFailure("SETUP_FAILED", "fixture server did not stop/save cleanly")
    receipt["world"] = str(world)
    receipt["fixture"] = tree_hash(world)
    initial_spec = mechanic_spec({"mechanic_spec": recipe.get("initial_mechanic_spec")
                                 or recipe["mechanic_spec"]}, native=True)
    receipt["native_snapshot"] = await native_snapshot(args, directory / "native", world, path, initial_spec)
    receipt["config_sha256"] = scale.binary_sha256(path)
    receipt["setup_elapsed_seconds"] = time.monotonic() - setup_started
    if recipe["id"] in catalog_mixes.IDS:
        receipt["scale"] = catalog_mixes.scale_receipt(recipe, receipt["native_snapshot"])
    scale.dump(directory / "receipt.json", receipt)
    return receipt


async def prepare(args: argparse.Namespace, directory: Path,
                  recipe: dict[str, Any] | None = None) -> dict[str, Any]:
    if recipe is not None:
        return await prepare_control(args, directory, recipe)
    directory.mkdir()
    world = directory.parent / "world"
    commands = {"setup": PLATFORM_COMMANDS}
    scale.dump(directory / "commands.json", commands)
    scale.dump(directory / "anchors.json", [[0, 0]])
    path, url, game = config(args, directory, world, 1, operators=True, name_prefix="load_setup")
    session = Session(args, directory)
    receipt: dict[str, Any] = {"commands": PLATFORM_COMMANDS, "commands_sha256": digest(commands)}
    try:
        server = [str(args.server_bin), "--config", str(path), "--no-console"]
        receipt["server_command"] = server
        await session.spawn("server", server)
        await wait_dashboard(session, url, args.readiness_seconds)
        bots = bot_command(args, directory, game, 1) + ["--commands-file", str(directory / "commands.json"),
            "--anchors", str(directory / "anchors.json"), "--no-edits", "--name-prefix", "load_setup"]
        receipt["bot_command"] = bots
        await session.spawn("bots", bots, events=True)
        receipt["ready"] = await session.ready(1, args.readiness_seconds)
        receipt.update(await _await_fixture_checkpoint(session, args.readiness_seconds))
    finally:
        await session.stop()
        receipt["shutdown"] = session.shutdown
        scale.dump(directory / "receipt.json", receipt)
    if any(row["forced"] or row["returncode"] != 0 for row in session.shutdown.values()):
        raise ClassifiedFailure("SETUP_FAILED", "fixture did not stop/save cleanly")
    receipt["setup_bot_report"] = json.loads((directory / "bots-report.json").read_text())
    # Reopen the saved fixture: decoded columns are the oracle, not /fill feedback.
    verify_dir = directory / "verify"
    verify_dir.mkdir()
    verify_path, verify_url, verify_game = config(args, verify_dir, world, 1, operators=False)
    verifier = Session(args, verify_dir)
    try:
        await verifier.spawn("server", [str(args.server_bin), "--config", str(verify_path), "--no-console"])
        await wait_dashboard(verifier, verify_url, args.readiness_seconds)
        await verifier.spawn("bots", bot_command(args, verify_dir, verify_game, 1) +
            ["--unassisted-login", "--survival", "--no-edits", "--verify-platform", "--name-prefix", "load_verify"], events=True)
        receipt["platform_ready"] = await verifier.ready(1, args.readiness_seconds)
    finally:
        await verifier.stop()
        receipt["verify_shutdown"] = verifier.shutdown
    if any(row["forced"] or row["returncode"] != 0 for row in verifier.shutdown.values()):
        raise ClassifiedFailure("SETUP_FAILED", "platform verification did not stop/save cleanly")
    report = json.loads((verify_dir / "bots-report.json").read_text())
    receipt["proof"] = report
    if not report.get("fixture_platform_verified"):
        raise ClassifiedFailure("SETUP_FAILED", "saved fixture did not verify received safe platform")
    receipt["world"] = str(world)
    receipt["fixture"] = tree_hash(world)
    receipt["config_sha256"] = scale.binary_sha256(path)
    scale.dump(directory / "receipt.json", receipt)
    return receipt


async def login_window(session: Session, url: str, command: list[str], clients: int,
                       args: argparse.Namespace, result: dict[str, Any]) -> dict[str, Any]:
    initial = await session.http(url + "/load-metrics", 2)
    if initial.get("schema_version") != 1:
        raise ClassifiedFailure("INFRA_BLOCKED", "exact-window telemetry unavailable before login burst")
    cursor = initial.get("latest_tick")
    previous = cursor
    previous_wall_start: int | None = None
    start = time.time_ns()
    samples, queues, memory = [], [], []
    await session.spawn("bots", command, events=True)
    ready_task = asyncio.create_task(session.ready(clients, args.readiness_seconds))
    end: int | None = None
    deadline = time.monotonic() + args.readiness_seconds + 12
    raw_path = session.directory / "login-samples.jsonl"
    try:
        with raw_path.open("w", buffering=1) as raw:
            while True:
                if time.monotonic() >= deadline:
                    raise ClassifiedFailure("INFRA_BLOCKED", "login window has no post-end completion marker")
                metric = await session.http(
                    url + f"/load-metrics?after_tick={cursor or 0}&wait=true&min_samples=1", 12)
                processes = session.process_samples()
                raw.write(json.dumps({"metrics": metric, "processes": processes}) + "\n")
                if (metric.get("overrun") or
                        metric.get("instance_started_unix_ns") != initial.get("instance_started_unix_ns") or
                        metric.get("source_skipped_ticks_total") != initial.get("source_skipped_ticks_total")):
                    result["infra_errors"].append("login burst telemetry overwritten/reset/lost samples")
                for sample in metric.get("samples", []):
                    if previous is not None and sample["tick"] != previous + 1:
                        result["infra_errors"].append("login burst tick sequence is not contiguous")
                    previous = cursor = sample["tick"]
                    wall_start = sample.get("wall_start_unix_ns")
                    if not isinstance(wall_start, int) or wall_start <= 0:
                        result["infra_errors"].append("login burst timing lacks source wall timestamp")
                    elif previous_wall_start is not None and wall_start <= previous_wall_start:
                        result["infra_errors"].append("login burst source wall clock regressed")
                    previous_wall_start = wall_start
                    if isinstance(wall_start, int) and wall_start >= start:
                        samples.append(sample)
                queues.append(metric.get("queues"))
                memory.append(processes)
                if ready_task.done() and end is None:
                    result["ready"] = await ready_task
                    end = result["ready"].get("ready_unix_ns")
                    if not isinstance(end, int) or end <= start:
                        raise ClassifiedFailure("INFRA_BLOCKED", "ready event lacks a valid producer wall timestamp")
                if end is not None and any(s.get("wall_start_unix_ns", 0) >= end for s in metric.get("samples", [])):
                    break
    finally:
        if not ready_task.done():
            ready_task.cancel()
        await asyncio.gather(ready_task, return_exceptions=True)
    assert end is not None
    if "--unassisted-login" in command:
        release = session.login_release or {}
        start = release.get("unix_ns")
        if not isinstance(start, int) or start <= 0 or start >= end or release.get("clients") != clients:
            raise ClassifiedFailure("INFRA_BLOCKED", "missing exact common login-release producer timestamp")
        result["login_release"] = release
    samples = [s for s in samples if start <= s["wall_start_unix_ns"] < end]
    queues = [q for q in queues if q is None or start <= q["captured_unix_ns"] < end]
    elapsed = (end - start) / 1e9
    result["login_elapsed_seconds"] = elapsed
    valid = bool(samples) and all(isinstance(s.get("wall_us"), int) and
        s.get("wall_start_unix_ns", 0) > 0 for s in samples)
    if not valid:
        result["infra_errors"].append("login burst has no valid full-wall tick samples")
    if not queues or any(q is None for q in queues):
        result["infra_errors"].append("login burst has no edge queue metrics")
    tick = tick_metrics(samples) if valid else None
    if tick and (tick["p99_us"] > args.max_tick_p99_ms * 1000 or tick["max_us"] > args.max_tick_max_ms * 1000):
        result["performance_errors"].append("login burst wall-tick threshold exceeded")
    sustained = sustained_tps(samples, start, end, args.sustained_seconds) if valid else None
    if sustained is not None and sustained < args.min_tps:
        result["performance_errors"].append("login burst sustained TPS below operator threshold")
    result["login_burst"] = {"start_unix_ns": start, "end_unix_ns": end,
        "tick": tick, "tps": len(samples) / elapsed,
        "sustained_tps_min": sustained, "sustained_seconds": args.sustained_seconds,
        "sustained_threshold_applied": sustained is not None,
        "edge_send_queue": queues, "memory_samples": memory,
        "raw_samples": str(raw_path), "scope": "simultaneous connections through all-client readiness; separate from post-ready action windows"}
    return metric


async def restart_readback(args: argparse.Namespace, directory: Path, world: Path,
                           fixture: dict[str, Any], recipe: dict[str, Any],
                           before: dict[str, Any], checkpoints: list[dict[str, Any]]) -> dict[str, Any]:
    """Restart the exact native path with no bots, repair commands, or regeneration."""
    directory.mkdir()
    path, url, _ = config(args, directory, world, 1, operators=False,
                         plugins_dir=Path(fixture["plugins_dir"]) if fixture.get("plugins_dir") else None)
    session = Session(args, directory)
    result: dict[str, Any] = {"before": before, "world_before": tree_hash(world),
        "checkpoint": checkpoints[-1] if checkpoints else None,
        "fixture_regenerated": False, "repair_commands": [], "client_commands": [],
        "server_command": [str(args.server_bin), "--config", str(path), "--no-console"]}
    try:
        await session.spawn("server", result["server_command"])
        result["dashboard_ready"] = await wait_dashboard(session, url, args.readiness_seconds)
    finally:
        await session.stop()
        result["shutdown"] = session.shutdown
    if any(row["forced"] or row["returncode"] for row in session.shutdown.values()):
        raise ClassifiedFailure("CORRECTNESS_FAIL", "native restart did not stop/checkpoint cleanly")
    after = await native_snapshot(args, directory / "native", world, path,
                                  mechanic_spec(recipe, native=True), before)
    result["after"] = after
    result["world_after"] = tree_hash(world)
    result["same_world"] = Path(before["world"]).resolve() == Path(after["world"]).resolve() == world.resolve() and before["identity"] == after["identity"]
    if after["checkpoint_tick"] < before["checkpoint_tick"] or not result["same_world"]:
        raise ClassifiedFailure("CORRECTNESS_FAIL", "native restart changed source identity or reset committed tick")
    scale.dump(directory / "result.json", result)
    return result


def plugin_proof(directory: Path, recipe: dict[str, Any], fixture: dict[str, Any],
                 bots: dict[str, Any], windows: dict[str, Any],
                 before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    """Join received semantic replies with real, journal-ACKed host evidence."""
    required = recipe["required_plugins"]
    proof: dict[str, Any] = {"infra_errors": [], "errors": [], "successful_replies": 0,
        "windows": {}, "dashboard_before": before, "dashboard_after": after,
        "deployment": fixture.get("plugin_deployment"), "journal": {},
        "scope_notes": recipe.get("scope_notes", []),
        "durability_scope": "named public replies after preparation clean shutdown and fresh measurement process reopening the same native world"}
    absence_probes = []
    for snapshot in (before, after):
        if snapshot.get("runtime_ready") is not True or snapshot.get("loaded_packages", 0) < len(required):
            proof["infra_errors"].append("plugin: real deployed runtime/package dashboard evidence missing")
    for key in ("host_operations", "wasm_invocations", "command_results"):
        start, end = before.get(key), after.get(key)
        if type(start) is not int or type(end) is not int or end < start:
            proof["infra_errors"].append(f"plugin: missing/regressed real dashboard counter {key}")
        else:
            proof[key] = end - start
    log = re.sub(r"\x1b\[[0-9;]*m", "", (directory / "server.log").read_text(errors="replace"))
    ready = re.search(r"plugin\.runtime_ready.*loaded_packages=(\d+)", log)
    proof["runtime_ready"] = bool(ready and int(ready[1]) >= len(required))
    if not proof["runtime_ready"]:
        proof["infra_errors"].append("plugin: actual plugin.runtime_ready startup marker missing")
    # Host request IDs are NOT original slash commands. An admitted send-message
    # alone is not a successful town command.
    for line in log.splitlines():
        if "plugin.command_result" not in line:
            continue
        fields = {key: quoted or plain for key, quoted, plain in
                  re.findall(r'\b(plugin_id|tick|request|outcome)=(?:"([^"]*)"|([^\s]+))', line)}
        owner = fields.get("plugin_id")
        tick = fields.get("tick", "")
        if owner not in required or not tick.isdigit():
            continue
        name = next((name for name, window in windows.items()
            if window.get("tick_range") and window["tick_range"][0] <= int(tick) <= window["tick_range"][1]), None)
        if name is None:
            continue
        row = proof["journal"].setdefault(owner, {})
        outcome = fields.get("outcome", "unknown")
        row[outcome] = row.get(outcome, 0) + 1
        if fields.get("request") == "send-message":
            row["journaled_messages"] = row.get("journaled_messages", 0) + 1
        if outcome in ("rejected", "stale_revision"):
            refusal = {"plugin": owner, "tick": int(tick), "outcome": outcome,
                       "request": fields.get("request")}
            if (owner == "solaris-economy" and outcome == "rejected"
                    and re.fullmatch(r"pay-status-\d+", fields.get("request", ""))):
                absence_probes.append(refusal)
            else:
                proof["errors"].append(refusal)
    for owner in required:
        if not proof["journal"].get(owner):
            proof["infra_errors"].append(f"plugin: journal-ACKed plugin.command_result missing for {owner}")
    mechanics = bots.get("mechanics", {})
    replies = mechanics.get("command_replies")
    if not isinstance(replies, list):
        proof["infra_errors"].append("plugin: received literal semantic command replies missing")
        replies = []
    if mechanics.get("command_replies_truncated", 0):
        proof["infra_errors"].append("plugin: literal command reply evidence truncated")
    # Vanilla /give and /tp use translated components. Do not guess their
    # text, or count them as plugin answers; every plugin answer below must
    # still match its declared literal body, actor, window and multiplicity.
    proof["unrenderable_replies"] = mechanics.get("command_replies_unrenderable", 0)
    expected = Counter((action["bot"], action["window"], action["expected_feedback"])
        for action in recipe["mechanic_spec"]["actions"] if action.get("expected_feedback"))
    observed: Counter = Counter()
    proof["unmatched_replies"] = []
    for reply in replies:
        name, text, bot = reply.get("window"), reply.get("text"), reply.get("bot")
        if name not in windows or type(bot) is not int or not isinstance(text, str):
            continue
        key = (bot, name, text)
        if key not in expected:
            # Ordinary operator transport/item feedback is retained but is not
            # plugin success. Every declared plugin answer must still arrive.
            proof["unmatched_replies"].append(reply)
            continue
        observed[key] += 1
        counts = proof["windows"].setdefault(name, {})
        counts[str(bot)] = counts.get(str(bot), 0) + 1
        proof["successful_replies"] += 1
    proof["state_readbacks"] = []
    for key, count in expected.items():
        bot, phase, text = key
        row = {"bot": bot, "window": phase, "text": text,
               "expected": count, "received": observed[key]}
        proof["state_readbacks"].append(row)
        if observed[key] != count:
            proof["errors"].append(row)
    # The economy first probes whether this new payment ID already exists.
    # Its NotFound branch is a journaled refusal, not a failed payment. Accept
    # exactly one such probe only alongside the committed payment and every
    # exact balance/replay readback; any other host refusal still fails.
    proof["payment_absence_probes"] = absence_probes
    if absence_probes and (len(absence_probes) != 1 or proof["errors"]
            or observed[(0, "steady", "Paid 7 coins.")] != 1
            or proof["journal"].get("solaris-economy", {}).get("committed", 0) < 1):
        proof["errors"].extend(absence_probes)
    journaled = sum(row.get("journaled_messages", 0) for row in proof["journal"].values())
    if journaled < proof["successful_replies"]:
        proof["infra_errors"].append("plugin: received semantic replies exceed journal-ACKed plugin messages")
    proof["town_name"] = "load_matrix" if all(observed[(0, phase,
        "load_matrix: 1 members, 1 claims.")] == 1 for phase in ("steady", "burst")) else None
    proof["durable"] = proof["town_name"] is not None
    proof["oracle"] = (
        "Exact per-actor/per-window town/claim and balance state-query bodies, "
        "payment/replay outcomes and journal-ACKed per-package commits; counters are liveness only")
    return proof


async def release_cohort(args: argparse.Namespace, session: Session, game: int,
                         clients: int, burst_unix_ns: int) -> dict[str, Any]:
    """Release fresh TCP logins at the common burst slot, not during readiness."""
    loop = asyncio.get_running_loop()
    release = loop.create_future()
    timer = loop.call_at(loop.time() + max(0, (burst_unix_ns - time.time_ns()) / 1e9),
                         release.set_result, None)
    try:
        await release
    finally:
        timer.cancel()
    schedule = {"windows": [
        {"name": "burst", "duration_ms": 2000, "movement_hz": 0, "edit_hz": 0,
         "chat_hz": 1, "command_hz": 0},
        {"name": "recovery", "duration_ms": 1000, "recovery": True,
         "movement_hz": 0, "edit_hz": 0, "chat_hz": 0, "command_hz": 0}]}
    scale.dump(session.directory / "schedule.json", schedule)
    command = bot_command(args, session.directory, game, clients) + [
        "--name-prefix", "cohort", "--unassisted-login", "--survival", "--no-edits",
        "--open-loop", "--schedule-file", str(session.directory / "schedule.json")]
    row: dict[str, Any] = {"requested": clients, "scheduled_unix_ns": burst_unix_ns,
                           "released_unix_ns": time.time_ns(), "command": command}
    await session.spawn("bots", command, events=True)
    try:
        row["ready"] = await session.ready(clients, min(args.readiness_seconds, args.burst_seconds - 4))
        assert session.processes["bots"].stdin
        session.processes["bots"].stdin.write(b'{"event":"run"}\n')
        await session.processes["bots"].stdin.drain()
        row["complete"] = await _event(session, "schedule_complete", args.burst_seconds)
        row["completed_unix_ns"] = time.time_ns()
    except ClassifiedFailure as error:
        raise ClassifiedFailure("CORRECTNESS_FAIL", f"burst cohort failed: {error}") from error
    return row


async def measure(args: argparse.Namespace, scenario: dict[str, Any], directory: Path,
                  fixture: dict[str, Any], clients: int,
                  recipe: dict[str, Any] | None = None) -> dict[str, Any]:
    directory.mkdir()
    schedule = {"windows": [
        {"name": "steady", "duration_ms": round(args.steady_seconds * 1000),
         "movement_hz": 20, "edit_hz": 0, "chat_hz": 1},
        {"name": "burst", "duration_ms": round(args.burst_seconds * 1000),
         "movement_hz": 20, "edit_hz": 0, "chat_hz": 2},
        {"name": "recovery", "duration_ms": round(args.recovery_seconds * 1000), "recovery": True},
    ]}
    if recipe:
        args = copy.copy(args)
        view_distance = recipe.get("view_distance", args.view_distance)
        args.view_distance = max(args.view_distance, view_distance) if scenario["id"] in catalog_controls.RECIPE_IDS else view_distance
        for window in schedule["windows"]:
            window.update(recipe.get("schedule_overrides", {}).get(window["name"], {}))
            minimum = recipe.get("constraints", {}).get("minimum_window_ms", {}).get(window["name"], 0)
            if window["duration_ms"] < minimum:
                raise ClassifiedFailure("SETUP_FAILED", f"{window['name']} shorter than finite recipe {minimum}ms")
        minimum = recipe.get("constraints", {}).get("minimum_measurement_ms", 0)
        if sum(window["duration_ms"] for window in schedule["windows"]) < minimum:
            raise ClassifiedFailure("SETUP_FAILED", f"recipe requires at least {minimum}ms total measurement")
        scale.dump(directory / "mechanic-spec.json", mechanic_spec(recipe))
        group = min(64, recipe["group_size"])
        anchors = recipe["anchors"]
        scale.dump(directory / "anchors.json", [anchors[index % len(anchors)]
            for index in range(math.ceil(clients / group))])
    assisted = scenario["id"] == "join-ready-assisted" or scenario["id"] == "explore"
    if assisted and recipe is None:
        for window in schedule["windows"][:2]:
            window["edit_hz"] = 0 if scenario["id"] == "explore" else 1
    scale.dump(directory / "schedule.json", schedule)
    world = Path(fixture["world"])
    path, url, game = config(args, directory, world, clients, operators=assisted or recipe is not None,
                             plugins_dir=Path(fixture["plugins_dir"]) if fixture.get("plugins_dir") else None)
    session = Session(args, directory)
    cohort_count = recipe["expected"].get("cohort_clients", 0) if recipe else 0
    resident_count = clients - cohort_count
    cohort_session: Session | None = None
    cohort_task: asyncio.Task[dict[str, Any]] | None = None
    if cohort_count:
        (directory / "cohort").mkdir()
        cohort_session = Session(args, directory / "cohort")
    result: dict[str, Any] = {"clients": clients, "schedule": schedule,
        "schedule_sha256": digest(schedule), "windows": {}, "infra_errors": [],
        "correctness_errors": [], "performance_errors": [], "fixture_before": tree_hash(world)}
    raw_path = directory / "samples.jsonl"
    last_tick: int | None = None
    last_raw_tick: int | None = None
    ranges: list[tuple[str, int, int]] = []
    previous_wall_start: int | None = None
    plugin_before: dict[str, Any] = {}
    plugin_after: dict[str, Any] = {}
    try:
        server = [str(args.server_bin), "--config", str(path), "--no-console"]
        result["server_command"] = server
        await session.spawn("server", server)
        await wait_dashboard(session, url, args.readiness_seconds)
        bots = bot_command(args, directory, game, resident_count, recipe) + ["--open-loop", "--schedule-file", str(directory / "schedule.json")]
        if recipe:
            bots += ["--anchors", str(directory / "anchors.json"), "--mechanic-spec",
                     str(directory / "mechanic-spec.json"), *recipe["client_args"]]
        elif assisted:
            bots += ["--anchors", str(directory.parent / "prepare/anchors.json")]
            if scenario["id"] == "explore":
                bots += ["--no-edits", "--explore"]
        else:
            bots += ["--unassisted-login", "--survival", "--no-edits", "--verify-platform"]
        result["bot_command"] = bots
        initial = await login_window(session, url, bots, resident_count, args, result)
        if recipe and recipe.get("required_plugins"):
            plugin_before = (await session.http(url + "/stats", 2)).get("plugin") or {}
        last_tick = initial.get("latest_tick")
        last_raw_tick = last_tick
        instance = initial.get("instance_started_unix_ns")
        source_skips = initial.get("source_skipped_ticks_total")
        epoch_wall = time.time_ns()
        epoch_mono = time.monotonic()
        offset = 0
        for window in schedule["windows"]:
            start = epoch_wall + offset * 1_000_000
            offset += window["duration_ms"]
            ranges.append((window["name"], start, epoch_wall + offset * 1_000_000))
            result["windows"][window["name"]] = {"start_unix_ns": start, "end_unix_ns": ranges[-1][2],
                "tick_samples": [], "queue_samples": [], "memory_samples": []}
        proc = session.processes["bots"]
        assert proc.stdin
        proc.stdin.write(b'{"event":"run"}\n')
        await proc.stdin.drain()
        event_deadline = time.monotonic() + args.readiness_seconds
        while True:
            event = await asyncio.wait_for(session.events.get(), max(.01, event_deadline - time.monotonic()))
            if event.get("event") == "failure":
                session.alive()
                raise ClassifiedFailure("CORRECTNESS_FAIL", str(event.get("error")))
            if event.get("event") == "running":
                epoch = event.get("open_loop", {}).get("epoch_unix_ns")
                if not isinstance(epoch, int) or epoch <= 0:
                    raise ClassifiedFailure("INFRA_BLOCKED", "running event lacks shared wall epoch")
                shift = epoch - epoch_wall
                epoch_wall = epoch
                ranges = [(name, start + shift, end + shift) for name, start, end in ranges]
                for name, start, end in ranges:
                    result["windows"][name]["start_unix_ns"] = start
                    result["windows"][name]["end_unix_ns"] = end
                break
        result["run_epoch"] = {"unix_ns": epoch_wall, "monotonic_ns": round(epoch_mono * 1e9),
            "alignment": "producer shared schedule epoch"}
        if cohort_session is not None:
            cohort_task = asyncio.create_task(release_cohort(args, cohort_session, game, cohort_count,
                result["windows"]["burst"]["start_unix_ns"]))
        completion_seen = False
        stream_deadline = time.monotonic() + offset / 1000 + args.readiness_seconds
        with raw_path.open("w", buffering=1) as raw:
            while not completion_seen:
                if time.monotonic() >= stream_deadline:
                    raise ClassifiedFailure("INFRA_BLOCKED", "no completed tick starting at/after scheduled window end")
                session.alive()
                metrics = await session.http(
                    url + f"/load-metrics?after_tick={last_tick or 0}&wait=true&min_samples=20", 12)
                captured = metrics.get("captured_unix_ns", time.time_ns())
                processes = session.process_samples()
                if cohort_session is not None and "bots" in cohort_session.processes:
                    processes["cohort"] = scale.proc_sample(cohort_session.processes["bots"].pid)
                stats = await session.http(url + "/stats", 2)
                if recipe and recipe.get("required_plugins"):
                    plugin_after = stats.get("plugin") or {}
                raw.write(json.dumps({"captured_unix_ns": captured, "metrics": metrics,
                    "processes": processes, "stats": stats, "bots": session.latest_bot}) + "\n")
                if metrics.get("overrun"):
                    result["infra_errors"].append("raw tick ring overwrote requested samples")
                if metrics.get("instance_started_unix_ns") != instance:
                    result["infra_errors"].append("telemetry instance changed during measured stream")
                if metrics.get("source_skipped_ticks_total") != source_skips:
                    result["infra_errors"].append("core timing stream lost samples")
                for sample in metrics.get("samples", []):
                    tick = sample["tick"]
                    if last_raw_tick is not None and tick != last_raw_tick + 1:
                        result["infra_errors"].append(f"noncontiguous raw tick stream: {last_raw_tick}->{tick}")
                    last_raw_tick = last_tick = tick
                    wall_start = sample.get("wall_start_unix_ns")
                    if not isinstance(sample.get("wall_us"), int) or not isinstance(wall_start, int) or wall_start <= 0:
                        result["infra_errors"].append("raw tick lacks full active-iteration wall time/start")
                    elif previous_wall_start is not None and wall_start <= previous_wall_start:
                        result["infra_errors"].append("wall clock regressed; window boundaries invalid")
                    previous_wall_start = wall_start
                    if isinstance(wall_start, int) and wall_start >= ranges[-1][2]:
                        completion_seen = True
                name = next((name for name, start, end in ranges if start <= captured < end), None)
                if name:
                    window = result["windows"][name]
                    window["queue_samples"].append(metrics.get("queues"))
                    window["memory_samples"].append(processes)
                roster = stats.get("players", {}).get("count")
                if cohort_session is None:
                    roster_valid = roster == clients
                elif captured < result["windows"]["burst"]["start_unix_ns"]:
                    roster_valid = roster == resident_count
                elif captured < result["windows"]["recovery"]["start_unix_ns"]:
                    roster_valid = type(roster) is int and resident_count <= roster <= clients
                else:
                    roster_valid = roster == clients
                if not roster_valid:
                    result["correctness_errors"].append("measured server roster differs from the resident/cohort lifecycle")
                if any(row["rss_bytes"] > args.max_rss_mb * 1024 * 1024 for row in processes.values()):
                    result["performance_errors"].append("process RSS exceeds operator limit")
                    raise ClassifiedFailure(classify(infra=result["infra_errors"],
                        correctness=result["correctness_errors"], performance=result["performance_errors"]), "memory safety stop")
        if recipe and recipe["id"] in catalog_mixes.IDS and session.live_complete is None:
            await _event(session, "schedule_complete", args.readiness_seconds)
        if cohort_task is not None:
            result["cohort"] = await cohort_task
    except (ClassifiedFailure, OSError, ValueError, KeyError, asyncio.TimeoutError) as error:
        result["outcome"] = error.outcome if isinstance(error, ClassifiedFailure) else "INFRA_BLOCKED"
        result["reason"] = str(error) if isinstance(error, ClassifiedFailure) else f"{type(error).__name__}: {error}"
        result["measurement_complete"] = False
        for window in [result.get("login_burst"), *result["windows"].values()]:
            if window is not None:
                window["valid"] = False
                window["measurement_outcome"] = result["outcome"]
                window["invalid_reasons"] = [f"incomplete measurement: {result['reason']}"]
        return result
    finally:
        if cohort_task is not None and not cohort_task.done():
            cohort_task.cancel()
            await asyncio.gather(cohort_task, return_exceptions=True)
        if cohort_session is not None:
            await cohort_session.stop()
            result["cohort_shutdown"] = cohort_session.shutdown
        await session.stop()
        result["shutdown"] = session.shutdown
        result["raw_samples"] = str(raw_path)
        result["config_sha256"] = scale.binary_sha256(path)
        result["command_sha256"] = digest({"server": result.get("server_command"),
            "bots": result.get("bot_command"), "schedule": schedule})
        scale.dump(directory / "result.json", result)
    report = json.loads((directory / "bots-report.json").read_text())
    result["bots"] = report
    if cohort_session is not None:
        result.setdefault("cohort", {})["bots"] = json.loads((cohort_session.directory / "bots-report.json").read_text())
    if recipe and recipe["id"] in catalog_mixes.IDS:
        result["initial_native_snapshot"] = fixture.get("native_snapshot", {})
        result["live_complete"] = session.live_complete
    # Reconcile the shared bot epoch before assigning samples to windows.
    bot_epoch = report.get("open_loop", {}).get("epoch_unix_ns")
    if isinstance(bot_epoch, int):
        shift = bot_epoch - result["run_epoch"]["unix_ns"]
        ranges = [(name, start + shift, end + shift) for name, start, end in ranges]
    else:
        result["infra_errors"].append("bot report lacks shared wall epoch for exact-window alignment")
    for name, start, end in ranges:
        result["windows"][name]["start_unix_ns"] = start
        result["windows"][name]["end_unix_ns"] = end
    with raw_path.open() as raw:
        for line in raw:
            row = json.loads(line)
            for sample in row["metrics"].get("samples", []):
                at = sample.get("wall_start_unix_ns")
                name = next((n for n, start, end in ranges if isinstance(at, int) and start <= at < end), None)
                if name:
                    result["windows"][name]["tick_samples"].append(sample)
    for name, window in result["windows"].items():
        samples = window.pop("tick_samples")
        if not samples or any(not isinstance(s.get("wall_us"), int) for s in samples):
            result["infra_errors"].append(f"{name}: no complete raw wall tick samples")
            window["tick"] = None
        else:
            window["tick"] = tick_metrics(samples)
            elapsed = (window["end_unix_ns"] - window["start_unix_ns"]) / 1e9
            # Completed executions, not tick numbers that the core may skip.
            window["tps"] = len(samples) / elapsed
            window["sustained_tps_min"] = sustained_tps(samples, window["start_unix_ns"],
                window["end_unix_ns"], args.sustained_seconds)
            window["sustained_seconds"] = args.sustained_seconds
            window["tick_range"] = [samples[0]["tick"], samples[-1]["tick"]]
            if any(b["tick"] != a["tick"] + 1 for a, b in zip(samples, samples[1:])):
                result["infra_errors"].append(f"{name}: noncontiguous tick IDs")
            if window["tps"] < args.min_tps or (window["sustained_tps_min"] is not None and
                    window["sustained_tps_min"] < args.min_tps):
                result["performance_errors"].append(f"{name}: average/sustained TPS below {args.min_tps}")
            if window["tick"]["p99_us"] > args.max_tick_p99_ms * 1000:
                result["performance_errors"].append(f"{name}: tick p99 exceeds operator threshold")
            if window["tick"]["max_us"] > args.max_tick_max_ms * 1000:
                result["performance_errors"].append(f"{name}: tick max exceeds operator threshold")
        queues = window.pop("queue_samples")
        if not queues or any(q is None for q in queues):
            result["infra_errors"].append(f"{name}: edge queue telemetry unavailable")
        window["edge_send_queue"] = {"samples": len(queues), "snapshots": queues,
            "max_queued_or_reserved_slots": max((sum(q[k]["queued_or_reserved_slots"] for k in ("reliable", "world"))
                for q in queues if q is not None), default=None),
            "scope": "real queued-or-reserved slots; bytes unsupported, never inferred"}
        memory = window.pop("memory_samples")
        window["memory"] = {name: {"rss_max_bytes": max((m[name]["rss_bytes"] for m in memory), default=None),
            "rss_lifetime_peak_bytes": max((m[name]["peak_rss_bytes"] for m in memory), default=None),
            "rss_final_bytes": memory[-1][name]["rss_bytes"] if memory else None}
            for name in ("server", "bots")}
    for name, row in session.shutdown.items():
        if row["forced"] or row["returncode"] != 0:
            result["correctness_errors"].append(f"{name}: non-clean shutdown/save")
    if recipe:
        try:
            result["native_snapshot"] = await native_snapshot(args, directory / "native", world, path,
                mechanic_spec(recipe, native=True), fixture.get("native_snapshot"))
            if recipe.get("restart_check") or recipe.get("lifecycle_hooks"):
                result["restart"] = await restart_readback(args, directory / "restart", world, fixture,
                    recipe, result["native_snapshot"], session.checkpoints)
        except ClassifiedFailure as error:
            errors = result["infra_errors"] if error.outcome == "INFRA_BLOCKED" else result["correctness_errors"]
            errors.append(str(error))
        if recipe.get("required_plugins"):
            result["plugin"] = plugin_proof(directory, recipe, fixture, report,
                                             result["windows"], plugin_before, plugin_after)
        result["work_done"] = catalog_controls.validate(scenario["id"], result, recipe) if scenario["id"] in catalog_controls.RECIPE_IDS else catalog_mixes.validate(scenario["id"], result, recipe)
        if recipe["id"] in catalog_mixes.IDS:
            result["scale"] = catalog_mixes.scale_receipt(recipe, result.get("native_snapshot", {}),
                report.get("ready_clients", 0) + result.get("cohort", {}).get("bots", {}).get("ready_clients", 0))
        result["infra_errors"].extend(result["work_done"].get("infra_errors", []))
    else:
        result["work_done"] = work_proof(scenario["id"], report, clients)
    if not result["work_done"]["passed"]:
        result["correctness_errors"].extend(result["work_done"]["failures"])
    result["fixture_after"] = tree_hash(world)
    result["outcome"] = classify(infra=result["infra_errors"], correctness=result["correctness_errors"], performance=result["performance_errors"])
    result["measurement_complete"] = True
    for window in [result["login_burst"], *result["windows"].values()]:
        window["valid"] = not result["infra_errors"]
        window["measurement_outcome"] = "INFRA_BLOCKED" if result["infra_errors"] else result["outcome"]
        if result["infra_errors"]:
            window["invalid_reasons"] = result["infra_errors"]
    scale.dump(directory / "result.json", result)
    return result


def work_proof(scenario: str, report: dict[str, Any], clients: int) -> dict[str, Any]:
    failures = []
    schedule = report.get("open_loop", {})
    if schedule:
        if not schedule.get("schedule_complete"):
            failures.append("fixed wall schedule did not complete")
        for window in schedule.get("windows", []):
            if window.get("recovery") and any(window.get(kind, {}).get("offered", 0) or
                    window.get(kind, {}).get("admitted", 0) for kind in ("movement", "edit", "command", "chat")):
                failures.append("recovery window offered/admitted gameplay demand")
        if report.get("chat_echoes_received", 0) < report.get("chats_sent", 0):
            failures.append("admitted ordinary chats did not drain to received echoes")
    if report.get("ready_clients", report.get("clients")) != clients:
        failures.append("final bot readiness differs from requested count")
    if report.get("disconnects", 0):
        failures.append("client disconnected")
    if report.get("chunks_received", 0) < clients:
        failures.append("no received/decoded chunk work for all clients")
    if scenario == "JOIN-01":
        echoes = report.get("chat_echo_clients", 0)
        if echoes != clients:
            failures.append("not every survival login completed a server-confirmed first chat action")
    elif scenario == "join-ready-assisted":
        for key in ("break_updates_observed", "place_updates_observed"):
            if report.get(key, 0) <= 0:
                failures.append(f"missing received work proof: {key}")
    return {"passed": not failures, "failures": failures,
        "oracle": "received wire state/echo, not command submission or packet-write counts",
        "completed": {k: report.get(k) for k in ("chat_echo_clients", "chunks_received", "break_updates_observed",
            "place_updates_observed", "chunk_boundaries_crossed", "walking_distance_blocks")},
        "open_loop": report.get("open_loop")}


async def run(args: argparse.Namespace) -> int:
    task = asyncio.current_task()
    assert task is not None
    asyncio.get_running_loop().add_signal_handler(signal.SIGTERM, task.cancel)
    args.out.mkdir(parents=True)
    mapping = json.loads(MAPPING.read_text())
    entries = {s["id"]: s for s in mapping["scenarios"]}
    selected = args.scenario or list(entries)
    unknown = set(selected) - entries.keys()
    if unknown:
        raise ValueError(f"unknown scenario IDs: {sorted(unknown)}")
    summary: dict[str, Any] = {"schema_version": 1, "catalog_status": "DESIGN_ONLY",
        "smoke_only": args.smoke, "performance_evidence": False, "mapping_sha256": scale.binary_sha256(MAPPING),
        "command": [sys.executable, "-m", "tools.harness.backends.catalog_load", *sys.argv[1:]],
        "seed": args.seed, "threshold_source": "operator Main 2026-10-13; catalog has no numeric SLO",
        "thresholds": {"min_tps": args.min_tps, "max_tick_p99_ms": args.max_tick_p99_ms,
            "max_tick_max_ms": args.max_tick_max_ms, "max_rss_mb": args.max_rss_mb,
            "sustained_seconds": args.sustained_seconds}, "results": []}
    for scenario_id in selected:
        scenario = entries[scenario_id]
        directory = args.out / scenario_id
        directory.mkdir()
        receipt: dict[str, Any] = {"schema_version": 1, "scenario": scenario, "seed": args.seed,
            "catalog_closure": scenario.get("catalog_closure", True), "smoke_only": args.smoke,
            "performance_evidence": False, "mapping_sha256": summary["mapping_sha256"]}
        try:
            recipe = catalog_controls.recipe(scenario_id, args.clients, args.smoke) or catalog_mixes.recipe(scenario_id, args.clients, args.smoke)
            if recipe is not None:
                receipt["recipe_sha256"] = digest(recipe)
                if recipe["id"] in catalog_mixes.IDS:
                    receipt["scale"] = catalog_mixes.scale_receipt(recipe, {})
            capability = (recipe or {}).get("capability", scenario.get("capability", {}))
            if recipe and scenario_id in catalog_controls.RECIPE_IDS:
                capability = recipe.get("capability", {})
            if capability.get("outcome"):
                raise ClassifiedFailure(capability["outcome"], capability["reason"])
            if recipe is not None:
                minimum = recipe.get("constraints", {}).get("minimum_clients", 1)
                if min(args.ramp_clients or [args.clients]) < minimum:
                    raise ClassifiedFailure("SETUP_FAILED", f"recipe requires at least {minimum} clients for its declared regions")
            receipt["worldgen_gpu"] = args.worldgen_gpu
            binaries = {"server": args.server_bin, "bots": args.bots_bin}
            for name, binary in binaries.items():
                if binary is None or not binary.is_file() or not os.access(binary, os.X_OK):
                    raise ClassifiedFailure("INFRA_BLOCKED", f"missing/nonexecutable explicit release {name} binary: {binary}")
            receipt["binaries"] = {name: {"path": str(binary), "sha256": scale.binary_sha256(binary)} for name, binary in binaries.items()}
            receipt["registry_hashes"] = tree_hash(args.vanilla_data_dir)
            fixture = await prepare(args, directory / "prepare", recipe)
            receipt["fixture"] = fixture
            if "scale" in fixture:
                receipt["scale"] = fixture["scale"]
            if recipe is not None:
                receipt["recipe_sha256"] = digest(recipe)
            receipt["ramp"] = []
            ladder = args.ramp_clients or [args.clients]
            pristine = directory / "pristine-world"
            shutil.copytree(Path(fixture["world"]), pristine)
            for index, clients in enumerate(ladder):
                if index:
                    # Restore only this harness-owned path; preserve native path identity.
                    shutil.rmtree(Path(fixture["world"]))
                    shutil.copytree(pristine, Path(fixture["world"]))
                rung_recipe = catalog_controls.recipe(scenario_id, clients, args.smoke) or catalog_mixes.recipe(scenario_id, clients, args.smoke)
                result = await measure(args, scenario, directory / f"rung-{index + 1}-{clients}", fixture, clients, rung_recipe)
                receipt["ramp"].append({"clients": clients, "outcome": result["outcome"],
                    "receipt": str(directory / f"rung-{index + 1}-{clients}/result.json")})
                receipt["outcome"] = result["outcome"]
                if "scale" in result:
                    receipt["scale"] = result["scale"]
                if result["outcome"] != "PASS":
                    break
        except ClassifiedFailure as error:
            receipt["outcome"], receipt["reason"] = error.outcome, str(error)
        except (OSError, ValueError, KeyError, asyncio.TimeoutError) as error:
            receipt["outcome"], receipt["reason"] = "INFRA_BLOCKED", f"{type(error).__name__}: {error}"
        receipt["command_sha256"] = digest(summary["command"])
        scale.dump(directory / "result.json", receipt)
        summary["results"].append({"scenario": scenario_id, "outcome": receipt["outcome"], "receipt": str(directory / "result.json")})
        print(json.dumps({"event": "scenario", **summary["results"][-1]}), flush=True)
    summary["outcome"] = "PASS" if all(r["outcome"] == "PASS" for r in summary["results"]) else "MIXED"
    scale.dump(args.out / "result.json", summary)
    print(json.dumps({"event": "final", "outcome": summary["outcome"], "receipt": str(args.out / "result.json")}), flush=True)
    # Blocked/unsupported are recorded outcomes, not a crashed backend. Failed implemented gates are errors.
    return int(any(r["outcome"] in {"SETUP_FAILED", "CORRECTNESS_FAIL", "PERF_FAIL"} for r in summary["results"]))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--server-bin", type=Path)
    parser.add_argument("--bots-bin", type=Path)
    parser.add_argument("--plugins-dir", type=Path,
                        help="Real current-WIT v2 packages for plugin-heavy; never synthesized")
    parser.add_argument("--vanilla-data-dir", type=Path, default=ROOT / "data/vanilla")
    parser.add_argument("--clients", type=int, default=10)
    parser.add_argument("--ramp-clients", type=int, nargs="+")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--view-distance", type=int, default=4)
    parser.add_argument("--wire-trace-bots", type=int, nargs="+",
                        help="Opt-in per-bot command/position/confirm/movement/action JSONL diagnostics")
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--steady-seconds", type=float, default=20)
    parser.add_argument("--burst-seconds", type=float, default=10)
    parser.add_argument("--recovery-seconds", type=float, default=20)
    parser.add_argument("--readiness-seconds", type=int, default=30)
    parser.add_argument("--min-tps", type=float, default=19.5)
    parser.add_argument("--sustained-seconds", type=int, default=5)
    parser.add_argument("--max-tick-p99-ms", type=float, default=50)
    parser.add_argument("--max-tick-max-ms", type=float, default=250)
    parser.add_argument("--max-rss-mb", type=float, default=3072)
    parser.add_argument("--worldgen-gpu", action="store_true",
                        help="generate the server's chunk density on the GPU ([server].worldgen_gpu)")
    args = parser.parse_args()
    if args.out.exists():
        parser.error("--out must be a new directory; existing receipts are never overwritten")
    if args.scenario and len(args.scenario) != len(set(args.scenario)):
        parser.error("--scenario IDs must not be repeated")
    if not 1 <= args.clients <= 4096 or not 1 <= args.view_distance <= 32:
        parser.error("clients must be 1..4096 and view-distance 1..32")
    if args.ramp_clients and (any(not 1 <= n <= 4096 for n in args.ramp_clients) or args.ramp_clients != sorted(set(args.ramp_clients))):
        parser.error("ramp-clients must be strictly increasing within 1..4096")
    numbers = (args.steady_seconds, args.burst_seconds, args.recovery_seconds, args.min_tps,
               args.max_tick_p99_ms, args.max_tick_max_ms, args.max_rss_mb, args.readiness_seconds, args.sustained_seconds)
    if any(not math.isfinite(n) or n <= 0 for n in numbers):
        parser.error("durations and thresholds must be finite and positive")
    if args.smoke and args.steady_seconds + args.burst_seconds + args.recovery_seconds > 120:
        parser.error("smoke measurement windows must total <=120 seconds")
    args.out = args.out.resolve()
    args.vanilla_data_dir = args.vanilla_data_dir.resolve()
    for name in ("server_bin", "bots_bin", "plugins_dir"):
        value = getattr(args, name)
        if value:
            setattr(args, name, value.resolve())
    return asyncio.run(run(args))


if __name__ == "__main__":
    raise SystemExit(main())
