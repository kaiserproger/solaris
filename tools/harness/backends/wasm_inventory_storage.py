"""Real graphical P3 inventory/storage, market-menu and zone-market acceptance.

Three scenarios share this module because they drive the same fixture through the
same primitives:

- ``wasm-p3-inventory-storage`` is the command-only trade terminal: explicit
  operator setup seeds four emeralds and one diamond pickaxe, and one ordinary
  survival attack then gives that pickaxe a real durability component, so the
  unaffected stack carries a non-default component the bridge can read back.
- ``wasm-p3-inventory-menu`` drives that same fixture mode through the real
  client's menu surface: ``/trade menu`` asks the guest for the server-owned
  ``trade-market`` window, a real primary/secondary click on the window's own
  slots buys or refunds, and a click on the window's close slot or
  ``/trade menu-close`` closes it. The ``P3_MENU`` request markers are ordering
  fences only - the effect is read from the client's own screen, container slots
  and inventory - and the stale probes keep the live next-menu open while the
  guest is asked to close a foreign menu and to act on the session that the
  reconnect left behind.

- ``wasm-p3-zone-market`` drives that fixture's ``zone-market`` mode through real
  boundary crossings: the operator prepares the walking strip with the server's
  own debug corridor builder, whose four end caps form one continuous twelve-block
  stone walkway at y79 across x 5..16 (the zone box starts at y80), and the
  fixture creates the fixed ``trade-zone`` box on top of that. Ordinary walking
  then crosses the box face, and the fixture's own zone entry event opens the
  market on the entering session. The trade clicks reuse the same market and the
  same ledger machine.
  The one crossing that cannot be walked is the exit with the window still open,
  because a container screen swallows ordinary movement input: that crossing is a
  real operator ``/tp`` sent while the window is open followed by the client's own
  position report, and the close that follows is the fixture's own exit-driven
  close rather than a forced one. Every crossing is asserted from the client's own
  position against the declared box, the fixture's own fence marker, and the
  window that opens or closes as its effect.

All three fixtures require every ``/trade`` submission to publish a fresh marker, and
each phase re-reads the client-visible inventory. Chat waits use the client's
versioned state notifications and inventory waits use the bridge's
producer-driven exact-count wait, so no step sleeps or polls.

The reused request id, the remembered first session, the storage revision and
the refusal reasons belong to the guest and are not observable from a graphical
client: this driver asserts only the committed/refused outcome carried by the
marker, the exact client-visible counts, the client-visible window, and that the
unaffected tool's exposed components never drift. It deliberately claims neither
an operation-id receipt nor crash durability.
"""

from __future__ import annotations

import json
import math
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

if __package__:
    from .wasm_plugin_join_hello import HarnessHooks
else:
    # Direct scripts get this path automatically; importlib/runpy file loaders
    # do not. Both entry modes need the same sibling scenario module.
    import sys

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from wasm_plugin_join_hello import HarnessHooks


WASM_INVENTORY_STORAGE_SCENARIOS = frozenset({"wasm-p3-inventory-storage"})
WASM_INVENTORY_MENU_SCENARIOS = frozenset({"wasm-p3-inventory-menu"})
WASM_ZONE_MARKET_SCENARIOS = frozenset({"wasm-p3-zone-market"})
TRADE_MARKER_PREFIX = "P3_TRADE"
MENU_MARKER_PREFIX = "P3_MENU"
ZONE_MARKER_PREFIX = "P3_ZONE"
TRADE_CYCLE_KIND = "trade"
EXIT_CYCLE_KIND = "exit"
WALK_EXIT_ROUTE = "walk"
TELEPORT_EXIT_ROUTE = "tp"
COMMITTED_OUTCOME = "committed"
REFUSED_OUTCOME = "refused"
# The screen the real client opens for the server-owned generic 9xN menu, so the
# window class a menu phase must see through the bridge.
MENU_SCREEN_CLASS = "net.minecraft.client.gui.screens.inventory.ContainerScreen"
# Every component a client-visible item stack exposes through the bridge's
# `observe` payload. Arbitrary data components and storage revisions are not
# readable from the real client and are reported as observation gaps instead.
FINGERPRINT_FIELDS = (
    "item_id",
    "count",
    "name",
    "damage",
    "max_damage",
    "foil",
    "enchantments",
)
MCP_OBSERVATION_GAPS = (
    "minecraft_observe exposes item_id, count, name, damage, max_damage, foil and "
    "enchantments only; arbitrary data components and plugin storage revisions are "
    "not readable from the real client, so a refused storage precondition is "
    "asserted from the guest's own marker plus an unchanged client inventory.",
    "No core operator command can create a custom-named or enchanted stack, so the "
    "unaffected tool's non-default component is durability damage produced by an "
    "ordinary survival attack through the bridge; a named or enchanted stack would "
    "need a new grant path the graphical surface does not have.",
    "The real client cannot observe the durable world journal; this run proves "
    "live commit/refusal and reconnect persistence, not operation-id receipts "
    "or crash durability.",
)
MCP_MENU_OBSERVATION_GAPS = (
    "The bridge offers primary and secondary container clicks, and one "
    "quick-move action that always sends the window's quick-move input with "
    "button 0. The contract's shift-primary menu click is therefore reachable "
    "(as a quick move) and its shift-secondary click has no bridge action at all; "
    "neither is part of this scenario's acceptance path, and any such click would "
    "be covered by native wire tests instead.",
    "No MCP action can forge a stale wire click or a stale session: the stale "
    "probes drive the guest's own request path, fence it with its marker, and "
    "observe that the live window and inventory are unchanged. Proving refusal at "
    "the wire belongs to the native component tests, not to this graphical driver.",
    "The real client cannot read the plugin's ledger record: the ledger value is "
    "asserted from the marker the guest writes after its own read-back, and the "
    "client-visible counts are asserted separately.",
)
MCP_ZONE_OBSERVATION_GAPS = (
    "This server has no /fill or /setblock operator command, so the platform the "
    "zone crossing stands on is prepared with its own /debug water-corridor "
    "builder: four fixed invocations at y=78 whose z=13 end caps form one "
    "continuous stone walkway at y79 across x 5..16 on the z=12 block row. Each "
    "builder command is accepted only on its own verified-block feedback line and "
    "the whole strip is then read back through the client's own block scan "
    "(solid at y79, air at y80), so a refused or partial build fails the run "
    "instead of moving the crossing to a height the client cannot stand at.",
    "The real client cannot read the server's zone table, the plugin's tracked "
    "session or the zone command's typed answer: a crossing is asserted from the "
    "client-observed position against the box the manifest declares, the guest's "
    "own P3_ZONE fence marker, and the effect that marker precedes - the market "
    "window opening on entry and closing on exit. The zone's Applied answer is "
    "the guest's own precondition for its ready marker, so an unapplied upsert "
    "shows up as a missing marker and fails the phase rather than as a claim.",
    "A container screen swallows ordinary movement input, so the one crossing "
    "that leaves the zone with the market still open is an operator /tp sent "
    "while the window is open: the driver does not close the window itself, and "
    "the closed screen after that crossing is the guest's own exit-driven close. "
    "Every other crossing in this scenario is ordinary walking. The run records "
    "which route each crossing used rather than presenting them as identical.",
    "Ordinary movement is client-authoritative: the bridge walks the real client "
    "and the box is the manifest's own declaration, so a crossing proves the "
    "client position moved across that face plus the guest's transition marker, "
    "not a server-side zone membership read the graphical client cannot make.",
    "The real client cannot observe the durable world journal; this run proves "
    "the live entry/exit transitions with a real purchase and refund and their "
    "persistence across a reconnect, not crash durability or operation-id "
    "receipts.",
)


@dataclass(frozen=True)
class TradeScenarioHooks(HarnessHooks):
    """The join/hello hooks plus the reconnect primitives both trade routes need."""

    leave_play: Callable[..., dict[str, Any]]
    await_session_release: Callable[..., str]
    await_interactive_play: Callable[..., dict[str, Any]]


def require_str(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} must be a non-empty string")
    return value


def require_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{label} must be an integer")
    return value


def require_mapping(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def load_inventory_storage_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture."""

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_inventory_storage_expectation"),
        f"{scenario_id} wasm_inventory_storage_expectation",
    )


def load_inventory_menu_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture.

    The menu fixture seeds itself with the same explicit operator commands as the
    storage fixture, so it must declare ``no_debug_commands`` false for the same
    reason.
    """

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_inventory_menu_expectation"),
        f"{scenario_id} wasm_inventory_menu_expectation",
    )


def load_zone_market_expectation(run_dir: Path, scenario_id: str) -> dict[str, Any]:
    """Read the copied run manifest and return this scenario's declared fixture.

    The zone fixture seeds itself with the same explicit operator commands - a
    prepared platform, one teleport to the fixed outside position and the trade
    fixture's own grants - so it must declare ``no_debug_commands`` false.
    """

    manifest = json.loads((run_dir / "manifest.json").read_text(encoding="utf-8"))
    scenarios = [
        entry
        for entry in manifest.get("scenarios", [])
        if isinstance(entry, dict) and entry.get("id") == scenario_id
    ]
    if len(scenarios) != 1:
        raise ValueError(f"{scenario_id} must appear exactly once in the run manifest")
    scenario = scenarios[0]
    if scenario.get("no_debug_commands") is not False:
        raise ValueError(
            f"{scenario_id} seeds its fixture with explicit operator commands and must "
            "declare no_debug_commands false"
        )
    return require_mapping(
        scenario.get("wasm_zone_market_expectation"),
        f"{scenario_id} wasm_zone_market_expectation",
    )


def require_float(value: Any, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must be a number")
    return float(value)


def require_position(value: Any, label: str) -> tuple[float, float, float]:
    if not isinstance(value, list) or len(value) != 3:
        raise ValueError(f"{label} must be a list of three numbers")
    return (
        require_float(value[0], f"{label}[0]"),
        require_float(value[1], f"{label}[1]"),
        require_float(value[2], f"{label}[2]"),
    )


def require_box(
    zone: dict[str, Any], label: str
) -> tuple[tuple[float, float, float], tuple[float, float, float]]:
    minimum = require_position(zone.get("minimum"), f"{label} minimum")
    maximum = require_position(zone.get("maximum"), f"{label} maximum")
    if any(low > high for low, high in zip(minimum, maximum)):
        raise ValueError(f"{label} minimum {minimum} is past its maximum {maximum}")
    return minimum, maximum


def point_within_box(
    position: tuple[float, float, float],
    box: tuple[tuple[float, float, float], tuple[float, float, float]],
) -> bool:
    minimum, maximum = box
    return all(low <= value <= high for value, low, high in zip(position, minimum, maximum))


def crossing_axis(
    outside: tuple[float, float, float], inside: tuple[float, float, float]
) -> tuple[int, int]:
    """The one horizontal axis a boundary crossing travels along.

    The fixture declares two fixed points the walk crosses the box between;
    exactly one horizontal component may differ, because a diagonal walk would
    leave the distance to the crossed face ambiguous.
    """

    dx = inside[0] - outside[0]
    dz = inside[2] - outside[2]
    if (dx == 0.0) == (dz == 0.0):
        raise ValueError(
            f"the zone fixture's inside {inside} and outside {outside} must differ on "
            "exactly one horizontal axis"
        )
    return (1 if dx > 0.0 else -1, 0) if dx != 0.0 else (0, 1 if dz > 0.0 else -1)


def crossing_yaw(axis: tuple[int, int]) -> int:
    """The vanilla yaw that faces the crossing direction."""

    return round(math.degrees(math.atan2(-axis[0], axis[1])))


def crossing_travel(
    outside: tuple[float, float, float],
    axis: tuple[int, int],
    position: tuple[float, float, float],
) -> float:
    """How far the position has moved from the outside point along the axis."""

    return (position[0] - outside[0]) * axis[0] + (position[2] - outside[2]) * axis[1]


def face_travel(
    outside: tuple[float, float, float],
    box: tuple[tuple[float, float, float], tuple[float, float, float]],
    axis: tuple[int, int],
) -> float:
    """How far along the axis the box face nearest the outside point stands."""

    minimum, maximum = box
    if axis[0] != 0:
        face = minimum[0] if axis[0] > 0 else maximum[0]
        return (face - outside[0]) * axis[0]
    face = minimum[2] if axis[1] > 0 else maximum[2]
    return (face - outside[2]) * axis[1]


@dataclass
class ClientProbe:
    """One real client plus the observation primitives both P3 fixtures share.

    Chat waits use the client's versioned state notifications and inventory waits
    use the bridge's producer-driven exact-count wait, so no phase sleeps or polls
    on a fixed interval. Every assertion re-reads client-visible state instead of
    trusting the bridge's own reply.
    """

    client: Any
    transcript: list[dict[str, Any]]
    hooks: TradeScenarioHooks
    scenario_id: str
    run_dir: Path
    screenshots_dir: Path
    timeout_seconds: float

    @property
    def answer_wait_seconds(self) -> float:
        """The budget one marker, inventory sync or attack answer may take.

        Chat, inventory-sync and combat answers arrive within a tick or two; a
        bounded budget keeps a missing answer from hanging the scenario for the
        whole client timeout.
        """

        return min(self.timeout_seconds, 30.0)

    def call(
        self,
        command: str,
        payload: dict[str, Any],
        timeout: float | None = None,
    ) -> dict[str, Any]:
        return self.hooks.call(
            self.client,
            self.transcript,
            command,
            payload,
            self.timeout_seconds if timeout is None else timeout,
        )

    def recent_chat(self, observation: dict[str, Any]) -> list[str]:
        lines = observation.get("recent_chat")
        if not isinstance(lines, list) or not all(isinstance(line, str) for line in lines):
            raise RuntimeError("client observation did not contain string recent_chat entries")
        return lines

    @staticmethod
    def line_counts(lines: list[str]) -> dict[str, int]:
        counts: dict[str, int] = {}
        for line in lines:
            counts[line] = counts.get(line, 0) + 1
        return counts

    def fresh_chat_lines(
        self,
        observation: dict[str, Any],
        baseline: dict[str, int],
    ) -> list[str]:
        counts = self.line_counts(self.recent_chat(observation))
        return [
            line for line, count in counts.items()
            for _ in range(count - baseline.get(line, 0))
        ]

    def marker_baseline(self) -> dict[str, int]:
        """The chat already visible, so a later marker must be new."""

        return self.line_counts(self.recent_chat(self.observe_client()))

    def observe_client(self) -> dict[str, Any]:
        observation = self.call("observe", {})
        if not self.hooks.is_in_play(observation):
            raise RuntimeError(f"client left Play state during {self.scenario_id}")
        return observation

    def await_state(self, observation: dict[str, Any], budget: float) -> None:
        state_version = observation.get("state_version")
        if isinstance(state_version, bool) or not isinstance(state_version, int):
            raise RuntimeError("client observation did not contain integer state_version")
        # The deadline-driven callers can pass a budget below the client's
        # accepted 0.1 s floor; clamp instead of turning an ordinary timeout
        # into a rejected RPC argument.
        event_timeout = max(0.1, budget)
        self.call(
            "wait_state_change",
            {"observed_version": state_version, "timeout_seconds": event_timeout},
            event_timeout + 2.0,
        )

    def submit_chat(self, message: str) -> None:
        self.call("send_chat", {"message": message, "command": True})

    def wait_for_fresh_chat(
        self,
        matcher: Callable[[list[str]], bool],
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            if matcher(self.fresh_chat_lines(observation, baseline)):
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} was not observed after the command; fresh chat: "
                    f"{self.fresh_chat_lines(observation, baseline)}"
                )
            self.await_state(observation, remaining)

    def wait_for_fresh_marker(
        self,
        marker: str,
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        """Wait for one fresh chat line that begins with ``marker``.

        A marker is an ordering fence for the request that produced it, never an
        acknowledged effect: every caller reads the effect from client state.
        """

        return self.wait_for_fresh_chat(
            lambda fresh: any(line.startswith(marker) for line in fresh),
            description,
            baseline,
        )

    def fresh_prefixed_lines(
        self,
        observation: dict[str, Any],
        baseline: dict[str, int],
        prefix: str,
    ) -> list[str]:
        return [
            line
            for line in self.fresh_chat_lines(observation, baseline)
            if line.startswith(prefix + " ")
        ]

    def require_fresh_prefix_marker(
        self,
        prefix: str,
        expected: str,
        description: str,
        baseline: dict[str, int],
    ) -> dict[str, Any]:
        """Wait for one fence marker and require it is the only fresh one.

        Every marker here fences the request that produced it instead of
        reporting an effect, so an extra fresh line of the same family is an
        event the crossing did not ask for - a duplicated entry or a spurious
        exit - and fails the phase instead of being ignored. The expected line is
        matched by prefix, so a builder that reports what it verified after the
        part a caller fixes stays one line of that family.
        """

        self.wait_for_fresh_marker(expected, description, baseline)
        settled = self.observe_client()
        fresh = self.fresh_prefixed_lines(settled, baseline, prefix)
        if len(fresh) != 1 or not fresh[0].startswith(expected):
            raise RuntimeError(
                f"{description} expected exactly one fresh {prefix} marker beginning with "
                f"{expected!r}, observed {fresh}"
            )
        return settled

    def player_position(self, observation: dict[str, Any]) -> tuple[float, float, float]:
        player = observation.get("player")
        if not isinstance(player, dict):
            raise RuntimeError("client observation did not contain a player object")
        position = (player.get("x"), player.get("y"), player.get("z"))
        if not all(
            isinstance(value, (int, float)) and not isinstance(value, bool) for value in position
        ):
            raise RuntimeError(f"client observation exposed no numeric player position: {position}")
        return (float(position[0]), float(position[1]), float(position[2]))

    def wait_for_position(
        self,
        expected: tuple[float, float, float],
        tolerance: float,
        description: str,
    ) -> dict[str, Any]:
        """Wait for the client to report the position an operator command asked for."""

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            position = self.player_position(observation)
            if all(abs(value - want) <= tolerance for value, want in zip(position, expected)):
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not reach {expected} within {tolerance}: {position}"
                )
            self.await_state(observation, remaining)

    def await_crossing(
        self,
        *,
        outside: tuple[float, float, float],
        axis: tuple[int, int],
        threshold: float,
        entered: bool,
        description: str,
    ) -> dict[str, Any]:
        """Wait until the client's own position is past the declared face.

        What is waited on is the position the client itself reports, so a crossing
        is a boundary the server's zone owner can see rather than a claim the
        driver makes on the client's behalf.
        """

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            position = self.player_position(observation)
            travel = crossing_travel(outside, axis, position)
            reached = travel >= threshold if entered else travel <= threshold
            if reached:
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not cross the declared face: position {position}, "
                    f"travel {travel:.3f}, threshold {threshold:.3f}"
                )
            self.await_state(observation, remaining)

    def walk_across(
        self,
        *,
        outside: tuple[float, float, float],
        axis: tuple[int, int],
        threshold: float,
        entered: bool,
        facing: tuple[int, int],
        step_ticks: int,
        max_steps: int,
        description: str,
    ) -> tuple[dict[str, Any], int]:
        """Walk the real client across the declared face, one movement step at a time.

        ``axis`` is the measurement axis the threshold is expressed on, while
        ``facing`` is the direction the walk holds forward input in - leaving the
        box walks the opposite way from entering it. Each step holds the client's
        own forward input for real ticks, and the stop condition is re-read from
        the client's own position, so a short step walks again instead of being
        assumed. No step sleeps or polls on a fixed interval.
        """

        self.call("look", {"yaw_deg": crossing_yaw(facing), "pitch_deg": 0})
        observation = self.observe_client()
        position = self.player_position(observation)
        travel = crossing_travel(outside, axis, position)
        for step in range(max_steps + 1):
            reached = travel >= threshold if entered else travel <= threshold
            if reached:
                return observation, step
            if step == max_steps:
                break
            self.call("move_forward", {"ticks": step_ticks})
            observation = self.observe_client()
            position = self.player_position(observation)
            travel = crossing_travel(outside, axis, position)
        raise TimeoutError(
            f"{description} did not cross the declared face in {max_steps} movement steps: "
            f"last position {position}, travel {travel:.3f}, threshold {threshold:.3f}"
        )

    def confirm_position(self, dx_cm: int, dz_cm: int, description: str) -> dict[str, Any]:
        """Send one real client position packet after an operator teleport.

        An operator ``/tp`` commits the server-side pose but does not feed the
        zone observer, which consumes accepted client movement: the position
        packet this action sends is the client reporting where it now stands, and
        it is what turns the teleport into an observed boundary crossing.
        """

        response = self.call("move_by", {"dx_cm": dx_cm, "dz_cm": dz_cm})
        return {
            "dx_cm": dx_cm,
            "dz_cm": dz_cm,
            "bridge_response": response,
            "description": description,
        }

    def require_platform(
        self,
        *,
        position: tuple[float, float, float],
        tolerance: float,
        description: str,
    ) -> dict[str, Any]:
        """Require the teleported client to stand on the prepared platform.

        The operator setup is only real once the client sees it: the landing
        position is read back from the client, and the block the client occupies
        and the block under it are read through the bridge's own block reader at
        the client's own reported position rather than at a declared one.
        """

        landed = self.wait_for_position(position, tolerance, description)
        observed = self.player_position(landed)
        feet = {
            "x": math.floor(observed[0]),
            "y": round(observed[1]),
            "z": math.floor(observed[2]),
        }
        ground = {"x": feet["x"], "y": feet["y"] - 1, "z": feet["z"]}
        blocks: dict[str, Any] = {}
        for name, probe in (("feet", feet), ("ground", ground)):
            self.call(
                "wait_loaded_block",
                {**probe, "timeout_seconds": self.answer_wait_seconds},
                self.answer_wait_seconds + 2.0,
            )
            blocks[name] = self.call("read_block", probe)
        if blocks["ground"].get("is_air") is not False:
            raise RuntimeError(f"{description} found no block under the client: {blocks['ground']}")
        if blocks["feet"].get("is_air") is not True:
            raise RuntimeError(f"{description} left the client inside a block: {blocks['feet']}")
        return {"position": observed, "feet_probe": feet, "ground_probe": ground, "blocks": blocks}

    def inventory_entries(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        entries = observation.get("inventory")
        if not isinstance(entries, list) or not all(isinstance(entry, dict) for entry in entries):
            raise RuntimeError("client observation did not contain an inventory list")
        return entries

    def inventory_fingerprint(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        fingerprint: list[dict[str, Any]] = []
        for entry in self.inventory_entries(observation):
            slot = entry.get("slot")
            if isinstance(slot, bool) or not isinstance(slot, int):
                raise RuntimeError("client inventory entry did not expose an integer slot")
            stack = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
            stack["slot"] = slot
            fingerprint.append(stack)
        fingerprint.sort(key=lambda stack: stack["slot"])
        return fingerprint

    def item_count(self, observation: dict[str, Any], item_id: str) -> int:
        total = 0
        for entry in self.inventory_entries(observation):
            if entry.get("item_id") != item_id:
                continue
            count = entry.get("count")
            if isinstance(count, bool) or not isinstance(count, int):
                raise RuntimeError(f"client inventory entry for {item_id} had a non-integer count")
            total += count
        return total

    def tool_fingerprint(self, observation: dict[str, Any], item_id: str) -> dict[str, Any]:
        matching = [
            entry for entry in self.inventory_entries(observation) if entry.get("item_id") == item_id
        ]
        if len(matching) != 1:
            raise RuntimeError(
                f"expected exactly one client-visible {item_id} stack, observed {len(matching)}"
            )
        entry = matching[0]
        fingerprint = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
        count = fingerprint["count"]
        if isinstance(count, bool) or not isinstance(count, int):
            raise RuntimeError(f"client-visible {item_id} stack had a non-integer count")
        damage = fingerprint["damage"]
        if isinstance(damage, bool) or not isinstance(damage, int):
            raise RuntimeError(
                f"client-visible {item_id} stack exposed no integer damage component"
            )
        max_damage = fingerprint["max_damage"]
        if isinstance(max_damage, bool) or not isinstance(max_damage, int) or max_damage <= 0:
            raise RuntimeError(
                f"client-visible {item_id} stack exposed no positive max_damage component"
            )
        return fingerprint

    def await_inventory_counts(self, *counts: tuple[str, int]) -> None:
        for item_id, count in counts:
            self.call(
                "wait_inventory",
                {
                    "item_id": item_id,
                    "count": count,
                    "timeout_seconds": self.answer_wait_seconds,
                },
                self.answer_wait_seconds + 2.0,
            )

    def require_trade_settlement(
        self,
        *,
        label: str,
        expected_marker: str,
        outcome: str,
        expected_counts: tuple[tuple[str, int], ...],
        baseline: dict[str, int],
        before_inventory: list[dict[str, Any]],
        before_tool: dict[str, Any],
        initial_tool: dict[str, Any],
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Require one submission's marker, counts and untouched tool.

        The marker is the guest's own report of what its read-back agreed with, so
        the committed/refused outcome is only accepted here together with the
        client-visible counts it implies and a byte-identical unaffected tool.
        """

        self.wait_for_fresh_marker(
            expected_marker,
            f"{label} marker {expected_marker!r}",
            baseline,
        )
        self.await_inventory_counts(*expected_counts)
        settled = self.observe_client()
        fresh_markers = [
            line
            for line in self.fresh_chat_lines(settled, baseline)
            if line.startswith(TRADE_MARKER_PREFIX + " ")
        ]
        if fresh_markers != [expected_marker]:
            raise RuntimeError(
                f"{label} expected exactly one fresh trade marker {expected_marker!r}, "
                f"observed {fresh_markers}"
            )
        observed_counts = {
            item_id: self.item_count(settled, item_id) for item_id, _ in expected_counts
        }
        expected = dict(expected_counts)
        if observed_counts != expected:
            raise RuntimeError(
                f"{label} inventory mismatch: expected {expected}, observed {observed_counts}"
            )
        tool_item_id = before_tool.get("item_id")
        if not isinstance(tool_item_id, str) or not tool_item_id:
            raise RuntimeError(f"{label} tool baseline named no item: {before_tool!r}")
        after_tool = self.tool_fingerprint(settled, tool_item_id)
        if after_tool != before_tool or after_tool != initial_tool:
            raise RuntimeError(
                f"{label} changed the unaffected component-bearing tool: {after_tool}"
            )
        inventory_unchanged = self.inventory_fingerprint(settled) == before_inventory
        if outcome == REFUSED_OUTCOME and not inventory_unchanged:
            raise RuntimeError(f"{label} refused but still changed the client-visible inventory")
        if outcome == COMMITTED_OUTCOME and inventory_unchanged:
            raise RuntimeError(f"{label} committed without changing the client-visible inventory")
        return settled, {
            "marker": expected_marker,
            "fresh_markers": fresh_markers,
            "inventory": observed_counts,
            "inventory_unchanged": inventory_unchanged,
            "tool_fingerprint": after_tool,
            "tool_fingerprint_unchanged": after_tool == before_tool,
        }

    def screen(self, observation: dict[str, Any]) -> dict[str, Any]:
        screen = observation.get("screen")
        if not isinstance(screen, dict):
            raise RuntimeError("client observation did not contain a screen object")
        return screen

    def container(self, observation: dict[str, Any]) -> dict[str, Any]:
        container = observation.get("container")
        if not isinstance(container, dict):
            raise RuntimeError("client observation did not contain a container object")
        return container

    def container_slots(self, observation: dict[str, Any]) -> list[dict[str, Any]]:
        slots = self.container(observation).get("slots")
        if not isinstance(slots, list) or not all(isinstance(entry, dict) for entry in slots):
            raise RuntimeError("client observation did not contain a container slot list")
        return slots

    def menu_buttons(
        self,
        observation: dict[str, Any],
        menu_button_slot_count: int,
    ) -> list[dict[str, Any]]:
        """The window's own slots, i.e. the container slots before the appended
        player inventory. They are fixed buttons the server owns, so their
        contents never move while the window is open."""

        buttons: list[dict[str, Any]] = []
        for entry in self.container_slots(observation):
            slot = entry.get("slot")
            if isinstance(slot, bool) or not isinstance(slot, int):
                raise RuntimeError("client container slot entry did not expose an integer slot")
            if slot >= menu_button_slot_count:
                continue
            stack = {field: entry.get(field) for field in FINGERPRINT_FIELDS}
            stack["slot"] = slot
            buttons.append(stack)
        buttons.sort(key=lambda stack: stack["slot"])
        return buttons

    def wait_for_menu(self, title: str, description: str) -> dict[str, Any]:
        """Wait for the client to show a plain container window with this title."""

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            screen = self.screen(observation)
            if screen.get("open") is True and screen.get("title") == title:
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    f"{description} did not open on the real client: screen is {screen!r}"
                )
            self.await_state(observation, remaining)

    def wait_for_closed_screen(self, description: str) -> dict[str, Any]:
        """Wait for the client to hold no server-owned window at all.

        The vanilla client answers a server container close by dropping to its own
        inventory menu, so the container id must be back to zero as well.
        """

        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            screen = self.screen(observation)
            if screen.get("open") is not True:
                container_id = self.container(observation).get("container_id")
                if container_id != 0:
                    raise RuntimeError(
                        f"{description} left container {container_id!r} without a screen"
                    )
                return observation
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"{description} did not close on the real client: {screen!r}")
            self.await_state(observation, remaining)

    def require_menu(
        self,
        observation: dict[str, Any],
        menu: dict[str, Any],
        menu_button_slot_count: int,
        container_slot_count: int,
        label: str,
    ) -> dict[str, Any]:
        """Require the client to show one declared window, button for button."""

        screen = self.screen(observation)
        title = require_str(menu["title"], f"{label} title")
        if screen.get("open") is not True:
            raise RuntimeError(f"{label} has no screen open: {screen!r}")
        if screen.get("class") != MENU_SCREEN_CLASS:
            raise RuntimeError(
                f"{label} opened {screen.get('class')!r} instead of a plain container screen"
            )
        if screen.get("title") != title:
            raise RuntimeError(
                f"{label} window title is {screen.get('title')!r}, expected {title!r}"
            )
        container = self.container(observation)
        container_id = container.get("container_id")
        if isinstance(container_id, bool) or not isinstance(container_id, int) or container_id == 0:
            raise RuntimeError(f"{label} exposed no server-owned container id: {container!r}")
        if container.get("slot_count") != container_slot_count:
            raise RuntimeError(
                f"{label} window declares {container.get('slot_count')!r} slots, expected "
                f"{container_slot_count}: the declared buttons plus the appended inventory"
            )
        declared = menu.get("slots")
        if not isinstance(declared, list) or not declared:
            raise ValueError(f"{label} declares no menu slots")
        buttons = {stack["slot"]: stack for stack in self.menu_buttons(observation, menu_button_slot_count)}
        declared_slots = sorted(require_int(slot["slot"], f"{label} slot index") for slot in declared)
        if sorted(buttons) != declared_slots:
            raise RuntimeError(
                f"{label} shows buttons {sorted(buttons)} on the client, expected {declared_slots}"
            )
        for slot in declared:
            index = require_int(slot["slot"], f"{label} slot index")
            stack = buttons[index]
            for field, key in (("item_id", "item_id"), ("count", "count"), ("name", "label")):
                # The window's label reaches the client as the stack's own name.
                if stack.get(field) != slot.get(key):
                    raise RuntimeError(
                        f"{label} button {index} {field} is {stack.get(field)!r}, expected "
                        f"{slot.get(key)!r}"
                    )
        return {
            "container_id": container_id,
            "screen_class": screen.get("class"),
            "title": screen.get("title"),
            "slot_count": container.get("slot_count"),
            "buttons": [buttons[index] for index in declared_slots],
        }

    def open_menu(
        self,
        command_root: str,
        menu: dict[str, Any],
        description: str,
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Request one menu and wait for its window to appear.

        The returned record states what the request was: the marker the guest
        published is a fence for the request, and the window that follows is the
        effect this method waits for.
        """

        action = require_str(menu["open_action"], f"{description} open action")
        fence = require_str(menu["open_fence"], f"{description} open fence")
        command = f"{command_root} {action}"
        baseline = self.marker_baseline()
        self.submit_chat(command)
        self.wait_for_fresh_marker(fence, f"{description} request marker {fence!r}", baseline)
        observation = self.wait_for_menu(
            require_str(menu["title"], f"{description} title"),
            f"{description} window",
        )
        return observation, {
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the request; the window is the effect",
        }

    def request_close(
        self,
        command_root: str,
        action: str,
        fence: str,
        description: str,
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        """Send one close request and wait for the client to hold no window."""

        command = f"{command_root} {action}"
        baseline = self.marker_baseline()
        self.submit_chat(command)
        self.wait_for_fresh_marker(fence, f"{description} request marker {fence!r}", baseline)
        closed = self.wait_for_closed_screen(f"{description} window close")
        return closed, {
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the request; the closed window is the effect",
        }

    def click_container_slot(
        self,
        slot: int,
        button: str,
        description: str,
    ) -> dict[str, Any]:
        """Every fixture click closes its window and must be confirmed."""

        response = self.call("click_container_slot", {"slot": slot, "button": button})
        if response.get("confirmed") is not True:
            raise RuntimeError(f"{description} was not confirmed: {response!r}")
        return {
            "slot": slot,
            "button": button,
            "confirmed": True,
            "bridge_response": response,
        }

    def capture_screen(self, name: str) -> str:
        return self.hooks.capture_screenshot(
            self.client,
            self.transcript,
            self.run_dir,
            self.screenshots_dir,
            f"{self.scenario_id}-{name}",
            self.timeout_seconds,
        )

    def capture_inventory_screenshot(self, name: str) -> str:
        self.call("open_inventory", {})
        path = self.capture_screen(name)
        self.call("close_screen", {})
        state = self.call("state", {})
        # Closing the vanilla inventory is immediate; a bounded wait keeps a
        # stuck screen from hanging the whole scenario for the client timeout.
        self.hooks.await_interactive_play(
            self.client,
            self.transcript,
            state,
            min(self.timeout_seconds, 15.0),
        )
        return path

    def reconnect(self, server_addr: str, player_username: str) -> dict[str, Any]:
        """Disconnect, wait for the server to release the session, and reconnect."""

        self.call("disconnect", {})
        self.hooks.leave_play(self.client, self.transcript, self.timeout_seconds)
        session_release = self.hooks.await_session_release(self.run_dir, self.timeout_seconds)
        self.call("connect", {"server_addr": server_addr})
        play = self.call("wait_play", {"timeout_seconds": self.timeout_seconds})
        play = self.hooks.await_interactive_play(
            self.client,
            self.transcript,
            play,
            self.timeout_seconds,
        )
        if not self.hooks.is_in_play(play):
            raise RuntimeError("the real client did not reach Play state after reconnect")
        observation = self.observe_client()
        player = observation.get("player")
        if not isinstance(player, dict) or player.get("name") != player_username:
            raise RuntimeError(
                "the rejoined client identity changed: "
                f"{player.get('name') if isinstance(player, dict) else player!r}"
            )
        return {"session_release": session_release, "observation": observation}

    def run_setup_command(self, command: str, setup_commands: list[dict[str, Any]]) -> None:
        self.submit_chat(command)
        setup_commands.append({"command": "/" + command})

    def run_confirmed_setup_command(
        self,
        command: str,
        prefix: str,
        feedback: str,
        setup_commands: list[dict[str, Any]],
    ) -> dict[str, Any]:
        """Run one operator setup command and require its own feedback line.

        The server's debug builders answer with what they verified, so the
        confirmation is the server's own read-back of the blocks it wrote rather
        than an assumption that the command applied. Exactly one fresh line of
        that family is required, so a second report cannot be read as this
        command's.
        """

        setup_commands.append({"command": "/" + command})
        baseline = self.marker_baseline()
        self.submit_chat(command)
        settled = self.require_fresh_prefix_marker(
            prefix,
            feedback,
            f"operator setup {command!r} feedback",
            baseline,
        )
        return {
            "command": "/" + command,
            "feedback": feedback,
            "observed": [
                line
                for line in self.fresh_prefixed_lines(settled, baseline, prefix)
            ],
        }

    def require_walkway(
        self,
        *,
        top_y: int,
        surface_y: int,
        walkway_z: int,
        x_min: int,
        x_max: int,
        description: str,
    ) -> dict[str, Any]:
        """Require the client to see the prepared walkway, block for block.

        The operator setup is only real once the client sees it: the whole
        crossing strip is read through the bridge's own block scan, and every
        column is required to be solid where the walk happens and air where the
        player stands. A missing or refused setup command therefore fails here
        instead of showing up as a fall in the middle of a crossing.
        """

        self.call(
            "wait_loaded_block",
            {"x": x_min, "y": top_y, "z": walkway_z, "timeout_seconds": self.answer_wait_seconds},
            self.answer_wait_seconds + 2.0,
        )
        scan = self.call(
            "scan_blocks",
            {
                "min_x": x_min,
                "min_y": top_y,
                "min_z": walkway_z,
                "max_x": x_max,
                "max_y": surface_y,
                "max_z": walkway_z,
                "max_blocks": 512,
            },
        )
        blocks = scan.get("blocks")
        if not isinstance(blocks, list) or not blocks:
            raise RuntimeError(f"{description} block scan returned no blocks: {scan!r}")
        columns: dict[int, dict[str, Any]] = {}
        for block in blocks:
            if not isinstance(block, dict):
                raise RuntimeError(f"{description} block scan returned a non-object entry")
            x = require_int(block.get("x"), f"{description} block x")
            y = require_int(block.get("y"), f"{description} block y")
            columns.setdefault(x, {})[str(y)] = block
        expected_x = list(range(x_min, x_max + 1))
        if sorted(columns) != expected_x:
            raise RuntimeError(
                f"{description} block scan covered {sorted(columns)}, expected {expected_x}"
            )
        for x in expected_x:
            column = columns[x]
            solid = column.get(str(top_y))
            surface = column.get(str(surface_y))
            if solid is None or surface is None:
                raise RuntimeError(f"{description} column {x} is missing a layer: {column!r}")
            if solid.get("is_air") is not False:
                raise RuntimeError(f"{description} column {x} has no block to walk on: {solid!r}")
            if surface.get("is_air") is not True:
                raise RuntimeError(f"{description} column {x} stands the player inside a block: {surface!r}")
        return {
            "x": expected_x,
            "walk_y": top_y,
            "surface_y": surface_y,
            "z": walkway_z,
            "blocks": [
                {
                    "x": x,
                    "walk_block": columns[x][str(top_y)].get("block_id"),
                    "surface_block": columns[x][str(surface_y)].get("block_id"),
                }
                for x in expected_x
            ],
        }

    def seed_trade_fixture(
        self,
        setup: dict[str, Any],
        emerald: dict[str, Any],
        apple: dict[str, Any],
        tool: dict[str, Any],
    ) -> dict[str, Any]:
        """Seed the fixture with explicit operator commands and one real attack.

        The commands are declared debug setup (``no_debug_commands`` false) and are
        never claimed as no-debug survival. The attack exists so the unaffected
        stack carries a non-default, client-readable component before any trade
        runs.
        """

        emerald_item_id = require_str(emerald["item_id"], "emerald item id")
        apple_item_id = require_str(apple["item_id"], "apple item id")
        tool_item_id = require_str(tool["item_id"], "tool item id")
        damage_entity_type = require_str(setup["damage_entity_type"], "damage entity type")
        setup_commands: list[dict[str, Any]] = []
        self.run_setup_command(
            require_str(setup["gamemode_command"], "gamemode command"), setup_commands
        )
        self.run_setup_command(require_str(emerald["command"], "emerald give command"), setup_commands)
        self.run_setup_command(require_str(tool["command"], "tool give command"), setup_commands)
        self.await_inventory_counts(
            (emerald_item_id, require_int(emerald["count"], "seeded emerald count")),
            (apple_item_id, require_int(apple["initial_count"], "initial apple count")),
        )
        self.call(
            "wait_inventory",
            {"item_id": tool_item_id, "count": 1, "timeout_seconds": self.answer_wait_seconds},
            self.answer_wait_seconds + 2.0,
        )
        seeded = self.observe_client()
        if self.item_count(seeded, tool_item_id) != 1:
            raise RuntimeError(f"seed did not leave exactly one {tool_item_id} in the inventory")

        placement = seeded["player"]
        summon_command = require_str(
            setup["summon_command_template"], "summon command template"
        ).format(
            x=f"{float(placement['x']) + 1.0:.1f}",
            y=f"{float(placement['y']):.1f}",
            z=f"{float(placement['z']):.1f}",
        )
        self.run_setup_command(summon_command, setup_commands)
        self.call(
            "select_hotbar_item",
            {"item_id": tool_item_id, "count": 1, "timeout_seconds": self.answer_wait_seconds},
        )
        deadline = time.monotonic() + self.answer_wait_seconds
        while True:
            observation = self.observe_client()
            visible = self.call("list_entities", {"radius": 24.0, "limit": 64})
            entities = visible.get("entities")
            if not isinstance(entities, list):
                raise RuntimeError("list_entities did not return an entity list")
            targets = [
                entity for entity in entities
                if entity["entity_type"] == damage_entity_type
            ]
            if targets:
                target = min(targets, key=lambda entity: entity["distance"])
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(f"no client-visible {damage_entity_type} arrived after summon")
            self.await_state(observation, remaining)
        attack = self.call(
            "attack_entity_once",
            {
                "entity_id": require_int(target.get("entity_id"), "damage entity id"),
                "entity_uuid": require_str(target.get("entity_uuid"), "damage entity uuid"),
                "entity_type": damage_entity_type,
                "timeout_seconds": self.answer_wait_seconds,
            },
            self.answer_wait_seconds + 2.0,
        )
        if attack.get("confirmed") is not True:
            raise RuntimeError(f"the real survival attack was not confirmed by the server: {attack}")
        if attack.get("removed") is True:
            # A killed entity can drop a collectible item, which would perturb the
            # unchanged-inventory assertions this fixture depends on.
            raise RuntimeError(
                "the durability attack removed the summoned entity instead of only "
                f"damaging it: {attack}"
            )

        deadline = time.monotonic() + self.answer_wait_seconds
        damaged: dict[str, Any] | None = None
        while damaged is None:
            observation = self.observe_client()
            seeded_tool = self.tool_fingerprint(observation, tool_item_id)
            if seeded_tool["damage"] > 0:
                damaged = observation
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0.0:
                raise TimeoutError(
                    "the survival attack left no durability component on "
                    f"{tool_item_id}: {seeded_tool}"
                )
            self.await_state(observation, remaining)
        initial_tool = self.tool_fingerprint(damaged, tool_item_id)
        screenshot = self.capture_inventory_screenshot(
            require_str(setup["inventory_screenshot"], "seed screenshot name")
        )
        return {
            "commands": setup_commands,
            "screenshot": screenshot,
            "inventory": {
                emerald_item_id: self.item_count(damaged, emerald_item_id),
                apple_item_id: self.item_count(damaged, apple_item_id),
            },
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": {
                "entity_type": damage_entity_type,
                "entity_id": target.get("entity_id"),
                "confirmed": attack.get("confirmed"),
                "removed": attack.get("removed"),
                "health_before": attack.get("health_before"),
                "health_after": attack.get("health_after"),
            },
        }


def run_wasm_inventory_storage_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    expectation = load_inventory_storage_expectation(run_dir, scenario_id)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    package_id = require_str(expectation["package_id"], "package id")
    mode = require_str(expectation["mode"], "fixture mode")
    player_username = require_str(expectation["player_username"], "player username")
    command_root = require_str(expectation["command_root"], "command root")
    ledger_key = require_str(expectation["ledger_key"], "ledger key")
    marker_prefix = require_str(expectation["marker_prefix"], "marker prefix")
    if marker_prefix != TRADE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} trade marker prefix must be {TRADE_MARKER_PREFIX}")
    setup = require_mapping(expectation["setup"], "setup")
    sequence = expectation["sequence"]
    if not isinstance(sequence, list) or not sequence:
        raise ValueError(f"{scenario_id} sequence must be a non-empty list")
    rejoin = require_mapping(expectation["rejoin"], "rejoin phase")
    rejoin_expected_emerald = require_int(rejoin["emerald"], "rejoin emerald count")
    rejoin_expected_apple = require_int(rejoin["apple"], "rejoin apple count")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")
    tool_item_id = require_str(tool["item_id"], "tool item id")

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    # The phase logic below keeps the probe's own names for the primitives it
    # reads, so it stays a description of the trade flow.
    call = probe.call
    recent_chat = probe.recent_chat
    line_counts = probe.line_counts
    observe_client = probe.observe_client
    submit_chat = probe.submit_chat
    inventory_fingerprint = probe.inventory_fingerprint
    item_count = probe.item_count
    tool_fingerprint = probe.tool_fingerprint
    await_inventory_counts = probe.await_inventory_counts
    capture_inventory_screenshot = probe.capture_inventory_screenshot

    phases: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def run_trade_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} action")
        args = require_str(phase["args"], f"{label} command arguments")
        outcome = require_str(phase["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(
                f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}"
            )
        ledger = require_int(phase["ledger"], f"{label} ledger value")
        expected_emerald = require_int(phase["emerald"], f"{label} emerald count")
        expected_apple = require_int(phase["apple"], f"{label} apple count")
        before = observe_client()
        before_inventory = inventory_fingerprint(before)
        before_tool = tool_fingerprint(before, tool_item_id)
        baseline = line_counts(recent_chat(before))
        command = f"{command_root} {args}"
        submit_chat(command)
        settled, evidence = probe.require_trade_settlement(
            label=label,
            expected_marker=f"{marker_prefix} {action} {outcome} ledger={ledger}",
            outcome=outcome,
            expected_counts=(
                (emerald_item_id, expected_emerald),
                (apple_item_id, expected_apple),
            ),
            baseline=baseline,
            before_inventory=before_inventory,
            before_tool=before_tool,
            initial_tool=initial_tool,
        )
        record = {
            "label": label,
            "action": action,
            "command": "/" + command,
            "outcome": outcome,
            "ledger": ledger,
            **evidence,
        }
        phases.append(record)
        capture_name = phase.get("capture")
        if capture_name is not None:
            screenshots.append(
                capture_inventory_screenshot(require_str(capture_name, f"{label} capture name"))
            )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            adversarial_checks.append(
                {
                    "check": require_str(adversarial, f"{label} adversarial check"),
                    "command": "/" + command,
                    "expected": f"{outcome} ledger={ledger}",
                    "observed_marker": evidence["fresh_markers"][0],
                    "inventory": evidence["inventory"],
                    "inventory_unchanged": evidence["inventory_unchanged"],
                    "tool_fingerprint_unchanged": evidence["tool_fingerprint_unchanged"],
                }
            )
        return settled

    call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    joined = observe_client()
    player = joined.get("player")
    if not isinstance(player, dict) or player.get("name") != player_username:
        raise RuntimeError(
            f"unexpected client identity: {player.get('name') if isinstance(player, dict) else player!r}"
        )

    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    setup_commands = seeded["commands"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    for index, phase in enumerate(sequence):
        run_trade_phase(require_mapping(phase, f"sequence phase {index}"), f"sequence[{index}]")

    pre_rejoin = observe_client()
    pre_rejoin_emerald = item_count(pre_rejoin, emerald_item_id)
    pre_rejoin_apple = item_count(pre_rejoin, apple_item_id)
    rejoin_info = probe.reconnect(server_addr, player_username)
    session_release = rejoin_info["session_release"]
    await_inventory_counts(
        (emerald_item_id, rejoin_expected_emerald),
        (apple_item_id, rejoin_expected_apple),
    )
    rejoined = observe_client()
    observed_rejoin_emerald = item_count(rejoined, emerald_item_id)
    observed_rejoin_apple = item_count(rejoined, apple_item_id)
    if (observed_rejoin_emerald, observed_rejoin_apple) != (pre_rejoin_emerald, pre_rejoin_apple):
        raise RuntimeError(
            "inventory did not survive the disconnect/rejoin: "
            f"before={pre_rejoin_emerald}/{pre_rejoin_apple} "
            f"after={observed_rejoin_emerald}/{observed_rejoin_apple}"
        )
    rejoin_tool = tool_fingerprint(rejoined, tool_item_id)
    if rejoin_tool != initial_tool:
        raise RuntimeError(
            f"the component-bearing tool changed across the disconnect/rejoin: {rejoin_tool}"
        )
    screenshots.append(capture_inventory_screenshot("rejoin-inventory"))

    run_trade_phase(rejoin, "rejoin")
    run_trade_phase(
        require_mapping(expectation["live_session_probe"], "live session probe"),
        "live-session-after-refusal",
    )

    final_state = call("state", {})
    call("disconnect", {})

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "package_id": package_id,
        "mode": mode,
        "player": player_username,
        "command_root": "/" + command_root,
        "ledger_key": ledger_key,
        "setup": {
            "commands": setup_commands,
            "initial_inventory": seeded["inventory"],
            "tool_item_id": seeded["tool_item_id"],
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "phases": phases,
        "rejoin": {
            "session_release": session_release,
            "inventory": {
                emerald_item_id: observed_rejoin_emerald,
                apple_item_id: observed_rejoin_apple,
            },
            "tool_fingerprint": rejoin_tool,
        },
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_OBSERVATION_GAPS),
    }
    return "passed", final_state, screenshots, scenario_report


def run_wasm_inventory_menu_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    """Drive the server-owned market window through the real client's menu surface.

    Every trade phase opens the window it needs and clicks the window's own slot
    through a real container click; the guest's marker only fences that request,
    so the committed/refused outcome is accepted together with the client-visible
    counts it implies. The stale probes keep the live next-menu open while the
    guest is asked to close a foreign menu and to act on the session the reconnect
    left behind, and a current-session purchase on that same kept window then
    proves it is still live.
    """

    expectation = load_inventory_menu_expectation(run_dir, scenario_id)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    package_id = require_str(expectation["package_id"], "package id")
    mode = require_str(expectation["mode"], "fixture mode")
    player_username = require_str(expectation["player_username"], "player username")
    command_root = require_str(expectation["command_root"], "command root")
    ledger_key = require_str(expectation["ledger_key"], "ledger key")
    marker_prefix = require_str(expectation["marker_prefix"], "marker prefix")
    if marker_prefix != TRADE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} trade marker prefix must be {TRADE_MARKER_PREFIX}")
    menu_marker_prefix = require_str(expectation["menu_marker_prefix"], "menu marker prefix")
    if menu_marker_prefix != MENU_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} menu marker prefix must be {MENU_MARKER_PREFIX}")
    menu_button_slot_count = require_int(
        expectation["menu_button_slot_count"], "menu button slot count"
    )
    container_slot_count = require_int(
        expectation["menu_container_slot_count"], "menu container slot count"
    )
    setup = require_mapping(expectation["setup"], "setup")
    market = require_mapping(expectation["market"], "market window")
    next_market = require_mapping(expectation["next_market"], "next market window")
    trades = expectation["trades"]
    if not isinstance(trades, list) or not trades:
        raise ValueError(f"{scenario_id} trades must be a non-empty list")
    close_slot = require_mapping(expectation["close_slot"], "close slot phase")
    close_request = require_mapping(expectation["close_request"], "close request phase")
    rejoin = require_mapping(expectation["rejoin"], "rejoin phase")
    stale_close = require_mapping(expectation["stale_close"], "stale close phase")
    stale_session = require_mapping(expectation["stale_session"], "stale session phase")
    live_purchase = require_mapping(expectation["live_purchase"], "live purchase phase")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    phases: list[dict[str, Any]] = []
    menu_phases: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def record_adversarial(name: Any, label: str, detail: dict[str, Any]) -> None:
        adversarial_checks.append(
            {"check": require_str(name, f"{label} adversarial check"), **detail}
        )

    def inventory_counts(observation: dict[str, Any]) -> dict[str, int]:
        return {
            emerald_item_id: probe.item_count(observation, emerald_item_id),
            apple_item_id: probe.item_count(observation, apple_item_id),
        }

    def run_menu_trade(
        phase: dict[str, Any],
        label: str,
        menu: dict[str, Any],
        *,
        already_open: bool,
    ) -> dict[str, Any]:
        slot = require_int(phase["slot"], f"{label} slot")
        button = require_str(phase["button"], f"{label} click button")
        outcome = require_str(phase["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}")
        ledger = require_int(phase["ledger"], f"{label} ledger value")
        expected_marker = require_str(phase["marker"], f"{label} marker")
        if f"ledger={ledger}" not in expected_marker:
            raise ValueError(f"{label} marker {expected_marker!r} does not carry ledger={ledger}")
        expected_counts = (
            (emerald_item_id, require_int(phase["emerald"], f"{label} emerald count")),
            (apple_item_id, require_int(phase["apple"], f"{label} apple count")),
        )
        before = probe.observe_client()
        before_inventory = probe.inventory_fingerprint(before)
        before_tool = probe.tool_fingerprint(before, tool_item_id)
        if already_open:
            opened = before
            request: dict[str, Any] = {
                "command": None,
                "window": menu.get("id"),
                "request_only": "the previous phase opened this window and kept it open",
            }
        else:
            opened, request = probe.open_menu(command_root, menu, label)
        menu_fingerprint = probe.require_menu(
            opened,
            menu,
            menu_button_slot_count,
            container_slot_count,
            f"{label} window",
        )
        menu_capture = phase.get("menu_capture")
        if menu_capture is not None:
            screenshots.append(
                probe.capture_screen(require_str(menu_capture, f"{label} menu capture"))
            )
        baseline = probe.marker_baseline()
        click = probe.click_container_slot(slot, button, f"{label} slot {slot} {button} click")
        settled, evidence = probe.require_trade_settlement(
            label=label,
            expected_marker=expected_marker,
            outcome=outcome,
            expected_counts=expected_counts,
            baseline=baseline,
            before_inventory=before_inventory,
            before_tool=before_tool,
            initial_tool=initial_tool,
        )
        # Every trade click closes its accepted window before the transaction, so
        # the client must hold no server-owned window once the marker is out.
        probe.wait_for_closed_screen(f"{label} accepted window close")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "request": request,
            "click": click,
            "outcome": outcome,
            "ledger": ledger,
            "window_closed": True,
            **evidence,
        }
        phases.append(record)
        capture_name = phase.get("capture")
        if capture_name is not None:
            screenshots.append(
                probe.capture_inventory_screenshot(
                    require_str(capture_name, f"{label} capture name")
                )
            )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": None,
                    "click": {"slot": slot, "button": button},
                    "expected": expected_marker,
                    "observed_marker": evidence["fresh_markers"][0],
                    "inventory": evidence["inventory"],
                    "inventory_unchanged": evidence["inventory_unchanged"],
                    "tool_fingerprint_unchanged": evidence["tool_fingerprint_unchanged"],
                    "window_closed": True,
                },
            )
        return record

    def run_close_slot_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        slot = require_int(phase["slot"], f"{label} slot")
        button = require_str(phase["button"], f"{label} click button")
        fence = require_str(phase["fence"], f"{label} close fence")
        opened, request = probe.open_menu(command_root, market, label)
        menu_fingerprint = probe.require_menu(
            opened, market, menu_button_slot_count, container_slot_count, f"{label} window"
        )
        screenshots.append(
            probe.capture_screen(require_str(phase["menu_capture"], f"{label} menu capture"))
        )
        before_inventory = probe.inventory_fingerprint(probe.observe_client())
        baseline = probe.marker_baseline()
        click = probe.click_container_slot(slot, button, f"{label} slot {slot} {button} click")
        probe.wait_for_fresh_marker(fence, f"{label} request marker {fence!r}", baseline)
        closed = probe.wait_for_closed_screen(f"{label} window close")
        counts = inventory_counts(closed)
        if probe.inventory_fingerprint(closed) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory while closing")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "request": request,
            "click": click,
            "request_marker": fence,
            "request_only": "the marker fences the close request; the closed window is the effect",
            "window_closed": True,
            "inventory": counts,
            "inventory_unchanged": True,
        }
        phases.append(record)
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(phase["capture"], f"{label} capture name")
            )
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": None,
                    "click": {"slot": slot, "button": button},
                    "expected": "the window closes without touching the inventory",
                    "request_marker": fence,
                    "window_closed": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    def run_close_request_phase(phase: dict[str, Any], label: str) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} close action")
        fence = require_str(phase["fence"], f"{label} close fence")
        opened, open_request = probe.open_menu(command_root, market, label)
        menu_fingerprint = probe.require_menu(
            opened, market, menu_button_slot_count, container_slot_count, f"{label} window"
        )
        screenshots.append(
            probe.capture_screen(require_str(phase["menu_capture"], f"{label} menu capture"))
        )
        before_inventory = probe.inventory_fingerprint(probe.observe_client())
        closed, request = probe.request_close(command_root, action, fence, label)
        counts = inventory_counts(closed)
        if probe.inventory_fingerprint(closed) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory while closing")
        record = {
            "label": label,
            "menu": menu_fingerprint,
            "open_request": open_request,
            "request": request,
            "window_closed": True,
            "inventory": counts,
            "inventory_unchanged": True,
        }
        phases.append(record)
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(phase["capture"], f"{label} capture name")
            )
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": request["command"],
                    "expected": "the window closes without touching the inventory",
                    "request_marker": fence,
                    "window_closed": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    def run_stale_phase(
        phase: dict[str, Any],
        label: str,
        menu: dict[str, Any],
    ) -> dict[str, Any]:
        action = require_str(phase["action"], f"{label} action")
        fence = require_str(phase["fence"], f"{label} request fence")
        before = probe.observe_client()
        before_menu = probe.require_menu(
            before, menu, menu_button_slot_count, container_slot_count, f"{label} live window"
        )
        before_inventory = probe.inventory_fingerprint(before)
        baseline = probe.marker_baseline()
        command = f"{command_root} {action}"
        probe.submit_chat(command)
        probe.wait_for_fresh_marker(fence, f"{label} request marker {fence!r}", baseline)
        after = probe.observe_client()
        after_menu = probe.require_menu(
            after,
            menu,
            menu_button_slot_count,
            container_slot_count,
            f"{label} live window after the stale request",
        )
        if after_menu != before_menu:
            raise RuntimeError(
                f"{label} replaced the live window: {before_menu} -> {after_menu}"
            )
        counts = inventory_counts(after)
        if probe.inventory_fingerprint(after) != before_inventory:
            raise RuntimeError(f"{label} changed the client-visible inventory")
        record = {
            "label": label,
            "action": action,
            "command": "/" + command,
            "request_marker": fence,
            "request_only": "the marker fences the guest's request; the live window is the effect",
            "window": after_menu,
            "window_unchanged": True,
            "inventory": counts,
            "inventory_unchanged": True,
            "scope": (
                "observed continuity of the live client state; wire-level refusal of a stale "
                "menu or session belongs to the native component tests"
            ),
        }
        menu_phases.append(record)
        screenshots.append(
            probe.capture_screen(require_str(phase["capture"], f"{label} capture name"))
        )
        adversarial = phase.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "command": "/" + command,
                    "expected": "the live window and inventory stay exactly as they were",
                    "request_marker": fence,
                    "window": after_menu,
                    "window_unchanged": True,
                    "inventory": counts,
                    "inventory_unchanged": True,
                },
            )
        return record

    probe.call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    joined = probe.observe_client()
    player = joined.get("player")
    if not isinstance(player, dict) or player.get("name") != player_username:
        raise RuntimeError(
            f"unexpected client identity: {player.get('name') if isinstance(player, dict) else player!r}"
        )

    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    tool_item_id = seeded["tool_item_id"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    for index, phase in enumerate(trades):
        entry = require_mapping(phase, f"trade phase {index}")
        run_menu_trade(
            entry,
            require_str(entry.get("label", f"trade[{index}]"), f"trade phase {index} label"),
            market,
            already_open=False,
        )

    run_close_slot_phase(close_slot, "close-slot")
    run_close_request_phase(close_request, "close-request")

    rejoin_expected_emerald = require_int(rejoin["emerald"], "rejoin emerald count")
    rejoin_expected_apple = require_int(rejoin["apple"], "rejoin apple count")
    before_rejoin = probe.observe_client()
    pre_rejoin_emerald = probe.item_count(before_rejoin, emerald_item_id)
    pre_rejoin_apple = probe.item_count(before_rejoin, apple_item_id)
    rejoin_info = probe.reconnect(server_addr, player_username)
    probe.await_inventory_counts(
        (emerald_item_id, rejoin_expected_emerald),
        (apple_item_id, rejoin_expected_apple),
    )
    rejoined = probe.observe_client()
    observed_rejoin_emerald = probe.item_count(rejoined, emerald_item_id)
    observed_rejoin_apple = probe.item_count(rejoined, apple_item_id)
    if (observed_rejoin_emerald, observed_rejoin_apple) != (pre_rejoin_emerald, pre_rejoin_apple):
        raise RuntimeError(
            "inventory did not survive the disconnect/rejoin: "
            f"before={pre_rejoin_emerald}/{pre_rejoin_apple} "
            f"after={observed_rejoin_emerald}/{observed_rejoin_apple}"
        )
    rejoin_tool = probe.tool_fingerprint(rejoined, tool_item_id)
    if rejoin_tool != initial_tool:
        raise RuntimeError(
            f"the component-bearing tool changed across the disconnect/rejoin: {rejoin_tool}"
        )
    screenshots.append(
        probe.capture_inventory_screenshot(require_str(rejoin["capture"], "rejoin capture name"))
    )
    rejoin_record = {
        "session_release": rejoin_info["session_release"],
        "inventory": {
            emerald_item_id: observed_rejoin_emerald,
            apple_item_id: observed_rejoin_apple,
        },
        "tool_fingerprint": rejoin_tool,
        "screenshot": screenshots[-1],
    }

    opened, next_open_request = probe.open_menu(command_root, next_market, "next-market")
    next_fingerprint = probe.require_menu(
        opened, next_market, menu_button_slot_count, container_slot_count, "next-market window"
    )
    screenshots.append(probe.capture_screen("next-menu-open"))
    menu_phases.append(
        {
            "label": "next-market",
            "request": next_open_request,
            "window": next_fingerprint,
            "window_kept_open": True,
        }
    )

    run_stale_phase(stale_close, "stale-menu-close", next_market)
    run_stale_phase(stale_session, "stale-session-menu", next_market)

    live_record = run_menu_trade(
        live_purchase,
        "live-session-purchase",
        next_market,
        already_open=True,
    )

    final_state = probe.call("state", {})
    probe.call("disconnect", {})

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "package_id": package_id,
        "mode": mode,
        "player": player_username,
        "command_root": "/" + command_root,
        "ledger_key": ledger_key,
        "trade_marker_prefix": marker_prefix,
        "menu_marker_prefix": menu_marker_prefix,
        "setup": {
            "commands": seeded["commands"],
            "initial_inventory": seeded["inventory"],
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "phases": phases,
        "menu_phases": menu_phases,
        "rejoin": rejoin_record,
        "live_session_purchase": live_record,
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_MENU_OBSERVATION_GAPS),
        "sibling_scenario": (
            "wasm-p3-inventory-storage stays the command-only /trade scenario with its own "
            "manifest; this mode reuses the same fixture without changing it"
        ),
    }
    return "passed", final_state, screenshots, scenario_report


def run_wasm_zone_market_scenario(
    client: Any,
    run_dir: Path,
    scenario_id: str,
    server_addr: str,
    timeout_seconds: float,
    transcript: list[dict[str, Any]],
    hooks: TradeScenarioHooks,
) -> tuple[str, dict[str, Any], list[str], dict[str, Any]]:
    """Drive the market through real zone boundary crossings.

    The fixture creates one fixed box, and every phase crosses its face for real:
    ordinary forward input walks in and out, and the one crossing that leaves with
    the window still open is an operator ``/tp`` plus the client's own position
    report, because a container screen swallows ordinary movement input. Each
    crossing is asserted three ways - the client's own position past the declared
    face, the fixture's own fence marker, and the window that opens or closes as
    its effect - so a marker is never accepted as an effect and the exit-driven
    close is proven by the same window the entry opened.
    """

    expectation = load_zone_market_expectation(run_dir, scenario_id)
    screenshots_dir = run_dir / "screenshots"
    screenshots_dir.mkdir(parents=True, exist_ok=True)

    package_id = require_str(expectation["package_id"], "package id")
    mode = require_str(expectation["mode"], "fixture mode")
    if mode != "zone-market":
        raise ValueError(f"{scenario_id} must run the zone-market fixture mode, not {mode!r}")
    player_username = require_str(expectation["player_username"], "player username")
    command_root = require_str(expectation["command_root"], "command root")
    ledger_key = require_str(expectation["ledger_key"], "ledger key")
    marker_prefix = require_str(expectation["marker_prefix"], "marker prefix")
    if marker_prefix != TRADE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} trade marker prefix must be {TRADE_MARKER_PREFIX}")
    zone_marker_prefix = require_str(expectation["zone_marker_prefix"], "zone marker prefix")
    if zone_marker_prefix != ZONE_MARKER_PREFIX:
        raise ValueError(f"{scenario_id} zone marker prefix must be {ZONE_MARKER_PREFIX}")
    menu_button_slot_count = require_int(
        expectation["menu_button_slot_count"], "menu button slot count"
    )
    container_slot_count = require_int(
        expectation["menu_container_slot_count"], "menu container slot count"
    )
    zone = require_mapping(expectation["zone"], "zone definition")
    zone_id = require_str(zone["id"], "zone id")
    dimension = require_str(zone["dimension"], "zone dimension")
    box = require_box(zone, f"zone {zone_id}")
    setup_action = require_str(zone["setup_action"], f"{zone_id} setup action")
    ready_marker = require_str(zone["ready_marker"], f"{zone_id} ready marker")
    entered_marker = require_str(zone["entered_marker"], f"{zone_id} entered marker")
    exited_marker = require_str(zone["exited_marker"], f"{zone_id} exited marker")
    for label, marker in (
        ("ready", ready_marker),
        ("entered", entered_marker),
        ("exited", exited_marker),
    ):
        if not marker.startswith(ZONE_MARKER_PREFIX + " "):
            raise ValueError(
                f"{zone_id} {label} marker {marker!r} does not begin with {ZONE_MARKER_PREFIX}"
            )
    positions = require_mapping(expectation["positions"], "zone positions")
    outside = require_position(positions["outside"], "outside position")
    inside = require_position(positions["inside"], "inside position")
    teleport_command = require_str(positions["teleport_command"], "outside teleport command")
    landing_tolerance = require_float(positions["landing_tolerance"], "landing tolerance")
    axis = crossing_axis(outside, inside)
    terrain = require_mapping(expectation["terrain"], "terrain setup")
    terrain_commands = terrain.get("commands")
    if not isinstance(terrain_commands, list) or not terrain_commands:
        raise ValueError(f"{scenario_id} terrain commands must be a non-empty list")
    walkway = require_mapping(terrain.get("walkway"), "terrain walkway")
    walkway = {
        "walk_y": require_int(walkway["walk_y"], "walkway walk y"),
        "surface_y": require_int(walkway["surface_y"], "walkway surface y"),
        "z": require_int(walkway["z"], "walkway z"),
        "x_min": require_int(walkway["x_min"], "walkway x min"),
        "x_max": require_int(walkway["x_max"], "walkway x max"),
    }
    if walkway["surface_y"] != walkway["walk_y"] + 1:
        raise ValueError(f"{scenario_id} walkway surface must sit directly on its walk layer")
    if walkway["x_min"] > walkway["x_max"]:
        raise ValueError(f"{scenario_id} walkway x range is empty")
    if walkway["z"] != math.floor(outside[2]) or walkway["z"] != math.floor(inside[2]):
        raise ValueError(
            f"{scenario_id} walkway z {walkway['z']} is not the block row the declared outside "
            f"{outside} and inside {inside} stand on"
        )
    if not (walkway["x_min"] <= math.floor(outside[0]) and walkway["x_max"] >= math.floor(inside[0])):
        raise ValueError(
            f"{scenario_id} walkway x {walkway['x_min']}..{walkway['x_max']} does not span the "
            f"declared crossing from {outside} to {inside}"
        )
    crossing = require_mapping(expectation["crossing"], "crossing")
    step_ticks = require_int(crossing["step_ticks"], "crossing step ticks")
    max_steps = require_int(crossing["max_steps"], "crossing max steps")
    margin = require_float(crossing["margin"], "crossing margin")
    confirm_offset_cm = require_int(crossing["confirm_offset_cm"], "crossing confirm offset")
    setup = require_mapping(expectation["setup"], "setup")
    market = require_mapping(expectation["market"], "market window")
    cycles = expectation["cycles"]
    if not isinstance(cycles, list) or not cycles:
        raise ValueError(f"{scenario_id} cycles must be a non-empty list")
    reconnect = require_mapping(expectation["reconnect"], "reconnect phase")

    emerald = require_mapping(setup["emerald"], "setup emerald")
    apple = require_mapping(setup["apple"], "setup apple")
    tool = require_mapping(setup["tool"], "setup tool")
    emerald_item_id = require_str(emerald["item_id"], "emerald item id")
    apple_item_id = require_str(apple["item_id"], "apple item id")
    market_title = require_str(market["title"], "market title")

    # The face the walk crosses and the direction a leaving walk faces: the
    # declared outside point is the one fixed reference the thresholds use.
    # Entry opens a container immediately and consumes further movement input.
    # Crossing the real face is sufficient; extra interior travel is not required.
    entry_threshold = face_travel(outside, box, axis)
    exit_threshold = face_travel(outside, box, axis) - margin
    exit_axis = (-axis[0], -axis[1])

    probe = ClientProbe(
        client=client,
        transcript=transcript,
        hooks=hooks,
        scenario_id=scenario_id,
        run_dir=run_dir,
        screenshots_dir=screenshots_dir,
        timeout_seconds=timeout_seconds,
    )
    setup_commands: list[dict[str, Any]] = []
    cycles_records: list[dict[str, Any]] = []
    adversarial_checks: list[dict[str, Any]] = []
    screenshots: list[str] = []

    def record_adversarial(name: Any, label: str, detail: dict[str, Any]) -> None:
        adversarial_checks.append(
            {"check": require_str(name, f"{label} adversarial check"), **detail}
        )

    def window_record(observation: dict[str, Any], description: str) -> dict[str, Any]:
        return probe.require_menu(
            observation,
            market,
            menu_button_slot_count,
            container_slot_count,
            description,
        )

    def enter_zone(label: str) -> dict[str, Any]:
        """Cross the face by walking and require the window the entry opened."""

        baseline = probe.marker_baseline()
        crossed, steps = probe.walk_across(
            outside=outside,
            axis=axis,
            threshold=entry_threshold,
            entered=True,
            facing=axis,
            step_ticks=step_ticks,
            max_steps=max_steps,
            description=f"{label} entry walk",
        )
        position = probe.player_position(crossed)
        if not point_within_box(position, box):
            raise RuntimeError(
                f"{label} walked to {position}, which the declared box {box} does not contain"
            )
        probe.require_fresh_prefix_marker(
            zone_marker_prefix,
            entered_marker,
            f"{label} entry fence",
            baseline,
        )
        opened = probe.wait_for_menu(market_title, f"{label} entry window")
        window = window_record(opened, f"{label} entry window")
        return {
            "position": position,
            "movement_steps": steps,
            "marker": entered_marker,
            "window": window,
        }

    def exit_zone(label: str, route: str, window_open: bool) -> dict[str, Any]:
        """Cross the face leaving and require the window this fixture holds closed."""

        if route not in {WALK_EXIT_ROUTE, TELEPORT_EXIT_ROUTE}:
            raise ValueError(
                f"{label} exit route must be {WALK_EXIT_ROUTE} or {TELEPORT_EXIT_ROUTE}"
            )
        before = probe.observe_client()
        held = window_record(before, f"{label} window before the exit") if window_open else None
        baseline = probe.marker_baseline()
        confirmation: dict[str, Any] | None = None
        steps: int | None = None
        if route == TELEPORT_EXIT_ROUTE:
            # A container screen swallows ordinary movement input, so this one
            # crossing is the operator teleport plus the client's own report of
            # where it now stands. The close below is the fixture's exit-driven
            # close, not a screen the driver dismissed.
            probe.submit_chat(teleport_command)
            crossed = probe.await_crossing(
                outside=outside,
                axis=axis,
                threshold=exit_threshold,
                entered=False,
                description=f"{label} teleport landing",
            )
            confirmation = probe.confirm_position(
                -axis[0] * confirm_offset_cm,
                -axis[1] * confirm_offset_cm,
                f"{label} client position report after the teleport",
            )
        else:
            crossed, steps = probe.walk_across(
                outside=outside,
                axis=axis,
                threshold=exit_threshold,
                entered=False,
                facing=exit_axis,
                step_ticks=step_ticks,
                max_steps=max_steps,
                description=f"{label} exit walk",
            )
        position = probe.player_position(crossed)
        if point_within_box(position, box):
            raise RuntimeError(
                f"{label} left the client at {position}, which the declared box {box} still contains"
            )
        probe.require_fresh_prefix_marker(
            zone_marker_prefix,
            exited_marker,
            f"{label} exit fence",
            baseline,
        )
        closed = probe.wait_for_closed_screen(f"{label} exit window close")
        settled = probe.player_position(closed)
        if point_within_box(settled, box):
            raise RuntimeError(f"{label} settled back inside the declared box: {settled}")
        return {
            "route": route,
            "movement_steps": steps,
            "teleport_command": "/" + teleport_command if route == TELEPORT_EXIT_ROUTE else None,
            "position_confirmation": confirmation,
            "position": position,
            "settled_position": settled,
            "marker": exited_marker,
            "window_before_exit": held,
            "window_closed": True,
        }

    def run_zone_trade(cycle: dict[str, Any], label: str) -> dict[str, Any]:
        """One click on the window the zone entry opened, through the shared fixture."""

        slot = require_int(cycle["slot"], f"{label} slot")
        button = require_str(cycle["button"], f"{label} click button")
        outcome = require_str(cycle["outcome"], f"{label} outcome")
        if outcome not in {COMMITTED_OUTCOME, REFUSED_OUTCOME}:
            raise ValueError(f"{label} outcome must be {COMMITTED_OUTCOME} or {REFUSED_OUTCOME}")
        ledger = require_int(cycle["ledger"], f"{label} ledger value")
        expected_marker = require_str(cycle["marker"], f"{label} marker")
        if f"ledger={ledger}" not in expected_marker:
            raise ValueError(f"{label} marker {expected_marker!r} does not carry ledger={ledger}")
        expected_counts = (
            (emerald_item_id, require_int(cycle["emerald"], f"{label} emerald count")),
            (apple_item_id, require_int(cycle["apple"], f"{label} apple count")),
        )
        before = probe.observe_client()
        before_inventory = probe.inventory_fingerprint(before)
        before_tool = probe.tool_fingerprint(before, tool_item_id)
        baseline = probe.marker_baseline()
        click = probe.click_container_slot(slot, button, f"{label} slot {slot} {button} click")
        settled, evidence = probe.require_trade_settlement(
            label=label,
            expected_marker=expected_marker,
            outcome=outcome,
            expected_counts=expected_counts,
            baseline=baseline,
            before_inventory=before_inventory,
            before_tool=before_tool,
            initial_tool=initial_tool,
        )
        probe.wait_for_closed_screen(f"{label} accepted window close")
        record = {
            "click": click,
            "outcome": outcome,
            "ledger": ledger,
            "window_closed": True,
            **evidence,
        }
        capture_name = cycle.get("inventory_capture")
        if capture_name is not None:
            screenshots.append(
                probe.capture_inventory_screenshot(
                    require_str(capture_name, f"{label} inventory capture")
                )
            )
        adversarial = cycle.get("adversarial")
        if adversarial is not None:
            record_adversarial(
                adversarial,
                label,
                {
                    "click": {"slot": slot, "button": button},
                    "expected": expected_marker,
                    "observed_marker": evidence["fresh_markers"][0],
                    "inventory": evidence["inventory"],
                    "inventory_unchanged": evidence["inventory_unchanged"],
                    "tool_fingerprint_unchanged": evidence["tool_fingerprint_unchanged"],
                    "window_closed": True,
                },
            )
        return record

    def run_movement_probe(cycle: dict[str, Any], label: str, *, entered: bool) -> dict[str, Any]:
        """Move without crossing the face and require nothing happened.

        Movement that stays on one side of the boundary is not a crossing: it must
        not publish a transition marker, must not open a window, and must not
        leave the side it started on.
        """

        ticks = require_int(cycle["ticks"], f"{label} probe ticks")
        steps = require_int(cycle["steps"], f"{label} probe steps")
        direction = axis if entered else exit_axis
        baseline = probe.marker_baseline()
        before = probe.observe_client()
        before_position = probe.player_position(before)
        if point_within_box(before_position, box) is not entered:
            raise RuntimeError(
                f"{label} probe began on the wrong side of the declared box: {before_position}"
            )
        probe.call("look", {"yaw_deg": crossing_yaw(direction), "pitch_deg": 0})
        for _ in range(steps):
            probe.call("move_forward", {"ticks": ticks})
        after = probe.observe_client()
        after_position = probe.player_position(after)
        fresh = probe.fresh_prefixed_lines(after, baseline, zone_marker_prefix)
        if fresh:
            raise RuntimeError(f"{label} probe published a transition it did not cross: {fresh}")
        if probe.screen(after).get("open") is True:
            raise RuntimeError(f"{label} probe opened a window without a crossing")
        if point_within_box(after_position, box) is not entered:
            raise RuntimeError(
                f"{label} probe left its own side of the declared box: {after_position}"
            )
        return {
            "ticks": ticks,
            "steps": steps,
            "from": before_position,
            "to": after_position,
            "side": "inside" if entered else "outside",
            "fresh_markers": fresh,
            "window_open": False,
        }

    def run_reconnect(label: str) -> dict[str, Any]:
        """Reconnect the same player and require the fixture survived it outside the box."""

        expected_emerald = require_int(reconnect["emerald"], "reconnect emerald count")
        expected_apple = require_int(reconnect["apple"], "reconnect apple count")
        before = probe.observe_client()
        pre_reconnect = {
            emerald_item_id: probe.item_count(before, emerald_item_id),
            apple_item_id: probe.item_count(before, apple_item_id),
        }
        info = probe.reconnect(server_addr, player_username)
        probe.await_inventory_counts(
            (emerald_item_id, expected_emerald),
            (apple_item_id, expected_apple),
        )
        rejoined = probe.observe_client()
        observed = {
            emerald_item_id: probe.item_count(rejoined, emerald_item_id),
            apple_item_id: probe.item_count(rejoined, apple_item_id),
        }
        if observed != pre_reconnect:
            raise RuntimeError(
                "inventory did not survive the disconnect/rejoin: "
                f"before={pre_reconnect} after={observed}"
            )
        expected = {emerald_item_id: expected_emerald, apple_item_id: expected_apple}
        if observed != expected:
            raise RuntimeError(f"rejoined inventory {observed} does not match {expected}")
        rejoined_tool = probe.tool_fingerprint(rejoined, tool_item_id)
        if rejoined_tool != initial_tool:
            raise RuntimeError(
                f"the component-bearing tool changed across the disconnect/rejoin: {rejoined_tool}"
            )
        position = probe.player_position(rejoined)
        if point_within_box(position, box):
            raise RuntimeError(
                f"the rejoined client stood inside the declared box at {position}: the next entry "
                "would not be a crossing"
            )
        screenshots.append(
            probe.capture_inventory_screenshot(
                require_str(reconnect["capture"], "reconnect capture name")
            )
        )
        return {
            "session_release": info["session_release"],
            "inventory": observed,
            "tool_fingerprint": rejoined_tool,
            "position": position,
            "window_open": False,
        }

    probe.call("ping", {})
    play = hooks.connect_or_confirm_play(client, transcript, server_addr, timeout_seconds)
    if not hooks.is_in_play(play):
        raise RuntimeError("the real client did not enter Play")
    joined = probe.observe_client()
    player = joined.get("player")
    if not isinstance(player, dict) or player.get("name") != player_username:
        raise RuntimeError(
            f"unexpected client identity: {player.get('name') if isinstance(player, dict) else player!r}"
        )

    # The trade fixture seeds itself at the world spawn, before the walkway
    # exists, so the summoned damage target never stands in the crossing path.
    seeded = probe.seed_trade_fixture(setup, emerald, apple, tool)
    tool_item_id = seeded["tool_item_id"]
    initial_tool = seeded["tool_fingerprint"]
    screenshots.append(seeded["screenshot"])

    # The walkway and the fixed outside position are declared operator setup
    # (no_debug_commands false). Each builder command is accepted only on its own
    # verified-block feedback, the whole strip is then read back through the
    # client's own block scan, and the landing is accepted only once the client
    # reports it with solid ground under it.
    terrain_records: list[dict[str, Any]] = []
    for index, raw in enumerate(terrain_commands):
        entry = require_mapping(raw, f"terrain command {index}")
        terrain_records.append(
            probe.run_confirmed_setup_command(
                require_str(entry["command"], f"terrain command {index} command"),
                require_str(entry["prefix"], f"terrain command {index} prefix"),
                require_str(entry["feedback"], f"terrain command {index} feedback"),
                setup_commands,
            )
        )
    walkway = probe.require_walkway(
        top_y=walkway["walk_y"],
        surface_y=walkway["surface_y"],
        walkway_z=walkway["z"],
        x_min=walkway["x_min"],
        x_max=walkway["x_max"],
        description=f"{zone_id} walkway",
    )
    probe.run_setup_command(teleport_command, setup_commands)
    landing = probe.require_platform(
        position=outside,
        tolerance=landing_tolerance,
        description="outside landing",
    )

    # The zone is the fixture's own box, created by the fixture's own command: the
    # ready marker is its report that the owner answered the upsert Applied, and
    # the crossing below is only driven once that answer exists.
    zone_baseline = probe.marker_baseline()
    zone_command = f"{command_root} {setup_action}"
    probe.submit_chat(zone_command)
    ready = probe.require_fresh_prefix_marker(
        zone_marker_prefix,
        ready_marker,
        f"{zone_id} upsert applied",
        zone_baseline,
    )
    if probe.screen(ready).get("open") is True:
        raise RuntimeError(f"{zone_id} setup opened a window while the client was outside")
    if probe.container(ready).get("container_id") != 0:
        raise RuntimeError(f"{zone_id} setup left a container open: {probe.container(ready)}")
    outside_position = probe.player_position(ready)
    if point_within_box(outside_position, box):
        raise RuntimeError(f"{zone_id} setup began with the client inside the box: {outside_position}")
    screenshots.append(probe.capture_screen("zone-ready"))
    zone_record = {
        "id": zone_id,
        "dimension": dimension,
        "minimum": list(box[0]),
        "maximum": list(box[1]),
        "command": "/" + zone_command,
        "ready_marker": ready_marker,
        "ready_precondition": (
            "the guest publishes the ready marker only from the owner's Applied answer to the "
            "fixture's own upsert, so a refused or missing answer fails this wait instead of "
            "being reported as a created zone"
        ),
        "position": outside_position,
        "window_open": False,
    }
    record_adversarial(
        "zone_setup_applied_outside",
        "zone-setup",
        {
            "command": "/" + zone_command,
            "expected": ready_marker,
            "position": outside_position,
            "window_open": False,
        },
    )

    for index, raw in enumerate(cycles):
        cycle = require_mapping(raw, f"cycle {index}")
        label = require_str(cycle.get("label", f"cycle[{index}]"), f"cycle {index} label")
        kind = require_str(cycle["kind"], f"{label} kind")
        route = require_str(cycle["exit_route"], f"{label} exit route")
        record: dict[str, Any] = {"label": label, "kind": kind, "exit_route": route}
        if cycle.get("reconnect_before") is True:
            record["reconnect"] = run_reconnect(label)
        record["entry"] = enter_zone(label)
        screenshots.append(
            probe.capture_screen(require_str(cycle["window_capture"], f"{label} window capture"))
        )
        if kind == TRADE_CYCLE_KIND:
            record["trade"] = run_zone_trade(cycle, label)
        elif kind != EXIT_CYCLE_KIND:
            raise ValueError(f"{label} kind must be {TRADE_CYCLE_KIND} or {EXIT_CYCLE_KIND}")
        if cycle.get("interior_probe") is not None:
            if kind == EXIT_CYCLE_KIND:
                raise ValueError(
                    f"{label} cannot move inside while the window this crossing must close is open"
                )
            probe_config = require_mapping(cycle["interior_probe"], f"{label} interior probe")
            record["interior_probe"] = run_movement_probe(probe_config, label, entered=True)
            adversarial = probe_config.get("adversarial")
            if adversarial is not None:
                record_adversarial(
                    adversarial,
                    label,
                    {
                        "movement": "ordinary forward input that stays inside the box",
                        **record["interior_probe"],
                    },
                )
        record["exit"] = exit_zone(label, route, window_open=kind == EXIT_CYCLE_KIND)
        if cycle.get("exterior_probe") is not None:
            probe_config = require_mapping(cycle["exterior_probe"], f"{label} exterior probe")
            record["exterior_probe"] = run_movement_probe(probe_config, label, entered=False)
            adversarial = probe_config.get("adversarial")
            if adversarial is not None:
                record_adversarial(
                    adversarial,
                    label,
                    {
                        "movement": "ordinary forward input that stays outside the box",
                        **record["exterior_probe"],
                    },
                )
        screenshots.append(
            probe.capture_screen(require_str(cycle["closed_capture"], f"{label} closed capture"))
        )
        if kind == EXIT_CYCLE_KIND and cycle.get("adversarial") is not None:
            # A trade cycle's check belongs to its own click, which run_zone_trade
            # recorded with the settlement it observed; this one belongs to the
            # crossing that has to close the live window.
            record_adversarial(
                cycle["adversarial"],
                label,
                {
                    "kind": kind,
                    "exit_route": route,
                    **record["exit"],
                },
            )
        cycles_records.append(record)

    final_state = probe.call("state", {})
    probe.call("disconnect", {})

    scenario_report = {
        "result": "passed",
        "id": scenario_id,
        "package_id": package_id,
        "mode": mode,
        "player": player_username,
        "command_root": "/" + command_root,
        "ledger_key": ledger_key,
        "trade_marker_prefix": marker_prefix,
        "zone_marker_prefix": zone_marker_prefix,
        "zone": zone_record,
        "crossing": {
            "outside": list(outside),
            "inside": list(inside),
            "axis": list(axis),
            "entry_threshold": entry_threshold,
            "exit_threshold": exit_threshold,
            "walk_route": (
                "ordinary forward input held for real ticks through the bridge, stopped by the "
                "client's own reported position"
            ),
            "teleport_route": (
                "operator " + teleport_command + " sent while the window was open, followed by one "
                "real client position report, because a container screen swallows ordinary "
                "movement input; the close that follows is the fixture's exit-driven close"
            ),
        },
        "operator_setup": {
            "commands": setup_commands,
            "terrain": terrain_records,
            "walkway": walkway,
            "landing": landing,
        },
        "fixture_seed": {
            "commands": seeded["commands"],
            "initial_inventory": seeded["inventory"],
            "tool_item_id": tool_item_id,
            "tool_fingerprint": initial_tool,
            "durability_attack": seeded["durability_attack"],
        },
        "cycles": cycles_records,
        "adversarial_checks": adversarial_checks,
        "screenshots": screenshots,
        "mcp_observation_gaps": list(MCP_ZONE_OBSERVATION_GAPS),
        "sibling_scenarios": (
            "wasm-p3-inventory-storage and wasm-p3-inventory-menu keep their own manifests, "
            "fixture modes and routes; this mode reuses the same fixture, market and ledger "
            "machine without changing either"
        ),
    }
    return "passed", final_state, screenshots, scenario_report
