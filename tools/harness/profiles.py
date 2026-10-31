#!/usr/bin/env python3
"""Named validation profiles for the centralized harness.

Every profile is a real implementation: either direct calls into sibling
scenario modules or a maintained internal backend command. Unknown
profiles, nonzero commands, exceptions, and prepared-only runs never pass.
"""

from __future__ import annotations

import json
import os
import runpy
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from .runtime import AGENT_ROOT, REPO_ROOT, start_xvfb, stop_process

BACKENDS = REPO_ROOT / "tools" / "harness" / "backends"
DEFAULT_PLUGINS_ROOT = REPO_ROOT.parent / "solaris-default-plugins"



CORE_CLIENT_TIMEOUT_SECONDS = 600.0
LOADER_LIVE_TIMEOUT_SECONDS = 600.0
LOADER_PLATFORMS = ("fabric", "neoforge", "forge")


@dataclass
class ProfileContext:
    artifact_dir: Path
    extra_args: list[str] = field(default_factory=list)
    release: bool = False
    timeout_seconds: float | None = None
    platform: str | None = None
    # A named scenario's identity, set by `cmd_run` when the catalog entry
    # declares it. `None` keeps every profile's own default/override semantics.
    scenario_id: str | None = None
    manifest: str | None = None
    # Command evidence accumulated as the profile runs, so failure and
    # interruption receipts keep every command, exit code, and log.
    commands: list[list[str]] = field(default_factory=list)
    exits: list[int | None] = field(default_factory=list)
    logs: list[str] = field(default_factory=list)
    cwds: list[str] = field(default_factory=list)
    client: str = "java"
    client_evidence: dict[str, Any] = field(default_factory=dict)
    plugins_dir: Path | None = None
    windowed: bool = False
    # loader-live can exercise an immutable v2 candidate without building it.
    server_bin: Path | None = None


def _run_command(
    argv: list[str],
    ctx: ProfileContext,
    log_name: str,
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Capture command output and preserve evidence even when launch or waiting fails."""
    log_path = ctx.artifact_dir / log_name
    log_path.parent.mkdir(parents=True, exist_ok=True)
    started = time.time()
    ctx.commands.append([str(part) for part in argv])
    ctx.cwds.append(str(cwd or REPO_ROOT))
    ctx.exits.append(None)
    try:
        ctx.logs.append(log_path.relative_to(REPO_ROOT).as_posix())
    except ValueError:
        ctx.logs.append(str(log_path))
    proc = None
    try:
        with log_path.open("wb") as log:
            proc = subprocess.Popen(
                argv,
                cwd=str(cwd or REPO_ROOT),
                env=env if env is not None else os.environ.copy(),
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            print(
                f"[harness] stage={log_name.removesuffix('.log')} starting "
                f"pid={proc.pid} command={' '.join(str(part) for part in argv)} "
                f"log={ctx.logs[-1]}",
                flush=True,
            )
            try:
                returncode = proc.wait()
            except BaseException:
                stop_process(proc, interrupt=True)
                raise
    finally:
        if proc is not None:
            ctx.exits[-1] = proc.returncode
    duration_seconds = time.time() - started
    print(
        f"[harness] stage={log_name.removesuffix('.log')} exit={returncode} "
        f"in {duration_seconds:.1f}s log={ctx.logs[-1]}",
        flush=True,
    )
    if returncode != 0:
        raise RuntimeError(
            f"{' '.join(str(part) for part in argv)} exited {returncode}; see {log_path}"
        )
    return {"duration_seconds": duration_seconds}


def _merge_details(into: dict[str, Any], extra: dict[str, Any]) -> dict[str, Any]:
    for key, value in extra.items():
        if key in into and isinstance(into[key], list) and isinstance(value, list):
            into[key].extend(value)
        else:
            into[key] = value
    return into


def _with_backend_mode(argv: list[str], extra: list[str], modes: tuple[str, ...]) -> list[str]:
    if any(token in modes for token in extra):
        return argv + extra
    return argv + ["--run"] + extra


def _prepared_mode(argv: list[str]) -> bool:
    return "--prepare" in argv or "--check" in argv


def _backend_env(ctx: ProfileContext) -> dict[str, str]:
    """Base env for every backend: canonical run dir for child receipts."""
    env = os.environ.copy()
    env["SOLARIS_VALIDATION_RUN_DIR"] = str(ctx.artifact_dir)
    return env


def _server_binary(ctx: ProfileContext, log_name: str = "server-build.log") -> Path:
    """Resolve a frozen candidate or build the sole server executable."""
    override = os.environ.get("SOLARIS_SERVER_BINARY")
    if override:
        binary = Path(override).expanduser().resolve()
    else:
        env = os.environ.copy()
        env.setdefault("CARGO_BUILD_JOBS", "3")
        _run_command(
            ["cargo", "build", "--locked", "--bin", "solaris-v2", "-p", "mc-edge"],
            ctx, log_name, env=env,
        )
        target = Path(env.get("CARGO_TARGET_DIR", str(REPO_ROOT / "target")))
        binary = (target if target.is_absolute() else REPO_ROOT / target) / "debug" / "solaris-v2"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError(f"missing executable server binary: {binary}")
    return binary


# ---------------------------------------------------------------- cargo/jdk


def run_fmt(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["cargo", "fmt", "--all", "--", "--check"], ctx, "fmt.log"
    )


def run_clippy(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
        ctx,
        "clippy.log",
    )


def run_code_health(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["cargo", "run", "-p", "xtask", "--", "code-health"],
        ctx,
        "code-health.log",
    )


def _test_scope_memory(
    cgroup_file: Path = Path("/proc/self/cgroup"),
    cgroup_root: Path = Path("/sys/fs/cgroup"),
) -> dict[str, Any]:
    """Read the kernel's exact peak while this transient test scope still exists."""
    try:
        group = next(
            line.removeprefix("0::")
            for line in cgroup_file.read_text().splitlines()
            if line.startswith("0::")
        )
        scope = cgroup_root / group.lstrip("/")
        peak = int((scope / "memory.peak").read_text().strip())
        limit = (scope / "memory.max").read_text().strip()
    except (OSError, StopIteration, ValueError) as error:
        return {"memory_peak_bytes": None, "memory_peak_error": str(error)}
    details = {
        "memory_peak_bytes": peak,
        "memory_limit_bytes": None if limit == "max" else int(limit),
        "memory_cgroup": group,
    }
    try:
        details["memory_events"] = {
            name: int(value)
            for name, value in (
                line.split() for line in (scope / "memory.events").read_text().splitlines()
            )
        }
        details["memory_swap_peak_bytes"] = int(
            (scope / "memory.swap.peak").read_text().strip()
        )
    except (OSError, ValueError) as error:
        details["memory_pressure_error"] = str(error)
    return details


def run_test(ctx: ProfileContext, *, full: bool = False) -> dict[str, Any]:
    # One runner for both the full L2 gate and focused selectors. Nextest rejects
    # unsupported Cargo/libtest arguments rather than changing execution engines.
    non_execution = {"--no-run", "--help", "-h", "--version", "-V"}
    if any(arg in non_execution for arg in ctx.extra_args):
        raise ValueError("the test gate must execute tests; preparation/help flags are unsupported")
    env = os.environ.copy()
    env["NEXTEST_TEST_THREADS"] = env.get(
        "RUST_TEST_THREADS", env.get("NEXTEST_TEST_THREADS", "num-cpus")
    )
    # CI sets SOLARIS_NEXTEST_PROFILE=ci (short run, slow tests filtered out)
    # for `run test`; the L2 `run correctness` gate always runs the full suite.
    profile = "default" if full else env.get("SOLARIS_NEXTEST_PROFILE", "default")
    argv = [
        "cargo", "nextest", "run",
        "--profile", profile,
        *(["--ignore-default-filter"] if profile == "default" else []),
        "--no-fail-fast", "--no-tests=fail",
        *(ctx.extra_args or ["--workspace", "--all-targets"]),
    ]
    details = _run_command(argv, ctx, "test.log", env=env)
    details.update(_test_scope_memory())
    details["rust_test_threads_env"] = env.get("RUST_TEST_THREADS")
    details["test_runner"] = "nextest"
    details["nextest_test_threads_env"] = env["NEXTEST_TEST_THREADS"]
    details["nextest_profile"] = profile
    return details


def run_correctness(ctx: ProfileContext) -> dict[str, Any]:
    """The four L2 gates: fmt, clippy, code-health, then the test suite."""
    details: dict[str, Any] = {}
    _merge_details(details, run_fmt(ctx))
    _merge_details(details, run_clippy(ctx))
    _merge_details(details, run_code_health(ctx))
    _merge_details(details, run_test(ctx, full=True))
    return details


def run_build(ctx: ProfileContext) -> dict[str, Any]:
    argv = ["cargo", "build", "--locked", "-p", "mc-edge", "--bin", "solaris-v2"]
    if ctx.release:
        argv.append("--release")
    return _run_command(argv, ctx, "build.log")


def run_java(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        [
            str(AGENT_ROOT / "gradlew"),
            "--no-daemon",
            "--console=plain",
            ":bridge-core:test",
            ":java-agent:test",
            ":loader-core:test",
            ":loader-fabric:test",
            ":loader-neoforge:test",
            ":loader-forge:test",
        ],
        ctx,
        "java.log",
        cwd=AGENT_ROOT,
    )


def run_fixture_check(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["bash", "tools/build-loader-live-gate-fixture.sh", "--check"],
        ctx,
        "fixture-check.log",
    )

def _run_first_party_gate(ctx: ProfileContext, profile: str) -> dict[str, Any]:
    plugins_root = Path(
        os.environ.get("SOLARIS_DEFAULT_PLUGINS_ROOT", str(DEFAULT_PLUGINS_ROOT))
    )
    argv = [
        sys.executable,
        str(plugins_root / "tools" / "run_integration.py"),
        profile,
        "--core-root",
        str(REPO_ROOT),
        "--artifact-dir",
        str(ctx.artifact_dir),
    ]
    if profile == "standard-pack" and "--real-client" in ctx.extra_args:
        argv.extend(["--real-client", "--timeout-seconds", str(
            ctx.timeout_seconds or CORE_CLIENT_TIMEOUT_SECONDS
        )])
    return _run_command(
        argv, ctx, f"{profile}.log", cwd=plugins_root, env=_backend_env(ctx)
    )


def run_standard_pack(ctx: ProfileContext) -> dict[str, Any]:
    return _run_first_party_gate(ctx, "standard-pack")


def run_installer(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["bash", str(BACKENDS / "installer.sh")] + ctx.extra_args,
        ctx,
        "installer.log",
        env=_backend_env(ctx),
    )




def run_survival(ctx: ProfileContext) -> dict[str, Any]:
    """One ordinary survival scenario, driven over MCP for either client."""
    from .backends import gameplay

    return gameplay.run(ctx)
def run_title_connect(ctx: ProfileContext) -> dict[str, Any]:
    """No-`--server` Title/connect windowed proof for the native client."""
    from .backends import title_connect

    return title_connect.run(ctx)

def run_dialogs(ctx: ProfileContext) -> dict[str, Any]:
    """Server-driven dialog round trip (show_dialog -> custom_click_action) on the native client."""
    from .backends import dialogs

    return dialogs.run(ctx)


def run_admin_commands(ctx: ProfileContext) -> dict[str, Any]:
    """Vanilla /tellraw, /title, /bossbar, /tick and /locate observed through the native client."""
    from .backends import admin_commands

    return admin_commands.run(ctx)


def run_anti_xray(ctx: ProfileContext) -> dict[str, Any]:
    """Anti-xray gate: enclosed ores are absent from the native client's world until exposed."""
    from .backends import anti_xray

    return anti_xray.run(ctx)


def run_block_log(ctx: ProfileContext) -> dict[str, Any]:
    """Block-log gate: grief, /co rollback, restart and /co restore on the native client."""
    from .backends import block_log

    return block_log.run(ctx)

def run_chisel(ctx: ProfileContext) -> dict[str, Any]:
    """Chisel addon gate: plugin block-tool conversions on the native client."""
    from .backends import chisel

    return chisel.run(ctx)
def run_village_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Issue #1 settlement acceptance on the native client (windowed for evidence)."""
    from .backends import village_qa

    return village_qa.run(ctx)


def run_village_autonomy_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Generated-village autonomy: ownerless growth, no-give building, claim fencing (native)."""
    from .backends import village_autonomy_qa

    return village_autonomy_qa.run(ctx)

def run_village_owner_order_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Short owner-order persistence and admission scenario."""
    from .backends import village_autonomy_qa

    return village_autonomy_qa.run(ctx, owner_order=True)



def run_scene_shot(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import scene_shot

    return scene_shot.run(ctx)


def run_claim_only_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Claim/fencing/restart checklist on a settled autonomous world (native)."""
    from .backends import claim_only_qa

    return claim_only_qa.run(ctx)

def run_buildings_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Millenaire buildings: every blueprint L1..N built by a real Builder, block-match + function (native)."""
    from .backends import buildings_qa

    return buildings_qa.run(ctx)


def run_army_qa(ctx: ProfileContext) -> dict[str, Any]:
    """Millenaire army: barracks L1/L2 slots, dialog-only recruit/order/dismiss, combat, adversarial, restart (native)."""
    from .backends import army_qa

    return army_qa.run(ctx)

def run_core_client(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import client_probes

    return client_probes.run(ctx, "core-client")


def run_inventory(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import client_probes

    return client_probes.run(ctx, "inventory")


def run_entities(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import client_probes

    return client_probes.run(ctx, "entities")


def run_native_lifecycle(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import client_probes

    return client_probes.run(ctx, "native-lifecycle")


def run_native_logical(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import native_logical

    return native_logical.run(ctx)


def run_pause_empty(ctx: ProfileContext) -> dict[str, Any]:
    """Empty-server pause: frozen tick, console save, status and protocol join resume."""
    from .backends import pause_empty

    return pause_empty.run(ctx)

def run_item_merge(ctx: ProfileContext) -> dict[str, Any]:
    """Real item consolidation followed by hopper transfer observed from a chest."""
    from .backends import item_merge

    return item_merge.run(ctx)


def run_graves(ctx: ProfileContext) -> dict[str, Any]:
    """Death custody proven on the saved world: barrel contents, no inventory or drop copy."""
    from .backends import graves

    return graves.run(ctx)


def run_native_scale(ctx: ProfileContext) -> dict[str, Any]:
    from .backends import native_scale

    return native_scale.run(ctx)


def run_loader_live(ctx: ProfileContext) -> dict[str, Any]:
    from . import loader

    platform = ctx.platform or _flag_value(ctx.extra_args, "--platform")
    if platform not in LOADER_PLATFORMS:
        raise RuntimeError(
            f"loader-live requires --platform {'|'.join(LOADER_PLATFORMS)}, "
            f"got {platform!r}"
        )
    if ctx.server_bin is None or ctx.plugins_dir is None:
        raise ValueError("loader-live requires --server-bin and --plugins-dir")
    if ctx.scenario_id is None:
        raise ValueError("loader-live requires a named scenario")
    from .backends.gameplay import prepare_operator_fixture

    prepare_operator_fixture(ctx, [loader.USERNAMES[platform]])
    details: dict[str, Any] = {}
    timeout = ctx.timeout_seconds or LOADER_LIVE_TIMEOUT_SECONDS
    scenario = None
    if ctx.scenario_id == "loader-v2-settlements-smoke":
        from .loader_settlements import run

        scenario = {"run": run}
    elif ctx.scenario_id is not None:
        # Named first-party scenarios normally live beside their sibling package.
        plugins_root = Path(
            os.environ.get("SOLARIS_DEFAULT_PLUGINS_ROOT", str(DEFAULT_PLUGINS_ROOT))
        )
        scenario = runpy.run_path(str(plugins_root / "tools" / f"{ctx.scenario_id}.py"))
    result = loader.run(
        platform, timeout, ctx.artifact_dir, scenario,
        server_bin=ctx.server_bin,
        plugins_dir=ctx.plugins_dir,
    )
    details["scenario"] = result
    if result.get("passed") is not True:
        raise RuntimeError(f"loader-live scenario did not pass: {result}")
    return details

def _flag_value(args: list[str], flag: str) -> str | None:
    for index, token in enumerate(args):
        if token == flag and index + 1 < len(args):
            return args[index + 1]
        if token.startswith(flag + "="):
            return token[len(flag) + 1 :]
    return None


_REGRESSION_MODES = ("--check", "--prepare", "--run", "--validate-run")


def _regression_env(ctx: ProfileContext) -> dict[str, str]:
    env = _backend_env(ctx)
    env.setdefault("SOLARIS_REAL_CLIENT_RUN_ROOT", str(ctx.artifact_dir / "regression"))
    if ctx.timeout_seconds is not None:
        seconds = int(ctx.timeout_seconds)
        if seconds <= 0 or seconds != ctx.timeout_seconds:
            raise ValueError("regression, playable and replay require a positive whole-second timeout")
        env["SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS"] = str(seconds)
    return env


def _run_client_backend(
    argv: list[str], ctx: ProfileContext, log_name: str, *, env: dict[str, str]
) -> dict[str, Any]:
    if not any(mode in argv for mode in ("--run", "--real-client", "--all")):
        return _run_command(argv, ctx, log_name, env=env)
    xvfb, display, display_log = start_xvfb(ctx.artifact_dir)
    try:
        env["DISPLAY"] = display
        env.pop("WAYLAND_DISPLAY", None)
        result = _run_command(argv, ctx, log_name, env=env)
        result["display"] = display
        return result
    finally:
        stop_process(xvfb)
        display_log.close()
        ctx.logs.append(str(display_log.name))


def run_regression(ctx: ProfileContext) -> dict[str, Any]:
    argv = _with_backend_mode(
        ["bash", str(BACKENDS / "regression.sh")], ctx.extra_args, _REGRESSION_MODES
    )
    details = _run_client_backend(argv, ctx, "regression.log", env=_regression_env(ctx))
    details["mode"] = "prepared" if _prepared_mode(argv) else "run"
    return details


def run_playable(ctx: ProfileContext) -> dict[str, Any]:
    env = _regression_env(ctx)
    env.setdefault(
        "SOLARIS_REAL_CLIENT_MANIFEST", "docs/playable/real-client-playable-loop.json"
    )
    env.setdefault("SOLARIS_REAL_CLIENT_SERVER_CONFIG", "playable.toml")
    env.setdefault("SOLARIS_REAL_CLIENT_FRESH_WORLD", "1")
    env.setdefault(
        "SOLARIS_REAL_CLIENT_AGENT_SCENARIO", "playable-04-twenty-minute-survival-loop"
    )
    env.setdefault("SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS", "1500")
    argv = _with_backend_mode(
        ["bash", str(BACKENDS / "regression.sh")], ctx.extra_args, _REGRESSION_MODES
    )
    details = _run_client_backend(argv, ctx, "playable.log", env=env)
    details["mode"] = "prepared" if _prepared_mode(argv) else "run"
    return details


def run_replay(ctx: ProfileContext) -> dict[str, Any]:
    env = _regression_env(ctx)
    # A named scenario binds its declared identity; a bare profile run keeps the
    # documented environment overrides (setdefault, not assignment).
    if ctx.manifest is not None:
        env["SOLARIS_REAL_CLIENT_MANIFEST"] = ctx.manifest
    else:
        env.setdefault(
            "SOLARIS_REAL_CLIENT_MANIFEST",
            "docs/real-client-regression/manifests/core-replay-seed-81.json",
        )
    if ctx.scenario_id is not None:
        env["SOLARIS_REAL_CLIENT_AGENT_SCENARIO"] = ctx.scenario_id
    else:
        env.setdefault("SOLARIS_REAL_CLIENT_AGENT_SCENARIO", "core-actions-seed-81")
    env.setdefault("SOLARIS_REAL_CLIENT_FRESH_WORLD", "1")
    env.setdefault("SOLARIS_REAL_CLIENT_TIMEOUT_SECONDS", "180")
    argv = _with_backend_mode(
        ["bash", str(BACKENDS / "regression.sh")], ctx.extra_args, _REGRESSION_MODES
    )
    details = _run_client_backend(argv, ctx, "replay.log", env=env)
    details["mode"] = "prepared" if _prepared_mode(argv) else "run"
    return details


def run_oracle(ctx: ProfileContext) -> dict[str, Any]:
    """Without --run this is readiness-only (blocked/degraded), never a pass."""
    env = _backend_env(ctx)
    env.setdefault("M79_ORACLE_REPORT_DIR", str(ctx.artifact_dir / "oracle"))
    argv = ["bash", str(BACKENDS / "oracle.sh")] + ctx.extra_args
    details = _run_command(argv, ctx, "oracle.log", env=env)
    details["mode"] = "run" if "--run" in argv else "prepared"
    return details


def run_oracle_check(ctx: ProfileContext) -> dict[str, Any]:
    env = _backend_env(ctx)
    env.setdefault("M79_ORACLE_CHECK_ROOT", str(ctx.artifact_dir / "oracle-check"))
    return _run_command(
        ["bash", str(BACKENDS / "oracle_check.sh")] + ctx.extra_args,
        ctx,
        "oracle-check.log",
        env=env,
    )




def run_catalog_load(ctx: ProfileContext) -> dict[str, Any]:
    """Explicit release artifacts; fixture preparation uses public v2 commands."""
    extra = ctx.extra_args[1:] if ctx.extra_args[:1] == ["--"] else ctx.extra_args
    details = _run_command(
        [sys.executable, "-m", "tools.harness.backends.catalog_load",
         "--out", str(ctx.artifact_dir / "catalog-load")] + extra,
        ctx, "catalog-load.log", env=_backend_env(ctx),
    )
    final_event = json.loads((ctx.artifact_dir / "catalog-load.log").read_text().splitlines()[-1])
    receipt_path = Path(final_event["receipt"])
    backend = json.loads(receipt_path.read_text())
    details.update({"backend_receipt": str(receipt_path), "backend_outcome": backend["outcome"],
                    "scenario_outcomes": backend["results"]})
    if backend["smoke_only"] or backend["outcome"] != "PASS":
        details["mode"] = "prepared"
    return details


def run_quirk_parity(ctx: ProfileContext) -> dict[str, Any]:
    """Vanilla-26.1.2-oracle parity of player-relied quirks and farm rates."""
    extra = ctx.extra_args[1:] if ctx.extra_args[:1] == ["--"] else ctx.extra_args
    out = ctx.artifact_dir / "quirk-parity"
    details = _run_command(
        [sys.executable, "-m", "tools.harness.quirk_parity", "--out", str(out)] + extra,
        ctx, "quirk-parity.log", env=_backend_env(ctx),
    )
    details["receipt"] = str(out / "receipt.json")
    return details

def run_constantinople(ctx: ProfileContext) -> dict[str, Any]:
    """Functional imported-city fixture; no throughput or performance verdict."""
    extra = ctx.extra_args[1:] if ctx.extra_args[:1] == ["--"] else ctx.extra_args
    native = "--native" in extra
    extra = [argument for argument in extra if argument != "--native"]
    details = _run_command(
        [sys.executable, "-m", "tools.coreperf.constantinople", "native-smoke" if native else "smoke",
         "--out", str(ctx.artifact_dir / "constantinople")] + extra,
        ctx, "constantinople.log", env=_backend_env(ctx),
    )
    final_event = json.loads((ctx.artifact_dir / "constantinople.log").read_text().splitlines()[-1])
    details.update({"fixture_receipt": final_event["receipt"], "performance_claim": False})
    return details



def run_seed_review(ctx: ProfileContext) -> dict[str, Any]:
    details = _run_command(
        ["python3", "-m", "tools.harness.backends.seed_review"] + ctx.extra_args,
        ctx,
        "seed-review.log",
        env=_backend_env(ctx),
    )
    details["mode"] = "prepared" if _prepared_mode(ctx.extra_args) else "run"
    return details


def run_seed_contact_sheet(ctx: ProfileContext) -> dict[str, Any]:
    """Capture preparation for the seed contact sheet; never a gameplay pass."""
    details = _run_command(
        ["python3", str(BACKENDS / "seed_contact_sheet.py")] + ctx.extra_args,
        ctx,
        "seed-contact-sheet.log",
        env=_backend_env(ctx),
    )
    details["mode"] = "prepared"
    return details


def run_bucket_resync(ctx: ProfileContext) -> dict[str, Any]:
    details = _run_client_backend(
        ["bash", str(BACKENDS / "bucket_resync.sh")] + ctx.extra_args,
        ctx,
        "bucket-resync.log",
        env=_backend_env(ctx),
    )
    details["mode"] = "prepared" if _prepared_mode(ctx.extra_args) else "run"
    return details


def run_harness_check(ctx: ProfileContext) -> dict[str, Any]:
    _run_command(
        [sys.executable, "-m", "unittest", "tools.harness.test_harness"],
        ctx,
        "harness-tests.log",
    )
    for backend in sorted(BACKENDS.glob("*.sh")):
        _run_command(["bash", "-n", str(backend)], ctx, f"syntax-{backend.stem}.log")
    return {}


def run_dh_gate(ctx: ProfileContext) -> dict[str, Any]:
    """Run the real Java/DH protocol-16 baseline with fail-closed prerequisites."""
    from .backends import dh_gate

    return dh_gate.run(ctx)


Profile = Callable[[ProfileContext], dict[str, Any]]

PROFILES: dict[str, dict[str, Any]] = {
    "harness-check": {
        "scope": "harness",
        "description": "Real process/receipt regression tests and private shell-engine syntax",
        "run": run_harness_check,
    },
    "correctness": {
        "scope": "rust",
        "description": "L2 gates: fmt, clippy, code-health, nextest workspace/all-target tests",
        "run": run_correctness,
    },
    "fmt": {
        "scope": "rust",
        "description": "cargo fmt --all -- --check",
        "run": run_fmt,
    },
    "clippy": {
        "scope": "rust",
        "description": "cargo clippy --workspace --all-targets -- -D warnings",
        "run": run_clippy,
    },
    "code-health": {
        "scope": "rust",
        "description": "cargo run -p xtask -- code-health",
        "run": run_code_health,
    },
    "test": {
        "scope": "rust",
        "description": "cargo nextest run --workspace --all-targets (Criterion test mode); trailing selectors use nextest",
        "run": run_test,
    },
    "build": {
        "scope": "rust",
        "description": "cargo build --locked -p mc-edge --bin solaris-v2; --release selects release",
        "run": run_build,
    },
    "java": {
        "scope": "loader",
        "description": "Gradle bridge/agent/loader module tests",
        "run": run_java,
    },
    "fixture-check": {
        "scope": "loader",
        "description": "verify the reproducible Loader fixture (--check)",
        "run": run_fixture_check,
    },
    "standard-pack": {
        "scope": "plugins",
        "description": "rebuild and byte-verify the explicit first-party standard-pack sibling",
        "run": run_standard_pack,
    },
    "installer": {
        "scope": "release",
        "description": "installer self-test backend",
        "run": run_installer,
    },
    "survival": {
        "scope": "real-client",
        "description": "Shared MCP survival mining, pickup and placement (--client java|native)",
        "run": run_survival,
    },
    "title-connect": {
        "scope": "real-client",
        "description": "Native Title → Multiplayer → Direct Connect, type address+name, join and disconnect back to Title (--client native, windowed for screenshots)",
        "run": run_title_connect,
    },
    "village-qa": {
        "scope": "real-client",
        "description": "Issue #1 settlement acceptance: house/workshop/storage/mine/camp, bind, door/sleep/harvest/mine/lumber PASS-FAIL (native --client, windowed for screenshots)",
        "run": run_village_qa,
    },
    "village-autonomy-qa": {
        "scope": "real-client",
        "description": "Generated-village autonomy: walk-to-village adoption, unfunded Storage L1 + House L1, dialog claim, second-claim refusal (native --client, windowed for screenshots)",
        "run": run_village_autonomy_qa,
    },
    "village-owner-order-qa": {
        "scope": "real-client",
        "description": "Owner-order queue, restart, real storage funding and cancellation (native, windowed)",
        "run": run_village_owner_order_qa,
    },
    "scene-shot": {
        "scope": "real-client",
        "description": "Scripted windowed native or vanilla-Java (--client java, rendering oracle) scene captures on an isolated world",
        "run": run_scene_shot,
    },
    "claim-only-qa": {
        "scope": "real-client",
        "description": "Claim/fencing/restart checklist on a settled autonomous world (native --client, windowed)",
        "run": run_claim_only_qa,
    },
    "buildings-qa": {
        "scope": "real-client",
        "description": "Millenaire buildings: every blueprint L1..N built by a real Builder, block-match + function (native --client, windowed for screenshots)",
        "run": run_buildings_qa,
    },
    "army-qa": {
        "scope": "real-client",
        "description": "Millenaire army: barracks L1/L2 recruit slots, dialog-only orders, combat, adversarial, restart (native --client, windowed for screenshots)",
        "run": run_army_qa,
    },
    "dialogs": {
        "scope": "real-client",
        "description": "Native server-driven dialog: show_dialog with text/slider/checkbox inputs, submit, exact payload at the server (--client native, windowed for screenshots)",
        "run": run_dialogs,
    },
    "admin-commands": {
        "scope": "real-client",
        "description": "Vanilla admin commands as an operator player: /tellraw, /title, /bossbar, /tick freeze|step|sprint|rate|query and /locate biome|poi|structure read back from the client's chat and HUD nodes (native --client, windowed for screenshots)",
        "run": run_admin_commands,
    },
    "anti-xray": {
        "scope": "real-client",
        "description": "Anti-xray gate: an enclosed diamond ore is absent from the client's world data until survival mining beside it reveals it (native --client, windowed for screenshots)",
        "run": run_anti_xray,
    },
    "block-log": {
        "scope": "real-client",
        "description": "Block-log gate: grief (break/place/steal/break chest), /co lookup + rollback restores blocks and chest contents exactly, restart keeps history, /co restore re-applies (native --client, windowed for screenshots)",
        "run": run_block_log,
    },
    "chisel": {
        "scope": "real-client",
        "description": "Chisel addon gate: plugin block-tool conversions of stone_bricks/stairs to slab/stairs with wear and slab grants (--client native, windowed for screenshots)",
        "run": run_chisel,
    },
    "core-client": {
        "scope": "real-client",
        "description": "Java compatibility gate; native is explicitly a cheap server/own-chat subset",
        "run": run_core_client,
    },
    "inventory": {
        "scope": "real-client",
        "description": "Java compatibility default; native/named inventory-client share Python crafting/cursor/selection",
        "run": run_inventory,
    },
    "entities": {
        "scope": "real-client",
        "description": "Shared MCP bounded entity feeding, damage, death and pickup (--client java|native)",
        "run": run_entities,
    },
    "native-lifecycle": {
        "scope": "real-client",
        "description": "Reconnect, received death/respawn and 30-minute RSS soak (--client native|java; --soak-seconds 0 is smoke-only)",
        "run": run_native_lifecycle,
    },
    "logical-native": {
        "scope": "functional-native",
        "description": "Cheap headless movement/look and rejected-use state subset; excludes original graphical/20-minute contracts",
        "run": run_native_logical,
    },
    "native-scale": {
        "scope": "functional-native",
        "description": "Headless native CPU/RSS canaries beside real Rust protocol bots; non-exclusive diagnostic only",
        "run": run_native_scale,
    },
    "pause-empty": {
        "scope": "functional-native",
        "description": "Empty-server pause: pause log, frozen tick across console save, status while paused, offline protocol join resume, re-pause after leave",
        "run": run_pause_empty,
    },
    "item-merge": {
        "scope": "functional-native",
        "description": "Live identical item merge, hopper pickup, and chest stack contents over the protocol wire",
        "run": run_item_merge,
    },
    "graves": {
        "scope": "functional-native",
        "description": "Death custody on the saved world: real protocol death, clean stop, then world probe asserts the barrel holds exactly the given stacks and no inventory/drop copy exists (requires --plugins-dir with the graves package)",
        "run": run_graves,
    },
    "loader-live": {
        "scope": "real-client",
        "description": "Loader gate via loader.run; requires --platform fabric|neoforge|forge",
        "run": run_loader_live,
    },
    "dh-gate": {
        "scope": "real-client",
        "description": "Real Java DH baseline: MC 26.1.2 + DH 3.3.3 protocol 16, isolated Fabric/NeoForge client gate",
        "run": run_dh_gate,
    },
    "regression": {
        "scope": "real-client",
        "description": "real-client regression backend (--run unless a mode is given)",
        "run": run_regression,
    },
    "playable": {
        "scope": "real-client",
        "description": "twenty-minute survival loop gate (thin-wrapper defaults are profile data)",
        "run": run_playable,
    },
    "replay": {
        "scope": "real-client",
        "description": "core replay seed-81 gate (thin-wrapper defaults are profile data)",
        "run": run_replay,
    },
    "oracle": {
        "scope": "oracle",
        "description": "M79 oracle suite backend (--run for gameplay; bare is readiness-only)",
        "run": run_oracle,
    },
    "oracle-check": {
        "scope": "oracle",
        "description": "oracle runner self-check backend",
        "run": run_oracle_check,
    },
    "quirk-parity": {
        "scope": "oracle",
        "description": "Real 26.1.2 vanilla oracle vs Solaris: player-relied quirks (QC, TNT duping, update order) and farm rates; receipts under the run artifact dir",
        "run": run_quirk_parity,
    },
    "catalog-load": {
        "scope": "scale",
        "description": "Owner catalog v2 open-loop controls, exact wall windows and classified receipts (explicit release binaries)",
        "run": run_catalog_load,
    },
    "constantinople": {
        "scope": "fixture",
        "description": "Imported vanilla city: deterministic population, street walking, native save/reopen (not performance)",
        "run": run_constantinople,
    },
    "seed-review": {
        "scope": "terrain",
        "description": "seed owner-review backend (module invocation)",
        "run": run_seed_review,
    },
    "seed-contact-sheet": {
        "scope": "terrain",
        "description": "seed contact-sheet capture preparation (never a gameplay pass)",
        "run": run_seed_contact_sheet,
    },
    "bucket-resync": {
        "scope": "debug",
        "description": "bucket resync debug-loop backend",
        "run": run_bucket_resync,
    },
}


def list_profiles() -> dict[str, dict[str, str]]:
    return {
        name: {"scope": spec["scope"], "description": spec["description"]}
        for name, spec in PROFILES.items()
    }
