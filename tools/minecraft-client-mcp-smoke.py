#!/usr/bin/env python3
"""Protocol and real-client smoke for the embedded Solaris Minecraft MCP server."""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import urllib.error
import urllib.request
from typing import Any


PROTOCOL_VERSION = "2025-11-25"
REQUIRED_TOOLS = {
    "minecraft_observe",
    "minecraft_read_block",
    "minecraft_wait_for_loaded_block",
    "minecraft_scan_blocks",
    "minecraft_list_entities",
    "minecraft_read_recipe_book",
    "minecraft_wait_for_visible_entity",
    "minecraft_wait_for_health_below",
    "minecraft_wait_for_inventory",
    "minecraft_wait_for_visible_item",
    "minecraft_wait_for_no_visible_item",
    "minecraft_connect",
    "minecraft_wait_for_play",
    "minecraft_select_hotbar_item",
    "minecraft_approach_entity",
    "minecraft_attack_entity_once",
    "minecraft_attack_entity_until_drop_collected",
    "minecraft_press_inputs",
    "minecraft_open_inventory",
    "minecraft_quick_move_container_slot",
    "minecraft_click_container_slot",
    "minecraft_click_container_button",
    "minecraft_drop_selected_item",
    "minecraft_run_scenario",
    "minecraft_disconnect",
}


class McpClient:
    def __init__(self, endpoint: str, token: str, request_timeout_seconds: float = 15.0) -> None:
        self.endpoint = endpoint
        self.token = token
        self.request_timeout_seconds = request_timeout_seconds
        self.session_id: str | None = None
        self.protocol_version = PROTOCOL_VERSION
        self.request_id = 0

    def initialize(self) -> dict[str, Any]:
        response, headers = self._post(
            {
                "jsonrpc": "2.0",
                "id": self._next_id(),
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "solaris-minecraft-mcp-smoke", "version": "0.1.0"},
                },
            },
            include_session=False,
        )
        self._raise_jsonrpc_error(response)
        session_id = headers.get("Mcp-Session-Id")
        if not session_id:
            raise RuntimeError("initialize response omitted Mcp-Session-Id")
        self.session_id = session_id
        result = response["result"]
        self.protocol_version = result["protocolVersion"]
        self._post(
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {},
            },
            expect_json=False,
        )
        return result

    def list_tools(self) -> list[dict[str, Any]]:
        return self._request("tools/list", {})["tools"]

    def call_tool(self, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        result = self._request(
            "tools/call",
            {"name": name, "arguments": arguments or {}},
        )
        if result.get("isError"):
            payload = result.get("structuredContent", result)
            raise RuntimeError(f"{name} failed: {json.dumps(payload, ensure_ascii=False)}")
        structured = result.get("structuredContent")
        if not isinstance(structured, dict):
            raise RuntimeError(f"{name} response omitted structuredContent")
        return structured

    def close(self) -> None:
        if self.session_id is None:
            return
        request = urllib.request.Request(self.endpoint, method="DELETE", headers=self._headers())
        try:
            with urllib.request.urlopen(request, timeout=self.request_timeout_seconds):
                pass
        finally:
            self.session_id = None

    def _request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        response, _ = self._post(
            {
                "jsonrpc": "2.0",
                "id": self._next_id(),
                "method": method,
                "params": params,
            }
        )
        self._raise_jsonrpc_error(response)
        result = response.get("result")
        if not isinstance(result, dict):
            raise RuntimeError(f"{method} returned a non-object result")
        return result

    def _post(
        self,
        payload: dict[str, Any],
        *,
        include_session: bool = True,
        expect_json: bool = True,
    ) -> tuple[dict[str, Any], Any]:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        headers = self._headers() if include_session else self._headers(with_session=False)
        request = urllib.request.Request(self.endpoint, data=body, method="POST", headers=headers)
        with urllib.request.urlopen(request, timeout=self.request_timeout_seconds) as response:
            response_body = response.read()
            if not expect_json:
                if response_body:
                    raise RuntimeError("notification response unexpectedly had a body")
                return {}, response.headers
            decoded = json.loads(response_body)
            if not isinstance(decoded, dict):
                raise RuntimeError("MCP response was not a JSON object")
            return decoded, response.headers

    def _headers(self, *, with_session: bool = True) -> dict[str, str]:
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if with_session and self.session_id is not None:
            headers["Mcp-Session-Id"] = self.session_id
            headers["MCP-Protocol-Version"] = self.protocol_version
        return headers

    def _next_id(self) -> int:
        self.request_id += 1
        return self.request_id

    @staticmethod
    def _raise_jsonrpc_error(response: dict[str, Any]) -> None:
        error = response.get("error")
        if error is not None:
            raise RuntimeError(f"MCP JSON-RPC error: {json.dumps(error, ensure_ascii=False)}")


def run(args: argparse.Namespace) -> dict[str, Any]:
    token = os.environ.get("SOLARIS_CLIENT_MCP_TOKEN", "")
    if not token:
        raise RuntimeError("SOLARIS_CLIENT_MCP_TOKEN is required")
    client = McpClient(args.endpoint, token, max(15.0, args.timeout_seconds + 5.0))
    initialized = client.initialize()
    connection_started = False
    succeeded = False
    try:
        tools = client.list_tools()
        names = {tool["name"] for tool in tools}
        missing = sorted(REQUIRED_TOOLS - names)
        if missing:
            raise RuntimeError(f"MCP tool catalog is missing: {', '.join(missing)}")

        before = client.call_tool("minecraft_observe")
        result: dict[str, Any] = {
            "endpoint": args.endpoint,
            "protocol_version": initialized["protocolVersion"],
            "server": initialized["serverInfo"],
            "tool_count": len(tools),
            "in_play_before_connect": bool(before.get("in_play")),
        }
        if args.server_address:
            client.call_tool("minecraft_connect", {"server_addr": args.server_address})
            connection_started = True
            play = client.call_tool(
                "minecraft_wait_for_play",
                {"timeout_seconds": min(args.timeout_seconds, 120.0)},
            )
            if not play.get("in_play"):
                raise RuntimeError(f"client did not reach play: {json.dumps(play, ensure_ascii=False)}")
            observed = client.call_tool("minecraft_observe")
            player = observed["player"]
            block = client.call_tool(
                "minecraft_wait_for_loaded_block",
                {
                    "x": math.floor(player["x"]),
                    "y": math.floor(player["y"]) - 1,
                    "z": math.floor(player["z"]),
                    "timeout_seconds": min(args.timeout_seconds, 120.0),
                },
            )
            scan = client.call_tool(
                "minecraft_scan_blocks",
                {
                    "min_x": math.floor(player["x"]) - 1,
                    "min_y": math.floor(player["y"]) - 1,
                    "min_z": math.floor(player["z"]) - 1,
                    "max_x": math.floor(player["x"]) + 1,
                    "max_y": math.floor(player["y"]),
                    "max_z": math.floor(player["z"]) + 1,
                    "max_blocks": 18,
                },
            )
            entities = client.call_tool(
                "minecraft_list_entities",
                {"radius": 32.0, "limit": 128},
            )
            recipe_book = client.call_tool(
                "minecraft_read_recipe_book",
                {"limit": 8192},
            )
            scenario_report: dict[str, Any] | None = None
            if args.scenario_id:
                scenario_arguments = {"id": args.scenario_id}
                if args.scenario_artifacts_dir:
                    scenario_arguments["artifacts_dir"] = args.scenario_artifacts_dir
                scenario_report = client.call_tool(
                    "minecraft_run_scenario",
                    scenario_arguments,
                )
                if scenario_report.get("result") != "passed":
                    raise RuntimeError(
                        "Minecraft scenario failed: "
                        + json.dumps(scenario_report, ensure_ascii=False)
                    )
            if args.exercise_input:
                client.call_tool(
                    "minecraft_press_inputs",
                    {"keys": ["forward"], "ticks": 2},
                )
            result.update(
                {
                    "in_play_after_connect": True,
                    "dimension": observed["dimension"],
                    "block_below_player": block["block_id"],
                    "scanned_block_count": scan["count"],
                    "visible_entity_count": entities["visible_count"],
                    "recipe_book": recipe_book,
                    "input_exercised": args.exercise_input,
                    "scenario": scenario_report,
                }
            )
            if args.disconnect:
                client.call_tool("minecraft_disconnect")
                connection_started = False
        succeeded = True
        return result
    finally:
        if connection_started and not succeeded:
            try:
                client.call_tool("minecraft_disconnect")
            except (KeyError, RuntimeError, TimeoutError, urllib.error.URLError):
                pass
        client.close()


def parse_args() -> argparse.Namespace:
    default_port = os.environ.get("SOLARIS_CLIENT_MCP_PORT", "39095")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--endpoint",
        default=f"http://127.0.0.1:{default_port}/mcp",
        help="Streamable HTTP MCP endpoint",
    )
    parser.add_argument("--server-address", help="Optional host:port to connect the real client")
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    parser.add_argument("--disconnect", action="store_true")
    parser.add_argument("--exercise-input", action="store_true")
    parser.add_argument("--scenario-id", help="Optional deterministic in-client scenario id")
    parser.add_argument(
        "--scenario-artifacts-dir",
        "--scenario-screenshots-dir",
        dest="scenario_artifacts_dir",
        help="Optional scenario artifact directory",
    )
    return parser.parse_args()


def main() -> int:
    try:
        print(json.dumps(run(parse_args()), indent=2, sort_keys=True))
        return 0
    except (KeyError, RuntimeError, TimeoutError, urllib.error.HTTPError) as error:
        print(f"minecraft MCP smoke failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
