# ADR 0007 - Connection liveness under outbound pressure

**Date:** 2026-07-22
**Status:** Accepted

## Problem

A real 26.1.2 client connected to a world with 5,132 injected cows kept sending
valid movement packets but did not process Solaris's keepalive echo before the
30-second deadline. Solaris replaced pending challenges every period on the
first failing build, then still closed the active client after challenge
replacement was fixed. Movement publication also used every simulation tick,
while vanilla's default entity tracking interval is three ticks.

## Decision

Each connection has at most one outstanding keepalive challenge. A new period
does not replace it. The challenge is cleared only by its matching echo.

Timeout requires both conditions:

1. the pending challenge exceeded the keepalive deadline;
2. no valid inbound packet was received during that deadline.

Any decoded inbound packet proves that the connection is alive, but it does
not clear or replace the challenge. A dead or fully stalled client still
closes after the bounded inactivity deadline.

Ordinary entity movement uses vanilla's default three-tick tracking interval.
When more than 512 entity states compete for one tracking turn, deterministic
rotating shards publish at most roughly 512 ordinary states per turn. The
latest unsent server state remains authoritative and is compared on the next
eligible turn. Arrows, item entities, and experience orbs bypass the shard so
combat and pickup feedback stays responsive.

## Consequences

- Dense outbound work cannot disconnect a client that is still sending valid
  traffic merely because its keepalive echo is delayed.
- Normal-size worlds retain vanilla's three-tick movement cadence.
- Extreme crowds trade visual update frequency for bounded packet work; no
  authoritative simulation state is dropped.
- Per-entity vanilla update intervals remain future parity work. The current
  latency-sensitive exceptions cover the gameplay-critical fast paths.

## Evidence

The O3 fixture contained 5,132 injected cows and 5,227 total server entities.
A real 26.1.2 MCP client completed 720 movement ticks plus 255 wait ticks and
remained in play. The server recorded no keepalive mismatch, keepalive timeout,
reliable command drop, or retry. Ignored local evidence is retained in
`.analysis/codex-logs/dense-5132-spawn.json`,
`.analysis/codex-logs/dense-5132-release-build-v5.log`,
`.analysis/codex-logs/dense-5132-keepalive-fixed-v5.json`, and
`.analysis/codex-logs/dense-5132-fixed-v5-server.log`.
