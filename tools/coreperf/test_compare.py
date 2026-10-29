"""Offline contract checks; no server, bots, build or load is started."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from . import compare


def args(**changes):
    values = dict(setup_smoke=False, prepare_only=False, clients=100, mobs=20000, villagers=64, fixture_cache=None,
                  steady_seconds=60, burst_seconds=30, recovery_seconds=60, warmup_ticks=200,
                  measure_ticks=1200, main_go_receipt=None,
                  seed=42, view_distance=6, server_cpus='0-7', bot_cpus='8-11',
                  vanilla_data_dir=Path('/data/vanilla'), min_tps=19.5, sustained_seconds=5,
                  max_tick_p99_ms=50, max_tick_max_ms=250, max_rss_mb=3072)
    values.update(changes)
    return argparse.Namespace(**values)


class CompareContractTests(unittest.TestCase):
    def test_representative_fixture_has_all_eight_kinds_and_villagers(self):
        recipe = compare.fixture(20000, 64)
        commands = [command for region in recipe['regions'] for command in region['commands']]
        for kind in compare.MOB_KINDS:
            summons = [command for command in commands if command.startswith(f'summon minecraft:{kind} ')]
            self.assertEqual(len(summons), 2500)
            self.assertTrue(all("PersistenceRequired:1b" in command for command in summons))
        self.assertEqual(sum(command.startswith('summon minecraft:villager ') for command in commands), 64)
        self.assertTrue(all(f"CustomName:'{compare.COHORT_NAME}'" in command
                            for command in commands if command.startswith("summon ")))
        self.assertEqual(sum('minecraft:composter' in command for command in commands), 64)
        self.assertEqual(sum('part=foot' in command for command in commands), 64)
        self.assertEqual(sum('part=head' in command for command in commands), 64)
        self.assertFalse(any('NoAI' in command or 'invulnerable' in command.lower() for command in commands))
        self.assertEqual(recipe['expected'], {f'minecraft:{kind}': 2500 for kind in compare.MOB_KINDS}
                         | {'minecraft:villager': 64})
        self.assertEqual(recipe, compare.fixture(20000, 64))

    def test_cli_refuses_dropped_baseline_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["compare", "--out", str(Path(directory) / "new"), "--mode", "abab",
                    "--server-bin", "/unused", "--bots-bin", "/unused", "--clients", "1"]
            with mock.patch.object(compare.os.sys, "argv", argv), self.assertRaises(SystemExit):
                compare.main()

    def test_cli_accepts_native_population_above_former_baseline_capacity(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["compare", "--out", str(Path(directory) / "new"), "--mode", "v2-only",
                    "--server-bin", "/unused", "--bots-bin", "/unused",
                    "--clients", "1", "--mobs", "100000", "--prepare-only"]
            with mock.patch.object(compare.os.sys, "argv", argv), mock.patch.object(
                    compare, 'run', new_callable=mock.AsyncMock, return_value=0) as run:
                self.assertEqual(compare.main(), 0)
            self.assertEqual(run.call_args.args[0].mobs, 100000)
            received = run.call_args.args[0]
            self.assertEqual((received.min_tps, received.sustained_seconds,
                              received.max_tick_p99_ms, received.max_tick_max_ms,
                              received.max_rss_mb), (19.5, 5, 50, 250, 3072))
            self.assertEqual((received.server_cpus, received.bot_cpus), ('0-7', '8-11'))

    def test_cpu_affinity_and_rss_budget_are_overridable(self):
        with tempfile.TemporaryDirectory() as directory:
            argv = ["compare", "--out", str(Path(directory) / "new"), "--mode", "v2-only",
                    "--server-bin", "/unused", "--bots-bin", "/unused", "--clients", "1",
                    "--server-cpus", "0,2,4,6", "--bot-cpus", "8,10", "--max-rss-mb", "8192"]
            with mock.patch.object(compare.os.sys, "argv", argv), mock.patch.object(
                    compare, 'run', new_callable=mock.AsyncMock, return_value=0) as run:
                self.assertEqual(compare.main(), 0)
            received = run.call_args.args[0]
            self.assertEqual((received.server_cpus, received.bot_cpus, received.max_rss_mb),
                             ('0,2,4,6', '8,10', 8192))

    def test_partial_population_has_exact_count_and_species_separated_roofed_pens(self):
        recipe = compare.fixture(8201, 1)
        self.assertEqual(sum(recipe['expected'].values()), 8202)
        for region in recipe['regions'][:-1]:
            kinds = {command.split()[1] for command in region['commands'] if command.startswith('summon ')}
            self.assertEqual(len(kinds), 1)
            roof_y = 67 if kinds == {'minecraft:spider'} else 71
            self.assertTrue(any(command.startswith('fill ') and f' {roof_y} ' in command
                                and command.endswith('minecraft:stone') for command in region['commands']))

    def test_spider_pen_roofs_bound_unassisted_falls_without_disabling_ai(self):
        spiders = 0
        for region in compare.fixture(100000, 0)['regions']:
            summons = [command.split() for command in region['commands']
                       if command.startswith('summon minecraft:spider ')]
            if not summons:
                continue
            spiders += len(summons)
            roofs = [command.split() for command in region['commands']
                     if command.startswith('fill ') and command.endswith('minecraft:stone')
                     and command.split()[2] == command.split()[5]]
            self.assertEqual(len(roofs), 1, region['name'])
            roof_y = int(roofs[0][2])
            x1, _, z1, x2, _, z2 = map(int, roofs[0][1:7])
            walls = {tuple(map(int, command.split()[1:7])) for command in region['commands']
                     if command.startswith('fill ') and command.endswith('minecraft:stone')
                     and command.split()[2] != command.split()[5]}
            self.assertEqual(walls, {
                (x1, 64, z1, x2, roof_y - 1, z1),
                (x1, 64, z2, x2, roof_y - 1, z2),
                (x1, 64, z1 + 1, x1, roof_y - 1, z2 - 1),
                (x2, 64, z1 + 1, x2, roof_y - 1, z2 - 1),
            })
            self.assertIn(
                f'fill {x1 + 1} 63 {z1 + 1} {x2 - 1} 63 {z2 - 1} minecraft:grass_block',
                region['commands'])
            for summon in summons:
                # Even a climbing spider's feet must stay below three blocks
                # above the floor. A detached free fall must remain nonlethal.
                self.assertLessEqual(roof_y - float(summon[3]), 3, region['name'])
                self.assertGreater(roof_y - float(summon[3]), 0.9, region['name'])
                self.assertTrue(x1 + 1 <= float(summon[2]) - 0.7 < float(summon[2]) + 0.7 <= x2)
                self.assertTrue(z1 + 1 <= float(summon[4]) - 0.7 < float(summon[4]) + 0.7 <= z2)
            self.assertFalse(any('NoAI' in command or 'Invulnerable' in command
                                 for command in region['commands']))
        self.assertEqual(spiders, 12500)

    def test_gameplay_uses_wire_crafting_nonlethal_combat_and_edits(self):
        spec = compare.gameplay(compare.fixture(8, 2))
        kinds = [action['kind'] for action in spec['actions']]
        self.assertEqual(kinds.count('attack_entity'), 1)
        self.assertEqual(kinds.count('click_slot'), 3)
        self.assertIn('open_container', kinds)
        self.assertTrue(all(action['window'] != 'recovery' for action in spec['actions']))
        self.assertEqual([row['edit_hz'] for row in compare.schedule(args())['windows']], [2, 2, 0])
        self.assertEqual(next(action['slot'] for action in spec['actions']
                              if action['kind'] == 'select_slot'), 7)
        self.assertEqual(next(action['slot_item'] for action in spec['actions']
                              if action['kind'] == 'click_slot'), 'minecraft:oak_log')

    def test_gameplay_oracle_requires_received_work_not_writes(self):
        report = {"break_updates_observed": 1, "place_updates_observed": 1,
                  "mechanics": {"players": [{"final_inventory": [{"name": "minecraft:oak_planks", "count": 4}]}],
                                "actions": [{"kind": kind, "completed": 1} for kind in
                                            ("open_container", "click_slot", "click_slot", "click_slot", "attack_entity")]}}
        self.assertEqual(compare.gameplay_work(report)["crafted_planks_received"], 4)
        for key in ("break_updates_observed", "place_updates_observed"):
            with self.subTest(key=key), self.assertRaises(compare.load.ClassifiedFailure):
                compare.gameplay_work(report | {key: 0})
        report["mechanics"]["actions"][-1]["completed"] = 0
        with self.assertRaises(compare.load.ClassifiedFailure):
            compare.gameplay_work(report)

    def test_public_fill_commands_fit_vanilla_budget(self):
        for region in compare.fixture(20000, 64)['regions']:
            for command in region['commands']:
                fields = command.split()
                if fields[0] == 'fill':
                    coords = list(map(int, fields[1:7]))
                    cells = 1
                    for axis in range(3):
                        cells *= abs(coords[axis] - coords[axis + 3]) + 1
                    self.assertLessEqual(cells, 32768)

    def test_canonical_never_resumes_without_main(self):
        with self.assertRaisesRegex(ValueError, 'Main'):
            compare.verify_resume(args())

    def test_go_receipt_is_required_and_nonempty(self):
        with tempfile.TemporaryDirectory() as directory:
            go = Path(directory) / 'go.txt'
            go.write_text('Main GO window 15; actual authorization fixture for unit test')
            compare.verify_resume(args(main_go_receipt=go))
            go.write_text('')
            with self.assertRaisesRegex(ValueError, 'empty Main'):
                compare.verify_resume(args(main_go_receipt=go))

    def test_setup_smoke_is_explicitly_tiny(self):
        tiny = dict(setup_smoke=True, clients=2, mobs=8, villagers=2, steady_seconds=2,
                    burst_seconds=2, recovery_seconds=2, warmup_ticks=0, measure_ticks=0)
        compare.verify_resume(args(**tiny))
        for key, value in (('clients', 3), ('mobs', 9), ('villagers', 3), ('burst_seconds', 3),
                           ('warmup_ticks', 1), ('measure_ticks', 1)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                compare.verify_resume(args(**(tiny | {key: value})))

    def test_prepare_only_checks_large_fixture_without_measurement_authorization(self):
        compare.verify_resume(args(prepare_only=True, clients=2, mobs=100000,
                                   warmup_ticks=0, measure_ticks=0))

    def test_canonical_tick_fences_cannot_be_shortened(self):
        for changes in ({'warmup_ticks': 199}, {'measure_ticks': 1199}):
            with self.assertRaisesRegex(ValueError, 'canonical'):
                compare.verify_resume(args(**changes))

    def test_recovery_has_zero_offers_and_burst_is_independent(self):
        windows = compare.schedule(args())['windows']
        self.assertEqual([window['duration_ms'] for window in windows], [60000, 30000, 60000])
        self.assertEqual([window['movement_hz'] for window in windows], [20, 40, 0])
        self.assertTrue(windows[2]['recovery'])
        for kind in ('movement_hz', 'edit_hz', 'command_hz', 'chat_hz'):
            self.assertEqual(windows[2][kind], 0)

    def exact_window(self, count=200, seconds=10, wall_us=1000, rss_mb=100):
        start = 1_000_000_000
        return {'start_unix_ns': start, 'end_unix_ns': start + seconds * 1_000_000_000,
                'tick_samples': [{'tick': tick + 1, 'wall_us': wall_us,
                                  'wall_start_unix_ns': start + tick * seconds * 1_000_000_000 // count}
                                 for tick in range(count)],
                'samples': [{'processes': {name: {'peak_rss_bytes': rss_mb * 1024 * 1024}
                                          for name in ('server', 'bots')}}]}

    def test_exact_windows_apply_catalog_thresholds_independently(self):
        windows = {'steady': self.exact_window(), 'burst': self.exact_window(wall_us=60_000),
                   'recovery': self.exact_window()}
        outcomes = {name: compare.classify_window(args(), window)['outcome']
                    for name, window in windows.items()}
        self.assertEqual(outcomes, {'steady': 'PASS', 'burst': 'PERF_FAIL', 'recovery': 'PASS'})
        self.assertEqual(windows['steady']['tick']['p99_us'], 1000)
        self.assertEqual(windows['burst']['tick']['p99_us'], 60_000)
        self.assertEqual(windows['recovery']['tps'], 20)

    def test_1200_ticks_in_150_seconds_cannot_pass(self):
        window = self.exact_window(count=1200, seconds=150)
        result = compare.classify_window(args(), window)
        self.assertEqual(result['outcome'], 'PERF_FAIL')
        self.assertEqual(window['tps'], 8)
        self.assertEqual(window['sustained_tps_min'], 8)

    def test_sustained_tps_rejects_low_interval_despite_good_average(self):
        window = self.exact_window()
        for tick in window['tick_samples']:
            tick['wall_start_unix_ns'] = window['start_unix_ns'] + 5_000_000_000 + (tick['tick'] - 1) * 25_000_000
        self.assertEqual(compare.classify_window(args(), window)['outcome'], 'PERF_FAIL')
        self.assertEqual(window['tps'], 20)
        self.assertEqual(window['sustained_tps_min'], 0)

    def test_tick_max_and_explicit_rss_caps_fail_closed(self):
        window = self.exact_window()
        window['tick_samples'][0]['wall_us'] = 250_001
        self.assertEqual(compare.classify_window(args(), window)['outcome'], 'PERF_FAIL')
        self.assertEqual(window['tick']['p99_us'], 1000)
        self.assertEqual(compare.classify_window(args(), self.exact_window(rss_mb=4096))['outcome'], 'PERF_FAIL')
        window = self.exact_window(rss_mb=4096)
        self.assertEqual(compare.classify_window(args(max_rss_mb=8192), window)['outcome'], 'PASS')
        self.assertEqual(window['process_peaks']['server'], 4096 * 1024 * 1024)

    def test_missing_or_noncontiguous_exact_ticks_are_infrastructure_failures(self):
        window = self.exact_window()
        window['tick_samples'][100]['tick'] += 1
        self.assertEqual(compare.classify_window(args(), window)['outcome'], 'INFRA_BLOCKED')
        self.assertEqual(compare.classify_window(args(), self.exact_window(count=0))['outcome'], 'INFRA_BLOCKED')

    def test_native_snapshots_use_actual_typed_identities(self):
        native = {'entities': {'minecraft:villager': {'samples': [
            {'uuid': 'actual-id', 'active': True, 'final_position': [1.0, 64.0, 2.0],
             'payload': {'CustomName': compare.COHORT_NAME}}]}}}
        expected = {'minecraft:villager': 1}
        self.assertEqual(set(compare.entities(native)), {'actual-id'})
        self.assertEqual(compare.cohort(native, expected)['counts'], expected)
        with self.assertRaises(compare.load.ClassifiedFailure):
            compare.cohort(native, {'minecraft:cow': 1, 'minecraft:villager': 1})

    def test_extra_natural_villagers_do_not_mask_missing_or_extra_named_cohort(self):
        expected = {'minecraft:villager': 1}
        fixture = {'uuid': 'fixture-id', 'active': True, 'final_position': [1, 64, 2],
                   'payload': {'CustomName': compare.COHORT_NAME}}
        natural = fixture | {'uuid': 'natural-id', 'payload': {}}
        def image(rows):
            return {'entities': {'minecraft:villager': {'samples': rows}}}
        result = compare.cohort(image([fixture, natural]), expected)
        self.assertEqual(result['counts'], expected)
        self.assertEqual(set(result['identities']), {'fixture-id'})
        for rows in ([natural], [fixture, natural, fixture | {'uuid': 'extra-named-id'}]):
            with self.assertRaises(compare.load.ClassifiedFailure):
                compare.cohort(image(rows), expected)

    def test_native_config_never_uses_legacy_world_contract(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(compare.load.scale, 'reserve_ports', return_value=(25566, 25567)):
            path, _, _ = compare.configure(args(), Path(directory), Path(directory) / 'world', 2, 'scale')
            self.assertNotIn('[admin]', path.read_text())
            self.assertFalse((Path(directory) / 'world').exists())

    def test_prepare_copies_args_for_wider_public_regions(self):
        original = args()
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(
                compare, 'configure', side_effect=RuntimeError('fixture config reached')) as configure:
            with self.assertRaisesRegex(RuntimeError, 'fixture config reached'):
                asyncio.run(compare.prepare(original, Path(directory) / 'prepare', compare.fixture(2, 2)))
        prepared = configure.call_args.args[0]
        self.assertIsNot(prepared, original)
        self.assertEqual(prepared.view_distance, 8)
        self.assertEqual(original.view_distance, 6)

    def test_preparation_records_server_and_bot_rss_separately(self):
        server_sample = {'rss_bytes': 40, 'peak_rss_bytes': 80}
        bots_sample = {'rss_bytes': 3, 'peak_rss_bytes': 4}
        stats = {'simulation_tick': 7, 'entities': {'total': 100}, 'chunks': {'loaded_total': 289}}
        session = argparse.Namespace(start_server=mock.AsyncMock(), await_server=mock.AsyncMock(),
            alive=mock.Mock(), process_samples=mock.Mock(return_value={'server': server_sample}),
            stats=mock.AsyncMock(return_value=stats), processes={'server': argparse.Namespace(pid=42)},
            stop=mock.AsyncMock(), shutdown={'server': {'returncode': 0, 'forced': False}})
        recipe = {'regions': [{'name': 'one'}, {'name': 'two'}], 'expected': {'minecraft:villager': 1}}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / 'server.toml'
            config.write_text('')
            with (mock.patch.object(compare, 'configure', return_value=(config, 'http://unused', 1)),
                  mock.patch.object(compare, 'CoreSession', return_value=session),
                  mock.patch.object(compare.load, 'public_region', new_callable=mock.AsyncMock,
                                    return_value={'process_samples': {'bots': bots_sample}}) as public_region,
                  mock.patch.object(compare, 'scope_memory'),
                  mock.patch.object(Path, 'read_text', return_value='mapping\nRss: 17 kB\nAnonymous: 12 kB\n'),
                  mock.patch.object(compare.load, 'tree_hash', return_value={}),
                  mock.patch.object(compare, 'snapshot', new_callable=mock.AsyncMock, return_value={}),
                  mock.patch.object(compare, 'cohort', return_value={'counts': recipe['expected']})):
                _, receipt = asyncio.run(compare.prepare(args(), root / 'prepare', recipe))
                self.assertNotIn('setup_ingress_pacing', public_region.call_args.kwargs)
                self.assertEqual(session.start_server.await_count, 1)
                self.assertEqual(session.stop.await_count, 1)
                self.assertNotIn('preparation_segmentation', receipt)
        self.assertEqual(receipt['regions'][0]['process_samples'],
                         {'server': server_sample, 'bots': bots_sample})
        self.assertEqual(receipt['regions'][0]['server_stats'], stats)
        self.assertEqual(receipt['regions'][0]['server_smaps_rollup'],
                         {'Rss': 17 * 1024, 'Anonymous': 12 * 1024})

    def test_server_ready_excludes_unpublished_default_population(self):
        session = compare.CoreSession(args(readiness_seconds=1), Path('/unused'), Path('/unused.toml'))
        session.stats = mock.AsyncMock(side_effect=[{'simulation_tick': 0}, {'simulation_tick': 1}])
        with mock.patch.object(compare.asyncio, 'sleep', new_callable=mock.AsyncMock):
            self.assertEqual(asyncio.run(session.await_server('http://unused'))['simulation_tick'], 1)
        self.assertEqual(session.stats.await_count, 2)

    def test_source_tick_is_not_async_publication_tick(self):
        self.assertEqual(compare.source_tick({'simulation_tick': 250, 'measurement_tick': 200}), 200)
        self.assertEqual(compare.source_tick({'simulation_tick': 250, 'tick': {'source_tick': 220}}), 220)
        self.assertEqual(compare.source_tick({'simulation_tick': 5, 'measurement_tick': 0, 'tick': None}), 0)

    def test_forced_or_failed_final_save_is_never_clean(self):
        for shutdown in ({'server': {'forced': True, 'returncode': 0}},
                         {'server': {'forced': False, 'returncode': 101}}):
            with self.assertRaises(compare.load.ClassifiedFailure):
                compare.clean_shutdown(argparse.Namespace(shutdown=shutdown))


class FixtureCacheTests(unittest.TestCase):
    BINARIES = {'server_sha256': 'srv-1', 'bots_sha256': 'bots-1'}

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.cache = self.root / 'cache'
        self.recipe = {'regions': [{'name': 'one'}], 'expected': {'minecraft:villager': 1}}
        self.key = compare.fixture_cache_key(args(), self.recipe, 'vanilla')
        self.config = self.root / 'server.toml'
        self.config.write_text('')
        self.generated = 0
        self.snapshot = mock.AsyncMock(return_value={})
        self.session = argparse.Namespace(
            start_server=mock.AsyncMock(), await_server=mock.AsyncMock(), alive=mock.Mock(),
            process_samples=mock.Mock(return_value={}), stats=mock.AsyncMock(return_value={}),
            processes={'server': argparse.Namespace(pid=os.getpid())}, stop=mock.AsyncMock(),
            shutdown={'server': {'returncode': 0, 'forced': False}})

    async def region(self, args, directory, game, region, **kwargs):
        self.generated += 1
        world = directory.parent.parent / 'world'
        world.mkdir(exist_ok=True)
        (world / 'entities.dat').write_text(f'generation {self.generated}')
        return {'process_samples': {}}

    def prepare(self, name, **changes):
        leg = self.root / name
        leg.mkdir()
        with (mock.patch.object(compare, 'configure', return_value=(self.config, 'http://unused', 1)),
              mock.patch.object(compare, 'CoreSession', return_value=self.session),
              mock.patch.object(compare.load, 'public_region', side_effect=self.region),
              mock.patch.object(compare, 'scope_memory'),
              mock.patch.object(compare, 'snapshot', self.snapshot),
              mock.patch.object(compare, 'cohort', return_value={'counts': self.recipe['expected']})):
            return asyncio.run(compare.prepare(
                args(fixture_cache=self.cache, **changes), leg / 'prepare', self.recipe, self.key, self.BINARIES))

    def test_key_follows_fixture_inputs_only(self):
        key = lambda recipe=self.recipe, vanilla='vanilla', **changes: compare.fixture_cache_key(
            args(**changes), recipe, vanilla)
        self.assertEqual(key(), self.key)
        for other in (key(mobs=8), key(villagers=2), key(vanilla='other'), key(view_distance=8), key(seed=7),
                      key(recipe=compare.fixture(8, 2))):
            self.assertNotEqual(other, self.key)

    def test_miss_populates_atomically_then_hit_copies_without_generating(self):
        world, receipt = self.prepare('a')
        self.assertEqual(receipt['fixture_cache']['status'], 'miss')
        entry = self.cache / self.key
        self.assertEqual(json.loads((entry / compare.CACHE_MARKER).read_text()),
                         {'key': self.key, 'prepared_by': self.BINARIES})
        self.assertEqual((entry / 'world/entities.dat').read_text(), 'generation 1')
        self.assertEqual([path.name for path in self.cache.iterdir()], [self.key])
        self.assertEqual(self.session.start_server.await_count, 1)

        rebuilt = {'server_sha256': 'srv-2', 'bots_sha256': 'bots-1'}
        self.BINARIES = rebuilt
        world, receipt = self.prepare('b')
        self.assertEqual(self.generated, 1)
        self.assertEqual(self.session.start_server.await_count, 1)
        self.assertEqual((world / 'entities.dat').read_text(), 'generation 1')
        self.assertEqual(receipt['fixture_cache']['status'], 'hit')
        self.assertEqual(receipt['fixture_cache']['prepared_by'], {'server_sha256': 'srv-1', 'bots_sha256': 'bots-1'})
        self.assertEqual(receipt['fixture_cache']['current'], rebuilt)
        self.assertEqual(self.snapshot.await_count, 2)  # readback runs on the copy too
        self.assertEqual(receipt['fixture']['sha256'], compare.load.tree_hash(world)['sha256'])

    def test_incomplete_entry_is_a_miss_and_is_replaced(self):
        (self.cache / self.key).mkdir(parents=True)
        (self.cache / self.key / 'world').mkdir()
        _, receipt = self.prepare('a')
        self.assertEqual(receipt['fixture_cache']['status'], 'miss')
        self.assertTrue((self.cache / self.key / compare.CACHE_MARKER).is_file())

    def test_tampered_cache_entry_fails_closed(self):
        self.prepare('a')
        (self.cache / self.key / 'world/entities.dat').write_text('tampered')
        with self.assertRaisesRegex(compare.load.ClassifiedFailure, 'differs from its recorded hash'):
            self.prepare('b')

    def test_incompatible_cached_world_names_both_binaries(self):
        self.prepare('a')
        self.snapshot.side_effect = compare.load.ClassifiedFailure('INFRA_BLOCKED', 'native probe rejected world')
        self.BINARIES = {'server_sha256': 'srv-2', 'bots_sha256': 'bots-1'}
        with self.assertRaisesRegex(compare.load.ClassifiedFailure, r'srv-1.*srv-2.*native probe rejected world'):
            self.prepare('b')

    def test_disabled_cache_records_disabled_and_writes_nothing(self):
        self.key = None
        with (mock.patch.object(compare, 'configure', return_value=(self.config, 'http://unused', 1)),
              mock.patch.object(compare, 'CoreSession', return_value=self.session),
              mock.patch.object(compare.load, 'public_region', side_effect=self.region),
              mock.patch.object(compare, 'scope_memory'),
              mock.patch.object(compare, 'snapshot', self.snapshot),
              mock.patch.object(compare, 'cohort', return_value={'counts': self.recipe['expected']})):
            (self.root / 'a').mkdir()
            _, receipt = asyncio.run(compare.prepare(args(), self.root / 'a/prepare', self.recipe))
        self.assertEqual(receipt['fixture_cache'], {'status': 'disabled', 'key': None, 'prepared_by': None})
        self.assertFalse(self.cache.exists())



if __name__ == '__main__':
    unittest.main()
