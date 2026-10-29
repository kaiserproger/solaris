"""Native-v2 public-wire scale and normal save/reopen durability gates.

Canonical runs remain owner-gated. --setup-smoke is explicitly tiny setup-only
validation, never a capacity/performance receipt. The owner dropped the previous
core baseline on 2026-10-16; this tool performs no A/B comparisons.
Exact /load-metrics executions are assigned by source wall-start time to each
producer window. Operator TPS, sustained-TPS, tick-tail and RSS limits fail closed;
rolling dashboard percentiles never classify a performance pass.
Default CPU affinity assigns server 0-7 (four physical SMT pairs) and bots 8-11
(two physical pairs); both are explicit, overridable and recorded per run.
"""
from __future__ import annotations

import argparse
import asyncio
import copy
import json
import math
import os
from pathlib import Path
import shlex
import shutil
import signal
import tempfile
import time
from typing import Any

from tools.harness.backends import catalog_load as load

ROOT = load.ROOT


MOB_KINDS = ("cow", "sheep", "pig", "chicken", "zombie", "skeleton", "spider", "creeper")
COHORT_NAME = json.dumps("solaris-coreperf-fixture")


def fixture(mobs: int, villagers: int) -> dict[str, Any]:
    """Equal, typed public-command cohorts; no fixed UUIDs or private seeding."""
    population = {kind: mobs // len(MOB_KINDS) + (index < mobs % len(MOB_KINDS))
                  for index, kind in enumerate(MOB_KINDS)}
    regions = []
    rules = ["gamerule doMobSpawning false", "gamerule doWeatherCycle false",
             "gamerule randomTickSpeed 3", "gamerule doDaylightCycle true",
             "weather clear", "time set 2000", "difficulty easy", "setworldspawn 32 64 32"]
    # Clear only pre-fixture natural spawns, before any measured cohort exists.
    rules += [f"kill @e[type=minecraft:{kind}]" for kind in (*MOB_KINDS, "villager")]
    group = 0
    columns = math.ceil(math.sqrt(sum(math.ceil(count / 1024) for count in population.values())))
    for kind, count in population.items():
        for offset in range(0, count, 1024):
            x, z = (group % columns) * 96, (group // columns) * 96
            commands = [f"fill {x - 8} 62 {z - 8} {x + 72} 62 {z + 72} minecraft:dirt",
                        f"fill {x - 8} 63 {z - 8} {x + 72} 63 {z + 72} minecraft:grass_block"]
            for dx in (0, 40):
                for dz in (0, 40):
                    commands.append(f"fill {x + dx - 8} 64 {z + dz - 8} {x + dx + 32} 70 {z + dz + 32} minecraft:air")
            # Species-separated roofed pens prevent sunlight burning, villager
            # predation, crossfire and spider escape. Creative observers cannot
            # trigger creepers. No NoAI, invulnerability, healing or refill.
            # Spiders still take damage after detaching from a wall. Keep their
            # climb/drop below three blocks rather than disabling natural damage.
            ceiling = 67 if kind == "spider" else 71
            commands += [f"fill {x - 9} 64 {z - 9} {x + 73} {ceiling - 1} {z - 9} minecraft:stone",
                         f"fill {x - 9} 64 {z + 73} {x + 73} {ceiling - 1} {z + 73} minecraft:stone",
                         f"fill {x - 9} 64 {z - 8} {x - 9} {ceiling - 1} {z + 72} minecraft:stone",
                         f"fill {x + 73} 64 {z - 8} {x + 73} {ceiling - 1} {z + 72} minecraft:stone",
                         f"fill {x - 9} {ceiling} {z - 9} {x + 73} {ceiling} {z + 73} minecraft:stone"]
            if not group:
                commands = rules + commands
                commands.append("setblock 32 64 34 minecraft:crafting_table")
            tag = f" {{CustomName:'{COHORT_NAME}',PersistenceRequired:1b}}"
            commands.extend(
                f"summon minecraft:{kind} {x + (n % 32) * 2 + .5} 64 {z + (n // 32) * 2 + .5}{tag}"
                for n in range(min(1024, count - offset)))
            regions.append({"name": f"{kind}-{offset // 1024}", "anchor": [x + 32, z + 32],
                            "view_distance": 8, "commands": commands})
            group += 1
    commands = ["fill -104 62 -8 -24 62 72 minecraft:dirt",
                "fill -104 63 -8 -24 63 72 minecraft:grass_block",
                "fill -104 64 -8 -64 70 32 minecraft:air", "fill -64 64 -8 -24 70 32 minecraft:air",
                "fill -104 64 32 -64 70 72 minecraft:air", "fill -64 64 32 -24 70 72 minecraft:air",
                "fill -105 64 -9 -23 66 -9 minecraft:stone", "fill -105 64 73 -23 66 73 minecraft:stone",
                "fill -105 64 -8 -105 66 72 minecraft:stone", "fill -23 64 -8 -23 66 72 minecraft:stone"]
    for n in range(villagers):
        x, z = -96 + n % 8 * 8, n // 8 * 8
        commands.extend([f"setblock {x} 64 {z} minecraft:composter",
                         f"setblock {x + 2} 64 {z} minecraft:red_bed[facing=east,part=foot]",
                         f"setblock {x + 3} 64 {z} minecraft:red_bed[facing=east,part=head]",
                         f"summon minecraft:villager {x + .5} 64 {z + 2.5} {{CustomName:'{COHORT_NAME}'}}"])
    commands += ["fill -96 63 76 -88 63 84 minecraft:farmland[moisture=7]",
                 "setblock -92 63 80 minecraft:water", "fill -96 64 76 -88 64 84 minecraft:wheat[age=0]",
                 "setblock -92 64 80 minecraft:air"]
    regions.append({"name": "village-crops", "anchor": [-64, 32], "view_distance": 8, "commands": commands})
    return {"schema_version": 1, "kind": "native-v2-public-commands",
            "seed": 42, "expected": {"minecraft:" + kind: count for kind, count in population.items() if count}
            | {"minecraft:villager": villagers},
            "cohort_scope": "exact typed CustomName-marked fixture UUIDs; natural entities remain outside the cohort",
            "regions": regions, "assistance": "operator roofed pens/POIs/crops/summons; creative gameplay is not earned survival",
            "equivalence": "same seed, public commands and typed cohort across independent native-v2 runs",
            "persistence": "all eight mob kinds use PersistenceRequired:1b",
            "variant": "v2-persistent-eight-kinds",
            "no_hidden_relief": "natural AI, health, cramming, movement, random ticks and daylight; no refill"}


def verify_resume(args: argparse.Namespace) -> None:
    if args.setup_smoke:
        if args.clients > 2 or args.mobs > 8 or args.villagers > 2 or any(
                seconds > 2 for seconds in (args.steady_seconds, args.burst_seconds, args.recovery_seconds)):
            raise ValueError("setup smoke is limited to 2 clients, 8 mixed mobs, 2 villagers, <=2s per window")
        if args.warmup_ticks != 0 or args.measure_ticks != 0:
            raise ValueError("setup smoke requires --warmup-ticks 0 --measure-ticks 0")
        return
    if args.prepare_only:
        return
    if args.warmup_ticks < 200 or args.measure_ticks < 1200:
        raise ValueError("canonical comparison requires >=200 warmup and >=1200 measured source ticks")
    if not args.main_go_receipt:
        raise ValueError("current Main exclusive GO is required")
    if not args.main_go_receipt.read_text().strip():
        raise ValueError("empty Main exclusive GO receipt")


def schedule(args: argparse.Namespace) -> dict[str, Any]:
    return {"windows": [
        {"name": name, "duration_ms": round(seconds * 1000),
         "movement_hz": 0 if name == "recovery" else (40 if name == "burst" else 20),
         "edit_hz": 0 if name == "recovery" else 2, "command_hz": 0,
         "chat_hz": 0 if name == "recovery" else 1, "recovery": name == "recovery"}
        for name, seconds in (("steady", args.steady_seconds), ("burst", args.burst_seconds),
                              ("recovery", args.recovery_seconds))]}


def gameplay(recipe: dict[str, Any]) -> dict[str, Any]:
    """One finite craft and one nonlethal bare-hand hit; all bots dig/place."""
    actions = []

    def action(kind: str, window: str, at: int, **fields: Any) -> None:
        actions.append({"kind": kind, "bot": 0, "window": window, "at_ms": at, **fields})

    action("command", "steady", 100, command="give @s minecraft:oak_log 1")
    action("command", "steady", 200, command="tp @s 32.5 64 32.5")
    action("open_container", "steady", 400, position=[32, 64, 34])
    action("click_slot", "steady", 700, slot_item="minecraft:oak_log", button=0, mode=0)
    action("click_slot", "steady", 1000, slot=1, button=0, mode=0)
    action("click_slot", "steady", 1300, slot=0, button=0, mode=1)
    action("close_container", "steady", 1600)
    action("command", "burst", 0, command="tp @s @e[type=minecraft:cow,sort=nearest,limit=1]")
    action("select_slot", "burst", 250, slot=7)
    action("attack_entity", "burst", 500, entity_kind="minecraft:cow")
    action("select_slot", "burst", 750, slot=0)
    action("command", "burst", 1000, command="tp @s 32.5 64 32.5")
    return {"schema_version": 1,
            "entities": [{"kind": "minecraft:cow", "count": recipe["expected"]["minecraft:cow"]}],
            "blocks": [{"name": "crafting_table", "position": [32, 64, 34]}],
            "containers": [{"name": "crafting", "position": [32, 64, 34], "bot": 0, "slots": 10}],
            "actions": actions, "action_hz": 8, "burst_action_hz": 8}


def gameplay_work(report: dict[str, Any]) -> dict[str, Any]:
    mechanics = report.get("mechanics", {})
    actions = mechanics.get("actions", [])
    completed = {kind: sum(row.get("completed", 0) for row in actions if row.get("kind") == kind)
                 for kind in ("open_container", "click_slot", "attack_entity")}
    inventory = mechanics.get("players", [{}])[0].get("final_inventory") or []
    planks = sum(slot.get("count", 0) for slot in inventory if slot.get("name") == "minecraft:oak_planks")
    work = {"breaks": report.get("break_updates_observed", 0),
            "placements": report.get("place_updates_observed", 0),
            "completed_actions": completed, "crafted_planks_received": planks}
    if (not work["breaks"] or not work["placements"] or planks != 4
            or completed != {"open_container": 1, "click_slot": 3, "attack_entity": 1}):
        raise load.ClassifiedFailure("CORRECTNESS_FAIL", f"real gameplay work missing: {work}")
    return work


def entities(snapshot: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {row["uuid"]: {"kind": kind, "position": row["final_position"],
                          "custom_name": row.get("payload", {}).get("CustomName")}
            for kind, group in snapshot["entities"].items()
            for row in group["samples"] if row.get("active")}


def cohort(snapshot: dict[str, Any], expected: dict[str, int]) -> dict[str, Any]:
    rows = {identity: row for identity, row in entities(snapshot).items()
            if row["kind"] in expected and row["custom_name"] == COHORT_NAME}
    counts = {kind: sum(row["kind"] == kind for row in rows.values()) for kind in expected}
    if counts != expected:
        raise load.ClassifiedFailure("SETUP_FAILED", f"typed fixture cohort {counts} differs from {expected}")
    return {"counts": counts, "identities": rows}


class CoreSession(load.Session):
    def __init__(self, args: argparse.Namespace, directory: Path, config: Path):
        super().__init__(args, directory)
        self.config = config

    async def start_server(self) -> None:
        command = [str(self.args.server_bin), "--config", str(self.config), "--no-console"]
        command = [*shlex.split(getattr(self.args, "server_wrapper", "") or ""),
                   "taskset", "-c", self.args.server_cpus, *command]
        load.scale.dump(self.directory / "server-command.json", command)
        await self.spawn("server", command)

    async def stats(self, url: str) -> dict[str, Any]:
        return await self.http(url + "/stats", 2)

    async def await_server(self, url: str) -> dict[str, Any]:
        deadline = time.monotonic() + self.args.readiness_seconds
        while time.monotonic() < deadline:
            self.alive("SETUP_FAILED")
            try:
                row = await self.stats(url)
                if row and row.get("simulation_tick", 0) > 0:
                    return row
            except load.ClassifiedFailure:
                # Dashboard sockets may not exist while ordinary startup is running.
                self.alive("SETUP_FAILED")
            await asyncio.sleep(.1)
        raise load.ClassifiedFailure("SETUP_FAILED", "server startup deadline")

    async def stop(self) -> None:
        # The ordinary final checkpoint may take longer than a bot disconnect.
        # Keep it bounded, but never turn a forced final-save exit into success.
        server = self.processes.pop('server', None)
        try:
            await super().stop()
            if server is not None:
                forced = False
                if server.returncode is None:
                    os.killpg(server.pid, signal.SIGTERM)
                    try:
                        await asyncio.wait_for(server.wait(), self.args.readiness_seconds)
                    except asyncio.TimeoutError:
                        forced = True
                        os.killpg(server.pid, signal.SIGKILL)
                        await server.wait()
                self.shutdown['server'] = {'returncode': server.returncode, 'forced': forced}
        finally:
            if server is not None:
                self.processes['server'] = server


def configure(args: argparse.Namespace, directory: Path, world: Path,
              clients: int, prefix: str) -> tuple[Path, str, int]:
    return load.config(args, directory, world, clients, operators=True, name_prefix=prefix)


def source_tick(stats: dict[str, Any]) -> int:
    tick = stats.get("tick") or {}
    return stats.get("measurement_tick", tick.get("source_tick", stats["simulation_tick"]))


async def snapshot(args: argparse.Namespace, directory: Path, world: Path,
                   config: Path) -> dict[str, Any]:
    return await load.native_snapshot(args, directory / 'native', world, config,
                                      {"entities": [], "blocks": [], "containers": []})


def clean_shutdown(session: CoreSession) -> None:
    if any(row['forced'] or row['returncode'] != 0 for row in session.shutdown.values()):
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', f'non-clean shutdown/final save: {session.shutdown}')


def scope_memory() -> dict[str, int]:
    relative = Path('/proc/self/cgroup').read_text().strip().split('::', 1)[1]
    group = Path('/sys/fs/cgroup' + relative)
    maximum = (group / 'memory.max').read_text().strip()
    if maximum == 'max':
        raise load.ClassifiedFailure('INFRA_BLOCKED', 'comparison requires a bounded mem-run scope')
    row = {name: int((group / filename).read_text()) for name, filename in (
        ('current_bytes', 'memory.current'), ('peak_bytes', 'memory.peak'), ('cap_bytes', 'memory.max'))}
    if row['peak_bytes'] * 4 > row['cap_bytes'] * 3:
        raise load.ClassifiedFailure('INFRA_BLOCKED', f'scope exceeds 75% memory guard: {row}')
    return row


def host_busy(cpus: str) -> float:
    selected = set()
    for part in cpus.split(','):
        bounds = part.split('-')
        selected.update(range(int(bounds[0]), int(bounds[-1]) + 1))
    names = {f'cpu{cpu}' for cpu in selected}
    total = 0
    for line in Path('/proc/stat').read_text().splitlines():
        fields = line.split()
        if fields and fields[0] in names:
            values = list(map(int, fields[1:9]))
            total += sum(values) - values[3] - values[4]
    return total / os.sysconf('SC_CLK_TCK')


CACHE_MARKER = 'COMPLETE'


def fixture_cache_key(args: argparse.Namespace, recipe: dict[str, Any], vanilla_sha: str) -> str:
    """Fixture inputs only; rebuilt server/bot binaries deliberately reuse the same world."""
    return load.digest({'schema': 1, 'mobs': args.mobs, 'villagers': args.villagers,
                        'seed': args.seed, 'view_distance': args.view_distance,
                        'recipe_sha256': load.digest(recipe), 'vanilla_data_sha256': vanilla_sha})


async def copy_tree(source: Path, destination: Path) -> None:
    process = await asyncio.create_subprocess_exec('cp', '--reflink=auto', '-a', str(source), str(destination))
    if await process.wait():
        raise load.ClassifiedFailure('INFRA_BLOCKED', f'fixture copy failed: {source} -> {destination}')


async def store_fixture(cache: Path, key: str, world: Path, receipt: dict[str, Any],
                        binaries: dict[str, str]) -> None:
    """Publish the pristine prepared world atomically: temp dir, marker last, then rename."""
    cache.mkdir(parents=True, exist_ok=True)
    entry = cache / key
    temp = Path(tempfile.mkdtemp(prefix=f'.{key}.', dir=cache))
    try:
        await copy_tree(world, temp / 'world')
        load.scale.dump(temp / 'receipt.json', {name: value for name, value in receipt.items()
                                                if name not in ('snapshot', 'cohort', 'fixture_cache')})
        load.scale.dump(temp / CACHE_MARKER, {'key': key, 'prepared_by': binaries})
        if (entry / CACHE_MARKER).is_file():
            return  # a concurrent run published the same key first
        shutil.rmtree(entry, ignore_errors=True)  # stale incomplete entry, never a complete one
        temp.rename(entry)
    finally:
        shutil.rmtree(temp, ignore_errors=True)


async def prepare_from_cache(args: argparse.Namespace, directory: Path, recipe: dict[str, Any],
                             entry: Path, key: str,
                             binaries: dict[str, str]) -> tuple[Path, dict[str, Any]]:
    """Copy a cached pristine world; fixture hash, snapshot and cohort checks run as on a fresh one."""
    world = directory.parent / 'world'
    setup_args = copy.copy(args)
    setup_args.view_distance = 8
    config, _, _ = configure(setup_args, directory, world, 1, 'load_setup')
    cached = json.loads((entry / 'receipt.json').read_text())
    prepared_by = json.loads((entry / CACHE_MARKER).read_text()).get('prepared_by')
    if cached.get('recipe_sha256') != load.digest(recipe):
        raise load.ClassifiedFailure('INFRA_BLOCKED', f'fixture cache entry {entry} recipe mismatch')
    await copy_tree(entry / 'world', world)
    receipt = dict(cached, config_sha256=load.scale.binary_sha256(config),
                   fixture_cache={'status': 'hit', 'key': key, 'entry': str(entry),
                                  'prepared_by': prepared_by, 'current': binaries})
    receipt['fixture'] = load.tree_hash(world)
    if receipt['fixture']['sha256'] != cached['fixture']['sha256']:
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', f'fixture cache entry {entry} differs from its recorded hash')
    try:
        receipt['snapshot'] = await snapshot(args, directory, world, config)
        receipt['cohort'] = cohort(receipt['snapshot'], recipe['expected'])
    except load.ClassifiedFailure as error:
        raise cache_incompatible(error, entry, prepared_by, binaries) from error
    load.scale.dump(directory / 'receipt.json', receipt)
    return world, receipt


def cache_incompatible(error: load.ClassifiedFailure, entry: Path, prepared_by: dict[str, str] | None,
                       binaries: dict[str, str]) -> load.ClassifiedFailure:
    return load.ClassifiedFailure(error.outcome, (
        f'cached fixture {entry} (prepared by {prepared_by}) unusable with current binaries {binaries}; '
        f'delete that entry or use a fresh --fixture-cache to regenerate: {error}'))


async def prepare(args: argparse.Namespace, directory: Path, recipe: dict[str, Any],
                  cache_key: str | None = None,
                  binaries: dict[str, str] | None = None) -> tuple[Path, dict[str, Any]]:
    directory.mkdir()
    cache = args.fixture_cache if cache_key else None
    if cache and (cache / cache_key / CACHE_MARKER).is_file():
        return await prepare_from_cache(args, directory, recipe, cache / cache_key, cache_key, binaries)
    world = directory.parent / 'world'
    setup_args = copy.copy(args)
    setup_args.view_distance = 8
    config, url, game = configure(setup_args, directory, world, 1, 'load_setup')
    server = CoreSession(setup_args, directory, config)
    receipt = {'recipe_sha256': load.digest(recipe), 'regions': [],
               'config_sha256': load.scale.binary_sha256(config),
               'fixture_cache': {'status': 'miss' if cache else 'disabled', 'key': cache_key,
                                 'prepared_by': binaries if cache else None}}
    try:
        await server.start_server()
        await server.await_server(url)
        for index, region in enumerate(recipe['regions']):
            server.alive('SETUP_FAILED')
            receipt['regions'].append(await load.public_region(
                setup_args, directory / f'region-{index}', game, region, save_command='save-all'))
            receipt['regions'][-1]['process_samples'].update(server.process_samples())
            receipt['regions'][-1]['server_stats'] = await server.stats(url)
            pid = server.processes['server'].pid
            receipt['regions'][-1]['server_smaps_rollup'] = {
                fields[0].removesuffix(':'): int(fields[1]) * 1024
                for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines()[1:]
                if len(fields := line.split()) == 3 and fields[2] == 'kB'}
            scope_memory()
    finally:
        await server.stop()
        receipt['shutdown'] = server.shutdown
        load.scale.dump(directory / 'receipt.json', receipt)
    clean_shutdown(server)
    receipt['fixture'] = load.tree_hash(world)
    receipt['snapshot'] = await snapshot(args, directory, world, config)
    receipt['cohort'] = cohort(receipt['snapshot'], recipe['expected'])
    load.scale.dump(directory / 'receipt.json', receipt)
    if cache:
        await store_fixture(cache, cache_key, world, receipt, binaries)
    return world, receipt


def classify_window(args: argparse.Namespace, window: dict[str, Any]) -> dict[str, Any]:
    """Apply the catalog controls' exact-window metrics and operator thresholds."""
    infra, performance = [], []
    samples = window.pop('tick_samples')
    if not samples:
        infra.append('no complete raw wall tick samples')
    else:
        window['tick'] = load.tick_metrics(samples)
        elapsed = (window['end_unix_ns'] - window['start_unix_ns']) / 1e9
        window['tps'] = len(samples) / elapsed
        window['sustained_tps_min'] = load.sustained_tps(
            samples, window['start_unix_ns'], window['end_unix_ns'], args.sustained_seconds)
        window['sustained_seconds'] = args.sustained_seconds
        window['tick_range'] = [samples[0]['tick'], samples[-1]['tick']]
        if any(b['tick'] != a['tick'] + 1 for a, b in zip(samples, samples[1:])):
            infra.append('noncontiguous tick IDs')
        if window['tps'] < args.min_tps or (
                window['sustained_tps_min'] is not None and window['sustained_tps_min'] < args.min_tps):
            performance.append(f'average/sustained TPS below {args.min_tps}')
        if window['tick']['p99_us'] > args.max_tick_p99_ms * 1000:
            performance.append('tick p99 exceeds operator threshold')
        if window['tick']['max_us'] > args.max_tick_max_ms * 1000:
            performance.append('tick max exceeds operator threshold')
    processes = window['samples']
    if not processes:
        infra.append('no process samples in finite wall window')
    else:
        window['process_peaks'] = {
            name: max(row['processes'][name]['peak_rss_bytes'] for row in processes)
            for name in ('server', 'bots')}
        if any(peak > args.max_rss_mb * 1024 * 1024 for peak in window['process_peaks'].values()):
            performance.append('process lifetime RSS exceeds operator limit')
    return {'outcome': load.classify(infra=infra, performance=performance),
            'valid': not infra, 'infra_errors': infra, 'performance_errors': performance}


async def measure(args: argparse.Namespace, directory: Path, world: Path,
                  recipe: dict[str, Any], initial: dict[str, Any]) -> dict[str, Any]:
    directory.mkdir()
    config, url, game = configure(args, directory, world, args.clients, 'scale')
    session = CoreSession(args, directory, config)
    windows = schedule(args)
    load.scale.dump(directory / 'schedule.json', windows)
    anchors = [region['anchor'] for region in recipe['regions']]
    group = max(1, min(64, args.clients // len(anchors)))
    load.scale.dump(directory / 'anchors.json', [anchors[n % len(anchors)] for n in range(math.ceil(args.clients / group))])
    load.scale.dump(directory / 'mechanic-spec.json', gameplay(recipe))
    bots = ['taskset', '-c', args.bot_cpus, *load.bot_command(args, directory, game, args.clients, {'group_size': group}),
            '--anchors', str(directory / 'anchors.json'), '--open-loop',
            '--mechanic-spec', str(directory / 'mechanic-spec.json'),
            '--schedule-file', str(directory / 'schedule.json')]
    result: dict[str, Any] = {'core': 'v2', 'clients': args.clients, 'setup_smoke': args.setup_smoke,
        'performance_evidence': False, 'schedule': windows, 'bot_command': bots, 'windows': {},
        'fixture_before': load.tree_hash(world), 'config_sha256': load.scale.binary_sha256(config),
        'timing_scope': 'exact source wall-start-aligned /load-metrics active iterations; excludes pacing sleep',
        'thresholds': {key: getattr(args, key) for key in
                       ('min_tps', 'sustained_seconds', 'max_tick_p99_ms', 'max_tick_max_ms', 'max_rss_mb')},
        'infra_errors': [], 'performance_errors': [],
        'normal_restore': False, 'measurement_complete': False}
    try:
        await session.start_server()
        restored = await session.await_server(url)
        # Public command preparation and normal shutdown happened before this new process.
        # A normal reader refusal fails startup, never reseeds.
        result['normal_restore'] = True
        await session.spawn('bots', bots, events=True)
        ready = await session.ready(args.clients, args.readiness_seconds)
        result['ready'] = ready
        start = await session.stats(url)
        ready_tick = start['simulation_tick']
        deadline = time.monotonic() + args.readiness_seconds
        while args.warmup_ticks and source_tick(start) < ready_tick + args.warmup_ticks:
            if time.monotonic() >= deadline:
                raise load.ClassifiedFailure('SETUP_FAILED', 'post-readiness warmup deadline')
            session.alive()
            if session.latest_bot.get('ready_clients', args.clients) != args.clients:
                raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'client readiness dropped during warmup')
            await asyncio.sleep(.1)
            start = await session.stats(url)
        result['source_start_tick'] = source_tick(start)
        metric_start = await session.http(url + '/load-metrics', 2)
        if metric_start.get('schema_version') != 1:
            raise load.ClassifiedFailure('INFRA_BLOCKED', 'exact-window telemetry unavailable')
        cursor = metric_start.get('latest_tick')
        previous_wall_start = None
        proc = session.processes['bots']
        assert proc.stdin
        proc.stdin.write(b'{"event":"run"}\n')
        await proc.stdin.drain()
        running = await load._event(session, 'running', args.readiness_seconds)
        epoch = running.get('open_loop', {}).get('epoch_unix_ns')
        if not isinstance(epoch, int) or epoch <= 0:
            raise load.ClassifiedFailure('INFRA_BLOCKED', 'missing producer wall epoch')
        offset = 0
        for window in windows['windows']:
            result['windows'][window['name']] = {'start_unix_ns': epoch + offset * 1_000_000,
                                                 'samples': [], 'tick_samples': [], 'valid': False}
            offset += window['duration_ms']
            result['windows'][window['name']]['end_unix_ns'] = epoch + offset * 1_000_000
        result['epoch_unix_ns'] = epoch
        deadline = time.monotonic() + offset / 1000 + args.readiness_seconds
        completion_seen = False
        with (directory / 'samples.jsonl').open('w', buffering=1) as raw:
            while True:
                session.alive()
                metrics = await session.http(
                    url + f'/load-metrics?after_tick={cursor or 0}&wait=true&min_samples=20', 12)
                at = metrics.get('captured_unix_ns', time.time_ns())
                if (metrics.get('schema_version') != 1 or metrics.get('overrun') or
                        metrics.get('instance_started_unix_ns') != metric_start.get('instance_started_unix_ns') or
                        metrics.get('source_skipped_ticks_total') != metric_start.get('source_skipped_ticks_total')):
                    raise load.ClassifiedFailure('INFRA_BLOCKED', 'exact tick stream overwritten/reset/lost samples')
                for sample in metrics.get('samples', []):
                    tick, wall_start = sample.get('tick'), sample.get('wall_start_unix_ns')
                    if (not isinstance(tick, int) or (cursor is not None and tick != cursor + 1) or
                            not isinstance(sample.get('wall_us'), int) or sample['wall_us'] < 0 or
                            not isinstance(wall_start, int) or wall_start <= 0 or
                            (previous_wall_start is not None and wall_start <= previous_wall_start)):
                        raise load.ClassifiedFailure('INFRA_BLOCKED', 'invalid/noncontiguous exact tick timing')
                    cursor, previous_wall_start = tick, wall_start
                    for window in result['windows'].values():
                        if window['start_unix_ns'] <= wall_start < window['end_unix_ns']:
                            window['tick_samples'].append(sample)
                    completion_seen |= wall_start >= epoch + offset * 1_000_000
                stats = await session.stats(url)
                if not stats:
                    raise load.ClassifiedFailure('INFRA_BLOCKED', 'missing live stats')
                if stats.get('players', {}).get('count') != args.clients:
                    raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'server roster differs from requested clients')
                if session.latest_bot.get('ready_clients', args.clients) != args.clients:
                    raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'not every requested bot remains ready')
                row = {'captured_unix_ns': at, 'metrics': metrics, 'stats': stats,
                       'processes': session.process_samples(),
                       'scope_memory': scope_memory(), 'bots': session.latest_bot,
                       'server_affinity_busy_seconds': host_busy(args.server_cpus)}
                raw.write(json.dumps(row) + '\n')
                for window in result['windows'].values():
                    if window['start_unix_ns'] <= at < window['end_unix_ns']:
                        window['samples'].append(row)
                if any(process['rss_bytes'] > args.max_rss_mb * 1024 * 1024
                       for process in row['processes'].values()):
                    result['performance_errors'].append('process RSS exceeds operator limit')
                    result['memory_safety_stop'] = row['processes']
                    raise load.ClassifiedFailure('PERF_FAIL', 'process RSS exceeds operator limit')
                if completion_seen and session.latest_bot.get('open_loop', {}).get('schedule_complete'):
                    break
                if time.monotonic() > deadline:
                    raise load.ClassifiedFailure('INFRA_BLOCKED', 'finite wall schedule completion deadline')
        result['source_end_tick'] = source_tick(stats)
        result['measurement_source_ticks'] = result['source_end_tick'] - result['source_start_tick']
        if result['measurement_source_ticks'] < args.measure_ticks:
            result['performance_errors'].append('fixed wall windows did not complete required source ticks')
        result['measurement_complete'] = True
    finally:
        await session.stop()
        result['shutdown'] = session.shutdown
        load.scale.dump(directory / 'result.json', result)
    clean_shutdown(session)
    report = json.loads((directory / 'bots-report.json').read_text())
    if report.get('event') != 'final' or report.get('success') is not True:
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'missing successful final bot event')
    result['bots'] = report
    result['gameplay_work'] = gameplay_work(report)
    final_snapshot = await snapshot(args, directory, world, config)
    final = cohort(final_snapshot, recipe['expected'])
    before, after = initial['cohort']['identities'], final['identities']
    if before.keys() != after.keys():
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'same-world restart changed fixture entity identities')
    moved = {kind: sum(after[key]['kind'] == kind and after[key]['position'] != old['position']
                       for key, old in before.items()) for kind in recipe['expected']}
    if not args.setup_smoke and any(count == 0 for count in moved.values()):
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', f'no real movement in a requested family: {moved}')
    accounting = report.get('open_loop', {})
    if not accounting.get('schedule_complete'):
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'bot schedule incomplete')
    for window in accounting.get('windows', []):
        if window.get('name') == 'recovery' and any(window.get(kind, {}).get('offered', 0)
                                                  for kind in ('movement', 'edit', 'command', 'chat')):
            raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'recovery offered gameplay input')
    if not args.setup_smoke and report.get('movements_sent', 0) <= 0:
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'no actual movement writes')
    result['work_done'] = {'passed': True, 'typed_before': initial['cohort']['counts'],
        'typed_after': final['counts'], 'moved_identities': moved,
        'identity_movement_scope': 'preparation checkpoint to final checkpoint, includes readiness/warmup; not a per-family measured-window completion oracle',
        'scope': 'movement+dig/place+crafting+nonlethal combat+typed cohort+normal restore; not earned survival or MIX acceptance'}
    result['fixture_after'] = load.tree_hash(world)
    result['snapshot'] = final_snapshot
    for name, window in result['windows'].items():
        window.update(classify_window(args, window))
        result['infra_errors'].extend(f'{name}: {error}' for error in window['infra_errors'])
        result['performance_errors'].extend(f'{name}: {error}' for error in window['performance_errors'])
        if not window['samples']:
            continue
        first, last = window['samples'][0], window['samples'][-1]
        elapsed = (last['captured_unix_ns'] - first['captured_unix_ns']) / 1e9
        counters = ('movements_sent', 'entity_moves_received', 'packets_received', 'decoded_bytes_received',
                    'route_refusals_missing_column', 'route_refusals_not_walkable', 'route_refusals_height_delta',
                    'disconnects', 'timeouts')
        window['bot_counter_deltas'] = {key: last['bots'].get(key, 0) - first['bots'].get(key, 0)
                                        for key in counters}
        if not args.setup_smoke and window is not result['windows']['recovery']:
            if window['bot_counter_deltas']['movements_sent'] <= 0 or window['bot_counter_deltas']['entity_moves_received'] <= 0:
                raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'active fixed wall window lacks movement writes/received entity moves')
        if elapsed > 0:
            own = last['processes']['server']['cpu_seconds'] - first['processes']['server']['cpu_seconds']
            busy = last['server_affinity_busy_seconds'] - first['server_affinity_busy_seconds']
            window['foreign_server_cpu'] = {'host_busy_seconds': busy, 'server_seconds': own,
                'foreign_seconds_raw': busy - own, 'elapsed_seconds': elapsed,
                'qualification': 'group residual includes host work/steal and jiffy quantization; never attribute it to server'}
    result['outcome'] = load.classify(infra=result['infra_errors'], performance=result['performance_errors'])
    result['performance_evidence'] = result['outcome'] == 'PASS' and not (args.setup_smoke or args.sizing)
    load.scale.dump(directory / 'result.json', result)
    if result['outcome'] != 'PASS':
        raise load.ClassifiedFailure(result['outcome'], '; '.join(result['infra_errors'] + result['performance_errors']))
    return result


async def reopen_only(args: argparse.Namespace, directory: Path, world: Path,
                      initial: dict[str, Any], recipe: dict[str, Any]) -> dict[str, Any]:
    directory.mkdir()
    config, url, _ = configure(args, directory, world, 1, 'scale')
    server = CoreSession(args, directory, config)
    try:
        await server.start_server()
        restored = await server.await_server(url)
        population = restored['entities']['total']
        if population < sum(recipe['expected'].values()):
            raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'normal reopen lost entity population')
    finally:
        await server.stop()
    clean_shutdown(server)
    final = cohort(await snapshot(args, directory, world, config), recipe['expected'])
    if final['identities'].keys() != initial['cohort']['identities'].keys():
        raise load.ClassifiedFailure('CORRECTNESS_FAIL', 'normal reopen changed typed fixture identities')
    receipt = {'outcome': 'PASS', 'performance_evidence': False, 'durability_only': True,
               'normal_restore': True, 'shutdown': server.shutdown, 'cohort': final,
               'fixture_after': load.tree_hash(world)}
    load.scale.dump(directory / 'result.json', receipt)
    return receipt


async def run(args: argparse.Namespace) -> int:
    args.out.mkdir(parents=True)
    result: dict[str, Any] = {'outcome': 'INFRA_BLOCKED', 'setup_smoke': args.setup_smoke,
                             'performance_evidence': False, 'command': list(os.sys.argv), 'legs': []}
    result['cpu_affinity'] = {'server': args.server_cpus, 'bots': args.bot_cpus}
    try:
        verify_resume(args)
        result['memory_initial'] = scope_memory()
        for binary in (args.server_bin, args.bots_bin):
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise ValueError(f'missing/nonexecutable explicit binary: {binary}')
        result['binaries'] = {str(binary): load.scale.binary_sha256(binary)
                              for binary in (args.server_bin, args.bots_bin)}
        result['registry_hashes'] = load.tree_hash(args.vanilla_data_dir)
        result['main_go_sha256'] = load.scale.binary_sha256(args.main_go_receipt) if args.main_go_receipt else None
        recipe = fixture(args.mobs, args.villagers)
        load.scale.dump(args.out / 'fixture.json', recipe)
        result['fixture_sha256'] = load.digest(recipe)
        cache_key = binaries = None
        result['fixture_cache'] = {'status': 'disabled', 'key': None}
        if args.fixture_cache:
            cache_key = fixture_cache_key(args, recipe, result['registry_hashes']['sha256'])
            binaries = {'server_sha256': result['binaries'][str(args.server_bin)],
                        'bots_sha256': result['binaries'][str(args.bots_bin)]}
            result['fixture_cache'] = {'dir': str(args.fixture_cache), 'key': cache_key,
                                       'current': binaries, 'legs': []}
        repetitions = 1 if args.setup_smoke or args.prepare_only else 2
        for index in range(repetitions):
            directory = args.out / f'leg-{index + 1}-v2-{args.clients}'
            directory.mkdir()
            leg = {'core': 'v2', 'clients': args.clients, 'outcome': 'INFRA_BLOCKED',
                   'receipt': str(directory / 'run/result.json')}
            result['legs'].append(leg)
            world, prepared = await prepare(args, directory / 'prepare', recipe, cache_key, binaries)
            status = prepared['fixture_cache']
            leg['fixture_cache'] = status['status']
            if args.fixture_cache:
                result['fixture_cache']['legs'].append(status)
            try:
                if args.prepare_only:
                    measured = await reopen_only(args, directory / 'run', world, prepared, recipe)
                else:
                    measured = await measure(args, directory / 'run', world, recipe, prepared)
            except load.ClassifiedFailure as error:
                if status['status'] == 'hit' and status['prepared_by'] != status['current']:
                    raise cache_incompatible(error, Path(status['entry']), status['prepared_by'],
                                             status['current']) from error
                raise
            leg['outcome'] = measured['outcome']
        result['memory_final_including_saves'] = scope_memory()
        result['outcome'] = 'PASS'
        result['performance_evidence'] = not (args.setup_smoke or args.prepare_only or args.sizing)
    except load.ClassifiedFailure as error:
        result.update(outcome=error.outcome, reason=str(error))
        if result['legs'] and result['legs'][-1]['outcome'] != 'PASS':
            result['legs'][-1].update(outcome=error.outcome, reason=str(error))
    except (ValueError, OSError, KeyError, asyncio.TimeoutError) as error:
        result.update(outcome='INFRA_BLOCKED', reason=f'{type(error).__name__}: {error}')
    finally:
        load.scale.dump(args.out / 'result.json', result)
    print(json.dumps(result), flush=True)
    return int(result['outcome'] != 'PASS')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--mode', choices=('v2-only',), required=True)
    parser.add_argument('--server-bin', type=Path, required=True)
    parser.add_argument('--bots-bin', type=Path, required=True)
    parser.add_argument('--clients', type=int, required=True)
    parser.add_argument('--mobs', type=int, default=20000,
                        help='total across eight persistent mob species')
    parser.add_argument('--villagers', type=int, default=64)
    parser.add_argument('--seed', type=int, choices=(42,), default=42)
    parser.add_argument('--view-distance', type=int, choices=(6,), default=6)
    parser.add_argument('--fixture-cache', type=Path,
                        help='directory of reusable prepared worlds keyed by fixture inputs only '
                             '(mobs, villagers, seed, view distance, recipe, vanilla data); hit copies, miss populates')
    parser.add_argument('--server-cpus', default='0-7',
                        help='taskset CPU list for the server (default: four physical SMT pairs)')
    parser.add_argument('--bot-cpus', default='8-11',
                        help='taskset CPU list for bots (default: two physical SMT pairs)')
    parser.add_argument('--steady-seconds', type=float, default=60)
    parser.add_argument('--burst-seconds', type=float, default=30)
    parser.add_argument('--recovery-seconds', type=float, default=60)
    parser.add_argument('--warmup-ticks', type=int, default=200)
    parser.add_argument('--measure-ticks', type=int, default=1200)
    parser.add_argument('--readiness-seconds', type=int, default=900)
    parser.add_argument('--prepare-concurrency', type=int, default=None,
                        help='bots: client preparations in flight at once (bots default 3)')
    parser.add_argument('--min-tps', type=float, default=19.5)
    parser.add_argument('--sustained-seconds', type=int, default=5)
    parser.add_argument('--max-tick-p99-ms', type=float, default=50)
    parser.add_argument('--max-tick-max-ms', type=float, default=250)
    parser.add_argument('--max-rss-mb', type=float, default=3072)
    parser.add_argument('--vanilla-data-dir', type=Path, default=ROOT / 'data/vanilla')
    parser.add_argument('--main-go-receipt', type=Path)
    parser.add_argument('--setup-smoke', action='store_true')
    parser.add_argument('--prepare-only', action='store_true',
                        help='fresh public fixture plus normal save/reopen proof, no measurement')
    parser.add_argument('--sizing', action='store_true',
                        help='retain full fences/work but label allocation receipts noncanonical')
    args = parser.parse_args()
    if args.out.exists():
        parser.error('--out must be new; receipts are never overwritten')
    if not 1 <= args.clients <= 4096 or not 8 <= args.mobs <= 100000 or not 1 <= args.villagers <= 64:
        parser.error('clients/mobs/villagers outside declared fixture limits (at least 8 mixed mobs)')
    if any(not math.isfinite(value) or value <= 0 for value in (
            args.steady_seconds, args.burst_seconds, args.recovery_seconds, args.readiness_seconds, args.max_rss_mb,
            args.min_tps, args.max_tick_p99_ms, args.max_tick_max_ms)):
        parser.error('wall durations/readiness must be finite and positive')
    if args.sustained_seconds < 1:
        parser.error('--sustained-seconds must be positive')
    if args.warmup_ticks < 0 or args.measure_ticks < 0:
        parser.error('tick fences cannot be negative')
    for name in ('out', 'server_bin', 'bots_bin', 'vanilla_data_dir', 'fixture_cache', 'main_go_receipt'):
        value = getattr(args, name)
        if value:
            setattr(args, name, value.resolve())
    return asyncio.run(run(args))


if __name__ == '__main__':
    raise SystemExit(main())
