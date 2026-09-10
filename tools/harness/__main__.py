#!/usr/bin/env python3
"""Canonical harness CLI: ``python3 -m tools.harness``.

Subcommands:
  list                  enumerate named profiles and their scopes
  run PROFILE           run one profile; emit a fail-closed result.json
  client [--platform P] [--check]
                        launch (or check) a real client; replaces the two
                        run-*-client-mcp.sh launchers
  mcp ...               route to the client MCP smoke (tools/harness/mcp.py)
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

from .profiles import LOADER_PLATFORMS, ProfileContext, list_profiles
from .runtime import AGENT_ROOT, REPO_ROOT, USERNAME_RE, port_open

ARTIFACT_ROOT = REPO_ROOT / ".analysis" / "validation"

CLIENT_DEFAULT_PORT = 39095
CLIENT_DEFAULT_GAME_DIR = AGENT_ROOT / "fabric-agent" / "run-mcp"
CLIENT_DEFAULT_USERNAME = "SolarisMcp"
LOADER_DEFAULT_USERNAME = "SolarisLoader"


# ------------------------------------------------------------ client settings


def resolve_client_settings(platform: str | None) -> dict[str, Any]:
    """Validate token/port/game-dir/username, mirroring the old launchers."""
    if platform is not None and platform not in LOADER_PLATFORMS:
        raise ValueError(f"unknown Loader platform: {platform}")
    token = os.environ.get("SOLARIS_CLIENT_MCP_TOKEN", "")
    if not token:
        raise RuntimeError("set SOLARIS_CLIENT_MCP_TOKEN to a random bearer token")
    raw_port = os.environ.get("SOLARIS_CLIENT_MCP_PORT", str(CLIENT_DEFAULT_PORT))
    try:
        port = int(raw_port, 10)
    except ValueError:
        raise RuntimeError(f"invalid SOLARIS_CLIENT_MCP_PORT: {raw_port!r}") from None
    if not 1 <= port <= 65535:
        raise RuntimeError(f"invalid SOLARIS_CLIENT_MCP_PORT: {raw_port!r}")
    default_game_dir = (
        str(REPO_ROOT / ".analysis" / "minecraft-loader-mcp" / platform)
        if platform is not None
        else str(CLIENT_DEFAULT_GAME_DIR)
    )
    raw_game_dir = os.environ.get("SOLARIS_CLIENT_MCP_GAME_DIR", default_game_dir)
    game_dir = (
        Path(raw_game_dir)
        if Path(raw_game_dir).is_absolute()
        else REPO_ROOT / raw_game_dir
    )
    default_username = LOADER_DEFAULT_USERNAME if platform is not None else CLIENT_DEFAULT_USERNAME
    username = os.environ.get("SOLARIS_CLIENT_MCP_USERNAME", default_username)
    if not USERNAME_RE.match(username or ""):
        raise RuntimeError(
            "invalid SOLARIS_CLIENT_MCP_USERNAME: "
            "use 1..16 ASCII letters, digits, or underscores"
        )
    return {
        "platform": platform,
        "token": token,
        "port": port,
        "game_dir": game_dir,
        "username": username,
    }


def check_java_25() -> None:
    if not _command_available("java"):
        raise RuntimeError("Java 25 is required but java is not available")
    probe = subprocess.run(
        ["java", "-XshowSettings:properties", "-version"],
        capture_output=True,
        text=True,
        check=False,
    )
    version = ""
    for line in (probe.stderr + probe.stdout).splitlines():
        stripped = line.strip()
        if stripped.startswith("java.specification.version ="):
            version = stripped.split("=", 1)[1].strip()
            break
    if version != "25":
        raise RuntimeError(
            f"Java 25 is required; found specification version {version or 'unknown'}"
        )


def _command_available(name: str) -> bool:
    return any(
        (Path(entry) / name).is_file()
        for entry in os.environ.get("PATH", "").split(os.pathsep)
        if entry
    )


def client_check_commands(
    platform: str | None, settings: dict[str, Any]
) -> list[list[str]]:
    game_dir = str(settings["game_dir"])
    username = str(settings["username"])
    if platform is None:
        return [
            [
                str(AGENT_ROOT / "gradlew"),
                "--no-configuration-cache",
                "-p",
                str(AGENT_ROOT),
                f"-Psolaris.clientMcp.gameDir={game_dir}",
                f"-Psolaris.clientMcp.username={username}",
                ":fabric-agent:validateClientMcpRunProperties",
                ":bridge-core:test",
                "--tests",
                "dev.solaris.agent.mcp.McpHttpServerTest",
            ]
        ]
    return [
        [
            str(AGENT_ROOT / "gradlew"),
            "--no-configuration-cache",
            "-p",
            str(AGENT_ROOT),
            f"-Psolaris.clientMcp.gameDir={game_dir}",
            f"-Psolaris.clientMcp.username={username}",
            ":validateLoaderClientMcpRunProperties",
            ":java-agent:jar",
        ],
        [
            str(AGENT_ROOT / "gradlew"),
            "--no-configuration-cache",
            "--dry-run",
            "-p",
            str(AGENT_ROOT),
            f"-Psolaris.clientMcp.gameDir={game_dir}",
            f"-Psolaris.clientMcp.username={username}",
            f":loader-{platform}:runClientMcp",
        ],
    ]


def client_launch_command(
    platform: str | None, settings: dict[str, Any]
) -> list[str]:
    game_dir = str(settings["game_dir"])
    username = str(settings["username"])
    task = ":fabric-agent:runClientMcp" if platform is None else f":loader-{platform}:runClientMcp"
    return [
        str(AGENT_ROOT / "gradlew"),
        "--no-configuration-cache",
        "-p",
        str(AGENT_ROOT),
        f"-Psolaris.clientMcp.gameDir={game_dir}",
        f"-Psolaris.clientMcp.username={username}",
        task,
    ]


def run_client(platform: str | None, check: bool) -> int:
    try:
        settings = resolve_client_settings(platform)
    except (RuntimeError, ValueError) as error:
        print(f"harness client: {error}", file=sys.stderr)
        return 2
    gradlew = AGENT_ROOT / "gradlew"
    if not (gradlew.is_file() and os.access(gradlew, os.X_OK)):
        print(f"harness client: missing executable Gradle wrapper: {gradlew}",
              file=sys.stderr)
        return 1
    if check:
        try:
            check_java_25()
        except RuntimeError as error:
            print(f"harness client: {error}", file=sys.stderr)
            return 1
        for argv in client_check_commands(platform, settings):
            completed = subprocess.run(argv, cwd=REPO_ROOT, check=False)
            if completed.returncode != 0:
                print(
                    f"harness client: check failed: {' '.join(argv)}",
                    file=sys.stderr,
                )
                return 1
        label = f"Loader {platform} MCP" if platform else "Minecraft MCP"
        print(f"{label} check passed; client was not launched.")
        return 0
    port = int(settings["port"])
    if port_open(port):
        print(
            f"harness client: MCP port {port} is already in use; "
            "stop the existing client or choose another port.",
            file=sys.stderr,
        )
        return 1
    game_dir = Path(settings["game_dir"])
    game_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(
        {
            "SOLARIS_CLIENT_MCP_TOKEN": str(settings["token"]),
            "SOLARIS_CLIENT_MCP_PORT": str(port),
            "SOLARIS_CLIENT_MCP_GAME_DIR": str(game_dir),
            "SOLARIS_CLIENT_MCP_USERNAME": str(settings["username"]),
        }
    )
    if platform is not None:
        print(f"Loader platform: {platform}")
    print(f"Minecraft MCP endpoint: http://127.0.0.1:{port}/mcp")
    print(f"Minecraft game directory: {game_dir}")
    # Same foreground semantics as the retired shell launchers.
    launch = client_launch_command(platform, settings)
    os.execvpe(launch[0], launch, env)
    return 1  # unreachable


# ------------------------------------------------------------------ run/receipt


def _utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def _new_artifact_dir(profile: str) -> Path:
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S")
    return Path(tempfile.mkdtemp(prefix=f"{stamp}-{profile}-", dir=str(ARTIFACT_ROOT)))


def _status_for(details: dict[str, Any]) -> str:
    """Prepared/preflight is never a pass; only real completion passes."""
    if details.get("mode") == "prepared":
        return "prepared"
    return "passed"




def cmd_list() -> int:
    for name, spec in sorted(list_profiles().items()):
        print(f"{name:20} {spec['scope']:12} {spec['description']}")
    return 0


def cmd_run(argv: list[str]) -> int:
    from . import profiles as catalog
    # The harness captures stdout into per-run server.log and observes readiness
    # there. Ordinary server launches keep tracing exclusively in log files.
    os.environ["SOLARIS_HARNESS_LOG_STDOUT"] = "1"

    parser = argparse.ArgumentParser(prog="python3 -m tools.harness run")
    parser.add_argument("profile")
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--timeout-seconds", type=float, default=None)
    parser.add_argument("--platform", default=None)
    args, extra = parser.parse_known_args(argv)
    profile = args.profile
    spec = catalog.PROFILES.get(profile)
    started_at = _utc_now()
    start_epoch = time.time()

    def finish(
        status: str,
        scope: str,
        details: dict[str, Any],
        error: str | None,
        artifact_dir: Path,
        exit_code: int,
        evidence: ProfileContext | None = None,
    ) -> int:
        ended_at = _utc_now()
        commands = [list(c) for c in (evidence.commands if evidence else [])]
        exits = list(evidence.exits if evidence else [])
        logs = list(evidence.logs if evidence else [])
        receipt = {
            "profile": profile,
            "status": status,
            "scope": scope,
            "commands": commands,
            "exits": exits,
            "logs": logs,
            "started_at": started_at,
            "ended_at": ended_at,
            "duration_seconds": round(time.time() - start_epoch, 3),
            "details": details,
            "error": error,
        }
        (artifact_dir / "result.json").write_text(
            json.dumps(receipt, indent=2, sort_keys=True) + "\n"
        )
        outcome = {"passed": "PASS", "failed": "FAIL",
                   "prepared": "PREPARED", "interrupted": "INTERRUPTED"}[status]
        print(
            f"[harness] {outcome} profile={profile} "
            f"status={status} artifact={artifact_dir} "
            f"in {receipt['duration_seconds']:.1f}s"
        )
        if error:
            print(f"[harness] error: {error}", file=sys.stderr)
        return exit_code

    if spec is None:
        ARTIFACT_ROOT.mkdir(parents=True, exist_ok=True)
        artifact_dir = _new_artifact_dir("unknown-profile")
        known = ", ".join(sorted(catalog.PROFILES))
        return finish("failed", "harness", {},

                      f"unknown profile {profile!r}; known: {known}",
                      artifact_dir, 2)
    ARTIFACT_ROOT.mkdir(parents=True, exist_ok=True)
    artifact_dir = _new_artifact_dir(profile)
    print(f"[harness] run profile={profile} artifact={artifact_dir}")
    ctx = ProfileContext(
        artifact_dir=artifact_dir,
        extra_args=extra,
        release=args.release,
        timeout_seconds=args.timeout_seconds,
        platform=args.platform,
    )
    try:
        details = spec["run"](ctx)
    except KeyboardInterrupt:
        return finish("interrupted", spec["scope"], {}, "interrupted by user",
                      artifact_dir, 130, ctx)
    except Exception as error:
        return finish("failed", spec["scope"], {}, str(error), artifact_dir, 1, ctx)
    status = _status_for(details)
    if status == "prepared":
        return finish(status, spec["scope"], details, None, artifact_dir, 0, ctx)
    return finish(status, spec["scope"], details, None, artifact_dir, 0, ctx)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python3 -m tools.harness")
    parser.add_argument("command", nargs="?", choices=["list", "run", "client", "mcp"])
    parser.add_argument("rest", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.command == "list":
        return cmd_list()
    if args.command == "run":
        if not args.rest:
            print("harness run: expected PROFILE", file=sys.stderr)
            return 2
        return cmd_run(args.rest)
    if args.command == "client":
        client = argparse.ArgumentParser(prog="python3 -m tools.harness client")
        client.add_argument("--platform", default=None)
        client.add_argument("--check", action="store_true")
        parsed = client.parse_args(args.rest)
        return run_client(parsed.platform, parsed.check)
    if args.command == "mcp":
        from . import mcp

        return mcp.main(args.rest)
    parser.print_help()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
