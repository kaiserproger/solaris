#!/usr/bin/env python3
"""Named validation profiles for the centralized harness.

Every profile is a real implementation: either direct calls into sibling
scenario modules or a maintained internal backend command. Unknown
profiles, nonzero commands, exceptions, and prepared-only runs never pass.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from .runtime import AGENT_ROOT, REPO_ROOT, start_xvfb, stop_process

BACKENDS = REPO_ROOT / "tools" / "harness" / "backends"

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
    # Command evidence accumulated as the profile runs, so failure and
    # interruption receipts keep every command, exit code, and log.
    commands: list[list[str]] = field(default_factory=list)
    exits: list[int | None] = field(default_factory=list)
    logs: list[str] = field(default_factory=list)


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
            try:
                returncode = proc.wait()
            except BaseException:
                stop_process(proc, interrupt=True)
                raise
    finally:
        if proc is not None:
            ctx.exits[-1] = proc.returncode
    if returncode != 0:
        raise RuntimeError(
            f"{' '.join(str(part) for part in argv)} exited {returncode}; see {log_path}"
        )
    return {"duration_seconds": time.time() - started}


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


def run_test(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["cargo", "test", "--workspace", "--all-targets"], ctx, "test.log"
    )


def run_correctness(ctx: ProfileContext) -> dict[str, Any]:
    """The four L2 gates: fmt, clippy, code-health, then the test suite."""
    details: dict[str, Any] = {}
    _merge_details(details, run_fmt(ctx))
    _merge_details(details, run_clippy(ctx))
    _merge_details(details, run_code_health(ctx))
    _merge_details(details, run_test(ctx))
    return details


def run_build(ctx: ProfileContext) -> dict[str, Any]:
    if ctx.release:
        argv = ["cargo", "build", "--locked", "--release", "--workspace"]
    else:
        argv = ["cargo", "build", "--bin", "mc-server"]
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


def run_installer(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["bash", str(BACKENDS / "installer.sh")] + ctx.extra_args,
        ctx,
        "installer.log",
        env=_backend_env(ctx),
    )


def _build_mc_server(ctx: ProfileContext) -> dict[str, Any]:
    """Build the server once; scenario functions must not rebuild it."""
    return _run_command(
        ["cargo", "build", "-p", "mc-server"], ctx, "server-build.log"
    )


def run_core_client(ctx: ProfileContext) -> dict[str, Any]:
    from . import compatibility

    details = _build_mc_server(ctx)
    timeout = ctx.timeout_seconds or CORE_CLIENT_TIMEOUT_SECONDS
    result = compatibility.run(timeout, ctx.artifact_dir, inventory=False)
    details["scenario"] = result
    if result.get("passed") is not True:
        raise RuntimeError(f"core-client scenario did not pass: {result}")
    return details


def run_inventory(ctx: ProfileContext) -> dict[str, Any]:
    from . import compatibility

    details = _build_mc_server(ctx)
    timeout = ctx.timeout_seconds or CORE_CLIENT_TIMEOUT_SECONDS
    result = compatibility.run(timeout, ctx.artifact_dir, inventory=True)
    details["scenario"] = result
    if result.get("passed") is not True:
        raise RuntimeError(f"inventory scenario did not pass: {result}")
    return details


def run_loader_live(ctx: ProfileContext) -> dict[str, Any]:
    from . import loader

    platform = ctx.platform or _flag_value(ctx.extra_args, "--platform")
    if platform not in LOADER_PLATFORMS:
        raise RuntimeError(
            f"loader-live requires --platform {'|'.join(LOADER_PLATFORMS)}, "
            f"got {platform!r}"
        )
    details = _build_mc_server(ctx)
    timeout = ctx.timeout_seconds or LOADER_LIVE_TIMEOUT_SECONDS
    result = loader.run(platform, timeout, ctx.artifact_dir)
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
    env.setdefault(
        "SOLARIS_REAL_CLIENT_MANIFEST",
        "docs/real-client-regression/manifests/core-replay-seed-81.json",
    )
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


def run_entity_scale(ctx: ProfileContext) -> dict[str, Any]:
    env = _backend_env(ctx)
    env.setdefault(
        "SOLARIS_ENTITY_BENCH_OUT_DIR",
        str(ctx.artifact_dir / "bench"),
    )
    return _run_command(
        ["bash", str(BACKENDS / "entity_scale.sh")] + ctx.extra_args,
        ctx,
        "entity-scale.log",
        env=env,
    )


def run_living_world_scale(ctx: ProfileContext) -> dict[str, Any]:
    env = _backend_env(ctx)
    env.setdefault(
        "SOLARIS_ENTITY_BENCH_OUT_DIR",
        str(ctx.artifact_dir / "bench"),
    )
    return _run_command(
        ["bash", str(BACKENDS / "living_world_scale.sh")] + ctx.extra_args,
        ctx,
        "living-world-scale.log",
        env=env,
    )


def run_seed_review(ctx: ProfileContext) -> dict[str, Any]:
    return _run_command(
        ["python3", "-m", "tools.harness.backends.seed_review"] + ctx.extra_args,
        ctx,
        "seed-review.log",
        env=_backend_env(ctx),
    )


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


Profile = Callable[[ProfileContext], dict[str, Any]]

PROFILES: dict[str, dict[str, Any]] = {
    "harness-check": {
        "scope": "harness",
        "description": "Real process/receipt regression tests and private shell-engine syntax",
        "run": run_harness_check,
    },
    "correctness": {
        "scope": "rust",
        "description": "L2 gates: fmt, code-health, clippy, test --workspace --all-targets",
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
        "description": "cargo test --workspace --all-targets",
        "run": run_test,
    },
    "build": {
        "scope": "rust",
        "description": "cargo build --bin mc-server (dev); --release for the locked release workspace build",
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
    "installer": {
        "scope": "release",
        "description": "installer self-test backend",
        "run": run_installer,
    },
    "core-client": {
        "scope": "real-client",
        "description": "server-only plugin gate via compatibility.run (direct call)",
        "run": run_core_client,
    },
    "inventory": {
        "scope": "real-client",
        "description": "inventory-path gate via compatibility.run(inventory=True)",
        "run": run_inventory,
    },
    "loader-live": {
        "scope": "real-client",
        "description": "Loader gate via loader.run; requires --platform fabric|neoforge|forge",
        "run": run_loader_live,
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
    "entity-scale": {
        "scope": "scale",
        "description": "entity-scale backend; evidence under the run artifact dir",
        "run": run_entity_scale,
    },
    "living-world-scale": {
        "scope": "scale",
        "description": "living-world-scale backend; evidence under the run artifact dir",
        "run": run_living_world_scale,
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
