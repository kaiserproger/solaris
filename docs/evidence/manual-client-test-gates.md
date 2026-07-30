# Manual and graphical client gate classification

Scope: Phase 1 inventory of workspace tests whose result requires a real
Minecraft Java Edition 26.1.2 client or an owner observation.

Solaris distinguishes a real-client prerequisite from a genuinely manual
assertion. Approved client automation may close deterministic functional gates.
It cannot prove subjective feel, and no unit, raw-TCP, or prepared-only result
can close either class.

## Fail-closed real-client catalogs

The three checked manifests contain 108 scenario declarations:

| Manifest | Scenarios | Current declaration | Passing evidence |
| --- | ---: | --- | --- |
| `playable/real-client-playable-loop.json` | 87 | `manual-pending` | `tools/run-playable-client-gate.sh --run` producing an `agent-run real-client` result |
| `real-client-regression/manifests/m94-regression-pack.json` | 20 | `manual-pending` | `tools/run-real-client-regression.sh --run` producing an `agent-run real-client` result |
| `real-client-regression/manifests/core-replay-seed-81.json` | 1 | `manual-pending` | `tools/run-core-replay-client-gate.sh --run` producing an `agent-run real-client` result |

The manifest status is a fail-closed template marker, not a claim that every
scenario is currently unimplemented or unrun. Each run must record
`owner-run`, `agent-run`, `prepared only`, or `not run` for the exact tree.
`--check` and `--prepare` validate or stage a route but do not satisfy it.
Changing all manifest declarations to `passed` would erase provenance and is
therefore not a valid cleanup.

The M94 catalog also names three rows with no complete route:

| Row | Owner | Reason and exact close condition |
| --- | --- | --- |
| P2 login compression | `mc-protocol` and `mc-net` login | No real-client compression run is mapped. Close with a dedicated 26.1.2 connection scenario that negotiates compression and records the decoded/client-visible result. |
| P3 authentication and administration | `mc-server` access policy and `mc-net` login | Online/offline authentication, duplicate names, whitelist, banlist, operator persistence, and public-safety review are not one current automated route. Close with explicit policy cases on one tree and owner review of the accepted public-server policy. |
| B6 falling blocks | `mc-entity` falling-block authority and `mc-net` publication | Start-only evidence does not cover landing, placement/removal, and drops. Close with a dedicated real-client route plus the owning vanilla oracle. |

## Public-alpha manual and graphical gates

| Gate | Automation level | Owner, reason, and exact close condition |
| --- | --- | --- |
| 24,000-tick day/night, `/time set`, and restart | Agent-run graphical gate passed 2026-07-30; no exact manifest scenario exists | `mc-protocol` clock payload and `mc-net` world-clock publication. Closed on the candidate tree with a real 26.1.2 client: 766 advancing ticks in the first interval, matching 24,003-tick `game_time` and overworld-clock deltas, rendered day/sunset/night/dawn/day evidence, `/time set day`, `/time set night`, and rendered day-one time after restart. Structured observations, screenshots, client/server logs, limits, and artifact paths are in [`world-clock-26.1.2.md`](world-clock-26.1.2.md#graphical-client-gate). |
| Seed `712816` fresh-world visual/playable and restart gate | Partly automatable; terrain feel remains owner-observed | `mc-worldgen` plus the playable route. The deterministic [`2048x2048`-block height, biome, and vegetation mosaics](worldgen-mosaics.md) are checked in. Close the remaining gate on a clean candidate world with the exact seed/config, ordinary client traversal, restart/rejoin, and an owner-recorded disposition of visual/playability findings. |
| Twenty-minute no-operator survival with natural friendly and hostile spawning | Agent-run real-client route exists; release-candidate execution remains required | Playable route. Close on the exact candidate tree with the manifest-backed 20-minute run, no operator or deterministic fixture, natural progression and populations, restart/reconnect, structured observations, screenshots, and client/server logs. A prior-tree pass is regression evidence, not release evidence. |
| Server-only and client-required plugin matrix | Graphical and automatable through the real-client and Loader adapters | `mc-script`, server adapters, and the client Loader. Close on the candidate tree with a server-only plugin accepting an unmodified client, a client-required fixture clearly rejecting an unmodified client, and every claimed supported Loader completing the exact bundle acknowledgement and Play transition. |
| Subjective movement, combat, water, terrain, and overall-session feel | Genuinely owner-observed | Playable route and repository owner. Close only with an owner-played candidate-tree session recording client version, config, duration, restart/rejoin, concrete observations, and disposition. Deterministic client automation remains the functional fence but cannot replace this judgment. |

A prior environment failed graphical startup because it exposed no `DISPLAY`,
`WAYLAND_DISPLAY`, or `XDG_RUNTIME_DIR`; the NeoForge client reported
`glfwInit failed`. That remains a host prerequisite failure, not a Solaris
result. The 2026-07-30 candidate-tree clock checkpoint instead had a working
`DISPLAY=:1` and completed the agent-run graphical clock gate. It was not an
owner-played subjective gate.

## Disposition

Every manual or graphical gate above has an owner, reason, and exact close
condition. None is silently skipped or counted as green:

- manifest preparation remains `prepared only`;
- an automated functional gate requires an exact `agent-run real-client`
  artifact;
- graphical gates require a graphical 26.1.2 client on the tested tree;
- subjective feel requires an owner observation;
- release gates must be reproduced on the exact release-candidate tree.

`benchmark: not applicable`: this checkpoint classifies test evidence and
changes no runtime or measured performance path.

## Reproduction and inspection

```sh
tools/run-playable-client-gate.sh --check
tools/run-real-client-regression.sh --check
tools/run-core-replay-client-gate.sh --check

tools/run-playable-client-gate.sh --run
tools/run-real-client-regression.sh --run
tools/run-core-replay-client-gate.sh --run
```

The `--run` commands require a graphical host. Run only the scenario relevant
to the current checkpoint and preserve its ignored `.analysis` artifact.
