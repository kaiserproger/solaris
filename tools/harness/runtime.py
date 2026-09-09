#!/usr/bin/env python3
"""Shared process lifecycle and real-client readiness for the harness.

Every wait here is push-driven: port readiness is observed with
non-blocking connects plus selector waits, process exit with pidfd, and
log/file readiness with inotify or -displayfd. Nothing declares success
merely because a timeout elapsed or time passed.
"""

from __future__ import annotations

import ctypes
import errno
import os
import re
import select
import selectors
import signal
import socket
import subprocess
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
AGENT_ROOT = Path(os.environ.get("SOLARIS_LOADER_ROOT", REPO_ROOT.parent / "solaris-loader")).expanduser().resolve()

USERNAME_RE = re.compile(r"[A-Za-z0-9_]{1,16}\Z")
PLATFORMS = ("fabric", "neoforge", "forge")

_CLIENT_READY_TIMEOUT_SECONDS = 180.0
_XVFB_READY_TIMEOUT_SECONDS = 20.0
_LOG_TAIL_BYTES = 8192

_LIBC = ctypes.CDLL(None, use_errno=True)


def _pidfd_open(pid: int) -> int:
    """Return a pidfd for pid, or -1 when the platform lacks pidfd_open."""
    try:
        pidfd_open = _LIBC.pidfd_open
    except AttributeError:
        return -1
    pidfd_open.argtypes = [ctypes.c_int, ctypes.c_uint]
    pidfd_open.restype = ctypes.c_int
    fd = pidfd_open(pid, 0)
    if fd < 0:
        return -1
    return int(fd)


def _inotify_for(parent: Path) -> int:
    """Return an inotify fd watching parent, or -1 when unavailable."""
    try:
        init = _LIBC.inotify_init1
        add = _LIBC.inotify_add_watch
    except AttributeError:
        return -1
    init.argtypes = [ctypes.c_int]
    init.restype = ctypes.c_int
    add.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    add.restype = ctypes.c_int
    fd = init(os.O_CLOEXEC | os.O_NONBLOCK)
    if fd < 0:
        return -1
    # IN_MODIFY | IN_CREATE | IN_MOVED_TO | IN_DELETE_SELF.
    watch = add(int(fd), os.fsencode(parent), 0x2 | 0x100 | 0x80 | 0x400)
    if watch < 0:
        os.close(int(fd))
        return -1
    return int(fd)


def _drain(fd: int) -> None:
    try:
        while os.read(fd, 65536):
            pass
    except (BlockingIOError, OSError):
        pass


def _tail_log(log_path: Path, max_bytes: int = _LOG_TAIL_BYTES) -> str:
    try:
        with log_path.open("rb") as handle:
            handle.seek(0, os.SEEK_END)
            size = handle.tell()
            handle.seek(max(0, size - max_bytes))
            return handle.read().decode("utf-8", "replace")
    except OSError:
        return ""


def reserve_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(2.0)
        try:
            sock.connect(("127.0.0.1", port))
        except OSError:
            return False
        return True


def _probe_port(port: int, timeout_seconds: float) -> bool:
    """Non-blocking connect; True only when the port accepts.

    The wait for writability is the readiness event itself, not a sleep.
    """
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        sock.setblocking(False)
        error = sock.connect_ex(("127.0.0.1", port))
        if error not in (0, errno.EINPROGRESS, errno.EALREADY):
            return False
        _, writable, _ = select.select([], [sock], [], max(0.0, timeout_seconds))
        if not writable:
            return False
        return sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR) == 0
    except OSError:
        return False
    finally:
        sock.close()


def _require_event_fds(
    process: subprocess.Popen[Any] | None, parent: Path
) -> tuple[selectors.BaseSelector, int, int]:
    """Selector over pidfd + inotify; Linux event sources are mandatory."""
    notify_fd = _inotify_for(parent)
    if notify_fd < 0:
        raise RuntimeError("harness readiness requires Linux inotify")
    pid_fd = -1
    if process is not None:
        if process.pid is None:
            os.close(notify_fd)
            raise RuntimeError("harness readiness requires a running process")
        pid_fd = _pidfd_open(process.pid)
        if pid_fd < 0:
            os.close(notify_fd)
            raise RuntimeError("harness readiness requires Linux pidfd")
    selector = selectors.DefaultSelector()
    selector.register(notify_fd, selectors.EVENT_READ)
    if pid_fd >= 0:
        selector.register(pid_fd, selectors.EVENT_READ)
    return selector, notify_fd, pid_fd


def wait_port(
    port: int,
    timeout_seconds: float,
    process: subprocess.Popen[Any],
    *,
    log_path: Path,
) -> None:
    """Return once port accepts; raise if the process exits or time runs out.

    Re-probes happen only after real events: socket writability, log growth
    (inotify), or process exit (pidfd). There is no polling quantum.
    """
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    deadline = time.monotonic() + timeout_seconds
    parent = log_path.parent
    parent.mkdir(parents=True, exist_ok=True)
    selector, notify_fd, pid_fd = _require_event_fds(process, parent)
    try:
        while True:
            if process.poll() is not None:
                tail = _tail_log(log_path)
                raise RuntimeError(
                    f"process exited with {process.returncode} before port "
                    f"{port} became ready; log {log_path}: {tail[-2000:]}"
                )
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                tail = _tail_log(log_path)
                raise RuntimeError(
                    f"port {port} did not become ready within "
                    f"{timeout_seconds:.1f}s; log {log_path}: {tail[-2000:]}"
                )
            if _probe_port(port, remaining):
                return
            if process.poll() is not None:
                tail = _tail_log(log_path)
                raise RuntimeError(
                    f"process exited with {process.returncode} before port "
                    f"{port} became ready; log {log_path}: {tail[-2000:]}"
                )
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                tail = _tail_log(log_path)
                raise RuntimeError(
                    f"port {port} did not become ready within "
                    f"{timeout_seconds:.1f}s; log {log_path}: {tail[-2000:]}"
                )
            # Block for the full remaining time; only log growth or process
            # exit wakes us to probe again.
            for key, _ in selector.select(remaining):
                _drain(key.fd)
    finally:
        selector.close()
        os.close(notify_fd)
        if pid_fd >= 0:
            os.close(pid_fd)


def stop_process(process: subprocess.Popen[Any] | None, *, interrupt: bool = False) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGINT if interrupt else signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        return
    try:
        process.wait(timeout=8)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
        process.wait(timeout=5)


def _wait_for_file(path: Path, timeout_seconds: float, process: subprocess.Popen[Any]) -> None:
    """Block until path exists; raise if process exits first (event-driven)."""
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    deadline = time.monotonic() + timeout_seconds
    path.parent.mkdir(parents=True, exist_ok=True)
    selector, notify_fd, pid_fd = _require_event_fds(process, path.parent)
    try:
        while not path.exists():
            if process.poll() is not None:
                raise RuntimeError(
                    f"Xvfb exited with {process.returncode} before {path} appeared"
                )
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeError(f"Xvfb socket {path} did not appear in time")
            for key, _ in selector.select(remaining):
                _drain(key.fd)
    finally:
        selector.close()
        os.close(notify_fd)
        if pid_fd >= 0:
            os.close(pid_fd)


def start_xvfb(artifact_dir: Path) -> tuple[subprocess.Popen[Any], str, Any]:
    """Start Xvfb and return (process, display, log_handle).

    Readiness comes from Xvfb's -displayfd announcement (or the X socket
    file event on fallback), never from assuming survival after a delay.
    """
    artifact_dir.mkdir(parents=True, exist_ok=True)
    log_handle = (artifact_dir / "xvfb.log").open("wb")
    read_fd, write_fd = os.pipe()
    try:
        try:
            process = subprocess.Popen(
                [
                    "Xvfb",
                    "-displayfd",
                    str(write_fd),
                    "-screen",
                    "0",
                    "1280x720x24",
                    "-nolisten",
                    "tcp",
                ],
                cwd=REPO_ROOT,
                stdout=log_handle,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                pass_fds=(write_fd,),
            )
        except (FileNotFoundError, OSError):
            return _start_xvfb_scan(artifact_dir, log_handle)
        finally:
            try:
                os.close(write_fd)
            except OSError:
                pass
        pid_fd = _pidfd_open(process.pid)
        deadline = time.monotonic() + _XVFB_READY_TIMEOUT_SECONDS
        display_number = b""
        selector = selectors.DefaultSelector()
        try:
            selector.register(read_fd, selectors.EVENT_READ)
            if pid_fd >= 0:
                selector.register(pid_fd, selectors.EVENT_READ)
            while True:
                if process.poll() is not None:
                    # -displayfd unsupported or Xvfb failed: fall back to scan.
                    selector.close()
                    if pid_fd >= 0:
                        os.close(pid_fd)
                    stop_process(process)
                    return _start_xvfb_scan(artifact_dir, log_handle)
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    stop_process(process)
                    raise RuntimeError("Xvfb did not announce a display via -displayfd")
                for key, _ in selector.select(remaining):
                    if key.fd == read_fd:
                        chunk = os.read(read_fd, 64)
                        if chunk:
                            display_number += chunk
                            if display_number.endswith(b"\n"):
                                display = f":{display_number.decode().strip()}"
                                return process, display, log_handle
                    else:
                        _drain(key.fd)
        finally:
            try:
                selector.close()
            except Exception:
                pass
            if pid_fd >= 0:
                try:
                    os.close(pid_fd)
                except OSError:
                    pass
    finally:
        try:
            os.close(read_fd)
        except OSError:
            pass


def _start_xvfb_scan(
    artifact_dir: Path, log_handle: Any
) -> tuple[subprocess.Popen[Any], str, Any]:
    for display_number in range(99, 120):
        socket_path = Path(f"/tmp/.X11-unix/X{display_number}")
        if socket_path.exists():
            continue
        process = subprocess.Popen(
            ["Xvfb", f":{display_number}", "-screen", "0", "1280x720x24", "-nolisten", "tcp"],
            cwd=REPO_ROOT,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            _wait_for_file(socket_path, _XVFB_READY_TIMEOUT_SECONDS, process)
        except RuntimeError:
            stop_process(process)
            continue
        return process, f":{display_number}", log_handle
    raise RuntimeError("could not start an isolated Xvfb display")


def current_client_state(client: Any, timeout_seconds: float) -> dict[str, Any]:
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    observed = client.call_tool("minecraft_observe")
    version = observed.get("state_version")
    if not isinstance(version, int) or version < 0:
        raise RuntimeError(f"client observation omitted state_version: {observed}")
    return observed


def next_client_state(client: Any, observed: dict[str, Any], deadline: float) -> dict[str, Any]:
    version = observed.get("state_version")
    if not isinstance(version, int) or version < 0:
        raise RuntimeError(f"client state snapshot omitted state_version: {observed}")
    remaining = deadline - time.monotonic()
    if remaining < 0.1:
        raise TimeoutError(f"client state event deadline elapsed: {observed.get('screen')}")
    # Push-driven: the client pushes a state-change notification; the call
    # returns when it fires, not when a sleep interval ends.
    client.call_tool(
        "minecraft_wait_for_state_change",
        {"observed_version": version, "timeout_seconds": min(remaining, 120.0)},
    )
    return current_client_state(client, remaining)


def retry_tool(
    client: Any, name: str, arguments: dict[str, Any], timeout_seconds: float
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    last_error: Exception | None = None
    observed: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        try:
            return client.call_tool(name, arguments)
        except Exception as error:  # MCP reports transient screen/state mismatches.
            last_error = error
            if observed is None:
                observed = current_client_state(client, min(timeout_seconds, 120.0))
            else:
                observed = next_client_state(client, observed, deadline)
    raise RuntimeError(f"{name} did not succeed within {timeout_seconds:.1f}s: {last_error}")


def wait_client_ready_for_connect(client: Any, timeout_seconds: float) -> dict[str, Any]:
    import json as _json

    deadline = time.monotonic() + timeout_seconds
    last = current_client_state(client, min(timeout_seconds, 120.0))
    while True:
        screen_payload = last.get("screen")
        if isinstance(screen_payload, dict):
            screen = str(screen_payload.get("class", ""))
        else:
            screen = str(last.get("current_screen", ""))
        if not last.get("in_play") and screen.endswith("TitleScreen"):
            return last
        if screen.endswith("LoadingErrorScreen"):
            raise RuntimeError(
                "Forge loading warning/error screen blocked bootstrap: "
                + _json.dumps(last, ensure_ascii=False, sort_keys=True)
            )
        if time.monotonic() >= deadline:
            break
        last = next_client_state(client, last, deadline)
    raise RuntimeError(
        "client bootstrap did not reach TitleScreen before connect: "
        + _json.dumps(last, ensure_ascii=False, sort_keys=True)
    )


def start_client(
    *,
    game_dir: Path,
    log_path: Path,
    token: str,
    mcp_port: int,
    username: str,
    display: str,
    platform: str | None = None,
) -> tuple[subprocess.Popen[Any], Any]:
    """Launch a real client and return (process, log_handle) once MCP is ready.

    platform None is the no-Loader Fabric agent; fabric/neoforge/forge use
    the matching Loader launch task. Prewritten game options are preserved.
    Readiness is the MCP port accepting (via log/process events); the call
    never returns merely because time passed.
    """
    if not token:
        raise ValueError("token must not be empty")
    if not isinstance(mcp_port, int) or not 1 <= mcp_port <= 65535:
        raise ValueError(f"mcp_port must be 1..65535, got {mcp_port!r}")
    if not USERNAME_RE.match(username or ""):
        raise ValueError(
            f"username must be 1..16 ASCII letters, digits, or underscores, got {username!r}"
        )
    if not display:
        raise ValueError("display must not be empty")
    if platform is not None and platform not in PLATFORMS:
        raise ValueError(f"platform must be one of {PLATFORMS}, got {platform!r}")
    game_dir = REPO_ROOT / game_dir if not game_dir.is_absolute() else game_dir
    gradlew = AGENT_ROOT / "gradlew"
    if not (gradlew.is_file() and os.access(gradlew, os.X_OK)):
        raise RuntimeError(f"missing executable Gradle wrapper: {gradlew}")
    game_dir.mkdir(parents=True, exist_ok=True)
    options = game_dir / "options.txt"
    if not options.exists():
        options.write_text("version:4790\nonboardAccessibility:false\n")
    if port_open(mcp_port):
        raise RuntimeError(
            f"MCP port {mcp_port} is already in use; stop the existing client first"
        )
    task = ":fabric-agent:runClientMcp" if platform is None else f":loader-{platform}:runClientMcp"
    env = os.environ.copy()
    env.update(
        {
            "DISPLAY": display,
            "SOLARIS_CLIENT_MCP_TOKEN": token,
            "SOLARIS_CLIENT_MCP_PORT": str(mcp_port),
            "SOLARIS_CLIENT_MCP_GAME_DIR": str(game_dir),
            "SOLARIS_CLIENT_MCP_USERNAME": username,
        }
    )
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_handle = log_path.open("wb")
    try:
        process = subprocess.Popen(
            [
                str(gradlew),
                "--no-configuration-cache",
                "-p",
                str(AGENT_ROOT),
                f"-Psolaris.clientMcp.gameDir={game_dir}",
                f"-Psolaris.clientMcp.username={username}",
                task,
            ],
            cwd=REPO_ROOT,
            env=env,
            stdout=log_handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except Exception:
        log_handle.close()
        raise
    try:
        wait_port(mcp_port, _CLIENT_READY_TIMEOUT_SECONDS, process, log_path=log_path)
    except Exception:
        stop_process(process)
        log_handle.close()
        raise
    return process, log_handle
