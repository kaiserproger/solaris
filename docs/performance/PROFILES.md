# Reproducible performance profiles

Solaris is a **draft prerelease**. These profiles measure specific workloads on
stated hardware; they are not performance guarantees, vanilla-parity proofs or
player-capacity promises. Protocol bots are real TCP clients, not graphical
Minecraft clients or human players. No survival/gameplay acceptance claim follows
from a successful measurement.

## Publication status

**Pending quiet-window measurement.** Main suspended exclusive measurement
windows during the shared-machine build wave. Per the publication decision,
ordinary-lane short smoke runs validate lifecycle only; their contended
MSPT/RSS/TPS values are discarded as publication evidence. The table is
deliberately unfilled rather than presenting historical or unrelated numbers as
these four profiles.

| Profile | Server RSS peak / steady (MiB) | MSPT p50 / p95 / p99 / max (ms) | TPS | Generated chunks/s |
|---|---|---|---|---|
| `idle` | pending | pending | pending | not instrumented |
| `explore-20` | pending | pending | pending | not instrumented |
| `native-100` | pending | pending | pending | not instrumented |
| `villager-heavy` | pending | pending | pending | not instrumented |

Only a complete `measured` receipt from an approved quiet window can replace a
pending cell. Preparation, short smoke results and the historical in-process
generator benchmark below cannot fill those cells.

## Reference hardware and build

- **CPU:** AMD Ryzen 5 7535HS with Radeon Graphics, 1 socket, 6 physical cores /
  12 SMT threads. Server affinity is logical CPUs 0–7 (four whole physical
  cores, both SMT siblings); protocol bots use 8–11 (the other two cores).
- **RAM:** 32 GB installed class; Linux `MemTotal=31,551,464 KiB`
  (30.09 GiB usable). Each invocation is bounded by a 6 GiB memory cgroup;
  that cap is not the reported process RSS.
- **World/artifact disk:** Samsung SSD 980 500GB, NVMe, ext4 on
  `/dev/nvme1n1p1`. The OS/root disk is a separate Samsung SSD 970 EVO Plus
  500GB. Worlds are not on tmpfs.
- **OS/kernel:** Ubuntu 26.04.1 LTS (x86-64), Linux `7.0.0-38-generic`.
- **Server source:** `0a86cfffc173d9fe13e9d0795a6c5494fc33a222` plus Main's
  operator chat-spam parity fix in `crates/mc-core/src/tick/input/social.rs`:
  operators still accumulate the saturating counter but are not kicked. The
  original baseline rejected fixture preparation with `disconnect.spam`.
- **Build:** `cargo build --release --bin solaris-v2 --bin player-scale-bots
  -j 2`; rustc 1.99.1 (`58cf8f9c3`, LLVM 23.1.1). Cargo release defaults:
  optimization level 3, debug information off, LTO off, 16 codegen units,
  incremental off. Repository target flags are
  `-C link-arg=-fuse-ld=mold`, linker `clang`. No additional `RUSTFLAGS`,
  frame-pointer, native-CPU, PGO or BOLT transformation.
- **Binary identity:** the retained build receipt records SHA-256 hashes of both
  the server and protocol-bot executables. The runner rejects mismatched hashes;
  use the receipt accompanying each run, not a hash from an earlier build.

## Profiles and conditions

All four use seed **42**, the ordinary generated Overworld (not flat, synthetic,
or Earth WorldPlan), native storage and tick WAL, offline local authentication,
no plugins, clear weather, easy difficulty, daylight enabled and
`randomTickSpeed=3`. Natural spawning is disabled by public preparation commands;
pre-existing natural entities are not silently deleted from the count. Every
receipt records their actual types/counts before and after the run. All entity
AI, health, movement and collision remain enabled: no `NoAI`, invulnerability,
refills or reduced AI cadence.

| Profile | Protocol clients | Named fixture entities | Server view / simulation distance | Activity |
|---|---:|---|---:|---|
| `idle` | 0 | No summoned cohort | 8 / 8 | Generated spawn area, no clients or player input; empty-server pause disabled so simulation continues |
| `explore-20` | 20 | No summoned cohort | 8 / 8 | One client per terrain anchor, 256-block grid spacing; creative, straight-line walking with sprint/jump legs; no block edits or recurring commands |
| `native-100` | 100 | 100,000 mixed mobs; no fixture villagers | 6 / 6 | Creative observers distributed across roofed pens; movement at 20 offered slots/s, dig/place at 2/s and chat at 1/s per client |
| `villager-heavy` | 100 | Same 100,000 mixed mobs **plus** 20,000 villagers | 6 / 6 | Same activity, distributed across mob and villager pens; real beds and composters |

The mixed cohort is **12,500 each** of cow, sheep, pig, chicken, zombie, skeleton,
spider and creeper. Those eight kinds and the heavy-profile villagers carry
`PersistenceRequired:1b` and a named fixture marker. Villagers are additional to
100,000 mobs, not included in that total. Saved-world checks compare typed cohort
UUIDs before and after; death/loss is a failed run, never hidden by replacement.
Natural entities outside the marker are listed separately in the receipts.

The heavy fixture is shared with the PGO experiment: public `/fill`, `/setblock`
and `/summon` commands create species-separated, roofed pens, up to 1,024 entities
per pen. Adjacent same-row mob pens share observer anchors; each villager has a
bed and composter in the bounded village pens. This is an operator-authored dense
stress scene, **not** 20,000 independent, naturally generated productive villagers.
Fixture creation uses distance 8; measured restarts use the distances above.

Spider pens have three blocks of interior headroom (floor top Y=64, roof Y=67);
the other mixed-mob pens retain their Y=71 roofs. Spiders still take ordinary
damage after detaching from a wall, so taller pens permit damaging free falls.
Climb contact also resets retained fall distance in the native movement path:
ceiling bounces must not accumulate separate short drops into false damage.
The lower physical ceiling bounds a single unassisted drop without changing AI,
health or damage rules. The recipe hash changes the fixture-cache key, so old
tall-pen fixtures are not reused.

Explorers use the existing bot's received-terrain collision checks. They may
turn when terrain is missing/unwalkable or the height change is too large;
`--explore` disables the ordinary 200-tick turn, not collision. Read
`chunk_boundaries_crossed`, route-refusal and movement counters in the receipt.
It is not flying, teleport spam, or a promise that every client advances
indefinitely through terrain. Login includes operator-assisted safe landing.

### Windows and metric definitions

- A fresh server process opens the prepared world. Fixture generation and all
  client login/setup are **outside** the measured interval.
- After all requested clients are ready: **30 seconds of active warmup**, then
  **120 seconds measured**, using the same offered workload in both intervals.
  Idle uses the same wall durations without bots. Changing either duration
  creates a different profile and is recorded in the receipt.
- **MSPT p50/p95/p99/max** use nearest-rank percentiles of every exact source
  `wall_us` sample whose `wall_start_unix_ns` lies in the half-open measured
  interval. This is active core-iteration wall time, excluding pacing sleep;
  dashboard rolling percentiles are not substituted.
- **TPS** is completed source iterations starting in that interval divided by
  its **full 120 seconds**, not the span from first to last observed sample.
  A missing/reset/overwritten tick stream invalidates the result.
- **RSS steady** is the median sampled server process RSS during measurement.
  **RSS peak** is Linux `VmHWM`, the server-process lifetime peak observed during
  measurement (therefore including restart/login/warmup, not the separate fixture
  builder or later offline snapshot reader). Bot RSS is recorded separately,
  never added to a number labelled server RSS. Samples follow exact-tick stream
  notifications, approximately once per 20 ticks; they are not a continuous RSS
  trace or a cgroup/page-cache measurement.
- Each run checks the live player count, successful complete bot schedule,
  movement, clean server/bot exits and saved fixture identities. A low TPS value
  is reported, not converted into a capacity pass or discarded.
- **Per-profile generated chunks/s: not instrumented.** The production dashboard
  reports resident chunks, which can include reloads and exclude evictions.
  Resident-chunk deltas and received packet counts are not generation throughput.
  See [in-process chunk generation](#in-process-chunk-generation) for the separate
  generator benchmark; it must not be combined with these load profiles.

## Reproduce

Run from a checkout with the pinned Rust toolchain, Clang/mold, Python 3, Linux
`taskset`, `lscpu`, `lsblk` and `findmnt`. Import the licensed 26.1.2 content cache
as described in the [operating guide](../OPERATING.md#vanilla-content-cache).
Neither Mojang bytes nor prepared worlds are shipped in Git.

### Build once and retain provenance

Use an ordinary, **non-PGO-instrumented release** build. Do not silently point a
release receipt at another binary: the runner checks both binary SHA-256 hashes.
The commit identifies the server source, while the receipt also retains the
actual build command/toolchain. If your source is dirty, retain its patch and
identify it in the receipt instead of presenting HEAD as the whole source.

```sh
ROOT="$(pwd -P)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
cargo build --release --bin solaris-v2 --bin player-scale-bots -j 2
python3 - <<'PY'
import hashlib, json, os, pathlib, subprocess
root = pathlib.Path.cwd()
target = pathlib.Path(os.environ['CARGO_TARGET_DIR']).resolve()
def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()
receipt = {
    'profile': 'release',
    'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'command': ['cargo', 'build', '--release', '--bin', 'solaris-v2', '--bin', 'player-scale-bots', '-j', '2'],
    'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
    'build_environment': {k: v for k, v in os.environ.items()
        if k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_DIR') or k.startswith('CARGO_PROFILE_RELEASE')},
    'cargo_config': (root / '.cargo/config.toml').read_text(),
    'server_sha256': sha(target / 'release/solaris-v2'),
    'bots_sha256': sha(target / 'release/player-scale-bots'),
}
(root / '.analysis').mkdir(exist_ok=True)
(root / '.analysis/profile-build.json').write_text(json.dumps(receipt, indent=2) + '\n')
PY
```

On the shared agent host, wrap builds in
`/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -m 4G` and use the assigned
target directory (raise the cap only after an observed OOM). Do not compile during somebody else's measurement window.

### One command per profile

The following Bash array only supplies the common binary/provenance paths;
each invocation owns preparation/cache reuse, startup, warmup, measurement,
shutdown, native snapshot checks and its receipt directory.

```sh
COMMON=(--server-bin "$CARGO_TARGET_DIR/release/solaris-v2"
        --bots-bin "$CARGO_TARGET_DIR/release/player-scale-bots"
        --build-receipt "$ROOT/.analysis/profile-build.json")
python3 -m tools.coreperf.profiles idle "${COMMON[@]}"
python3 -m tools.coreperf.profiles explore-20 "${COMMON[@]}"
python3 -m tools.coreperf.profiles native-100 "${COMMON[@]}"
python3 -m tools.coreperf.profiles villager-heavy "${COMMON[@]}"
```

Fixture preparation uses the harness's memory guard, so execute these commands
inside a bounded cgroup. On the shared host, request Main's **GO**, then prefix
**each** command with
`/home/kaiserroman/solaris/.analysis/tools/mem-run.sh -x -p -m 6G`.
On another Linux host use the same runner or an equivalent bounded cgroup,
record the cap and run without competing builds/tests. The published affinity
is server logical CPUs `0-7`, bots `8-11`; override `--server-cpus` and
`--bot-cpus` for a different topology and report the new mapping. Logical CPU
IDs are not a count of physical cores.

Default output is a unique `.analysis/validation/<UTC>-profile-<name>-<pid>/`.
`--out` can choose a **new** absolute directory; existing receipts are never
overwritten. `--fixture-cache` selects the shared pristine cache. A cache hit
copies the world, verifies its content hash and re-reads the named entity cohort;
binary hashes identify both its producer and current consumer. Use
`--prepare-only` outside the quiet window to populate it first. A `prepared`
receipt is **not** a measurement. The PGO cache is reusable only when the recipe,
content and fixture parameters match exactly; instrumented PGO timings are never
published as these release measurements.
Public input-echo fences during preparation share the bot's total
`--prepare-seconds` readiness deadline rather than a separate per-batch timeout.
Regional fixture clients use open-loop readiness to complete their setup before
Ready; no Run event is issued and their required schedule is never started.
The bot continues processing packets and replying to heartbeats while waiting
for a slow `save-all flush` to complete; measured open-loop demand is unchanged.

For a lifecycle-only check outside an exclusive window, add `--smoke`: the same
full population runs for 2 seconds warmup and 5 seconds observation. Its status
is `smoke`, with `publication_evidence=false`; even a successful smoke is not
an approved performance measurement. Use the ordinary memory-capped lane, not
`-x -p`, while Main's quiet windows are suspended.

### Evidence retained by each invocation

- `result.json`: status (`measured`, `prepared`, `smoke` or `failed`), exact invocation,
  workload conditions, host CPU/RAM/OS/kernel/disks/filesystem, release build
  receipt, registry hashes, commands/exits/log paths and the measurement summary.
- `fixture.json`, `prepare/receipt.json`: exact public commands, typed cohort,
  preparation provenance and cache status.
- `run/server.toml`, `run/server-command.json`, `run/schedule.json`, bot command
  and `run/bots-report.json`: actual configuration and offered/observed activity.
- `run/samples.jsonl`: raw exact ticks, process RSS, dashboard populations and
  bot counters; `run/result.json`: measured window boundaries and statistics.
- Server/bot logs plus read-only native snapshot receipts. Failed or incomplete
  runs retain evidence and return nonzero; do not publish their partial summary
  as a successful profile.

## In-process chunk generation

An existing **Gen32** quiet-window measurement on the same Ryzen 5 7535HS reports
the following Solaris vanilla-style Overworld generator costs. These are
**single-threaded in-process generation**, not full-server requested chunks/s,
not exploration throughput and not Mojang's Java server.

| Seed | Median ms/generated chunk | Observed min–max (ms/chunk) | Reciprocal rate (chunks/s) |
|---|---:|---:|---:|
| 712816 | 17.335 | 17.290–17.355 | 57.69 |
| 5617830 | 14.145 | 14.114–14.231 | 70.70 |
| 42 | 13.786 | 13.734–13.796 | 72.54 |

Each seed has three runs of 36 fresh chunks, `x,z=-3..2`; each repetition creates
a new generator, with data/tag/village-plan setup outside the timer. Ores,
vegetation and intersecting structure plans remain enabled. Lighting is measured
separately and **excluded** from the generation numbers above, as are network,
player simulation, persistence and startup. No explicit warmup run was excluded.
The rate column is `1000 / median(ms/chunk)`, not an independently measured
concurrent throughput. Three observations are not stable p99 evidence.

Provenance: Gen32's `default-measurements.json`, `simd-default` rows, measured
with `mem-run.sh -x -p -m 6G measure-default.sh`, default x86-64 release target
with Rust/Cargo rustflag environment overrides removed. Frozen executable
SHA-256: `d327b04e683d59eced6a0d61859d62b856639507a18285ba7afbc856e15ca53b`.
This historical source identity is base
`b8210b456857a0a43f03a0bc1530204d5d728eb9` plus the fp32/ore patch
`83b5d1a6a98d98e7ab3ddbdc5d42fd37c1bf499448231ad37dcd34a565250b6b`
and SIMD patch
`f969e468a707bbed311bad4dd38d6222eb5d5b940005c75a620f12c3969b9db1`;
it is not a new measurement of the profile server binary. The owner's retained
raw artifacts are under `.analysis/codex-logs/Gen32/`.

Raw ms/chunk observations, in run order:

```text
712816:  17.33538888888889, 17.35509172222222, 17.29047088888889
5617830: 14.11377777777778, 14.14462250000000, 14.23136988888889
42:      13.78616155555556, 13.79593002777778, 13.73403130555556
```

The [retained stage benchmark](../../crates/mc-worldgen/src/terrain/profile_tests.rs)
reproduces that workload in the current checkout. Build it **before** the quiet
window, then run the compiler-reported release test executable:

```sh
cargo test -p mc-worldgen --lib --release --no-run
# TEST_BINARY is the mc_worldgen executable printed by Cargo above.
"$TEST_BINARY" --ignored --exact terrain::profile_tests::vanilla_stages --nocapture --test-threads=1
```

`vanilla_stages_shared_solids` measures the same chunks as chunk workers
generate them with the shared solids cache (one density fill per chunk, timed
apart), and `vanilla_stages_gpu_density` (`--features gpu`) with GPU density;
every mode also prints `bake=`, the light a worker bakes with no neighbours.

Use the same memory/quiet-window rules as above. The separate GenBench
Java-versus-Solaris server experiment has no completed Solaris result in this
publication; no server-throughput ratio is claimed.
