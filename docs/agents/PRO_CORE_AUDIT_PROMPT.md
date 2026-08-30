# Solaris whole-core audit -> fully decomposed agent handoff

You are the planning/audit model for Solaris, a Rust Minecraft Java Edition 26.1.2-compatible server. This run is **analysis and decomposition only**. Do not implement production changes, do not commit, and do not collapse the audit into a short generic roadmap.

## Mission

Walk the entire current core and produce the next high-confidence engineering program for autonomous agents. Find concrete improvement opportunities, latent correctness defects, incomplete vanilla parity, architectural debt, measurable performance risks, missing observability/operator surfaces, test/QA blind spots, and release-engineering gaps. The goal is not to invent speculative rewrites; it is to turn evidence from the current repository into a finite, prioritized, dependency-aware backlog that other agents can execute without having to rediscover the system.

Treat current `AGENTS.md`, `docs/PUBLIC_ALPHA3_PLAN.md`, architecture/decision docs, current git state, existing evidence, tests and runtime tooling as source of truth. Preserve already-closed gates unless you find an actual regression or evidence contradiction.

## Required audit dimensions

Audit all of these separately, then cross-link dependencies:

1. **Protocol / vanilla parity** — handshake/login/configuration/play codecs, packet ordering, movement/collision, inventory/container semantics, blocks/items/entities, client-visible metadata, F3/server brand, ordinary-client behavior and exact 26.1.2 parity boundaries.
2. **World / chunks / persistence** — residency, chunk streaming/generation/light, cache pressure/eviction, save/restart, journals, corruption/fail-stop behavior, world contract and imports.
3. **Entity / simulation / physics / AI** — ownership, scheduling, natural spawning, pathing, collisions, combat, passive/hostile behavior, cross-region mutation, concurrency/order races.
4. **World generation** — terrain/coast/river/drainage, biome-domain coherence, multi-seed statistical quality, structures/features/vegetation, ore profiles, deterministic/chunk-order-independent contracts.
5. **Performance / memory / concurrency** — hot paths, locks, queues, batching, allocation/copies, serialization/compression, startup, tick latency, chunk throughput, cache sizing, autoscaling/admission, profiling/benchmark gaps. Require measurement before optimization.
6. **Plugin / Luau / Loader platform** — API completeness, capability/security model, server-only vs client-required deployment, lifecycle/reload, storage/events/commands/UI, bundled/standard plugin pack, permission/economy/audit/claims interoperability.
7. **Security / trust / operator authority** — auth modes, permissions/operators, custom payloads, plugin sandbox boundaries, untrusted client input, persistence integrity, resource exhaustion and safe defaults.
8. **Operator UX / observability** — config discoverability, normal op/deop workflow, console, lightweight dashboard, telemetry/metrics/logging, diagnostics, backups/recovery, F3/server identity.
9. **Testing / QA / fuzz / benchmarks** — unit/property/integration/raw-TCP/real-client coverage, ignored/manual tests, deterministic waits, Xvfb graphical client gates, adversarial exploratory QA, fuzz targets, coverage, performance regression gates.
10. **Build / portability / release engineering** — Linux/aarch64 portability, toolchain, Loader builds, CI, packaging, release/tag discipline, docs/version drift, dependency/supply-chain hygiene.
11. **Documentation / maintainability** — stale docs, ownership boundaries, giant modules/functions, dead/superseded APIs, confusing abstractions, missing examples and runbooks.

## Evidence standard

For every finding:

- cite concrete file paths and symbols (line numbers when available);
- state the observed fact, not just an opinion;
- distinguish **confirmed defect**, **parity gap**, **measured risk**, **architecture debt**, **missing product/operator surface**, and **research question**;
- record existing tests/evidence that already protect the area;
- do not reopen completed work because it merely looks old;
- if evidence is insufficient, make the first task an explicit measurement/reproduction checkpoint.

## Severity and priority

Use:

- **P0** — corrupts state, disconnects/crashes ordinary clients, breaks common survival, creates security/integrity risk, or blocks alpha release.
- **P1** — important parity/product/operator/performance work with clear user or maintainability value.
- **P2** — useful breadth/polish/optimization after measurement, not release-blocking.
- **P3 / parking lot** — speculative or low-value; keep out of execution waves unless a prerequisite changes.

Also label confidence: `confirmed`, `strong`, `tentative`.

## Decomposition contract

Every executable backlog item must be a **finite agent checkpoint**, preferably one observable vertical slice. For every task include:

- stable task ID, short title, P-level, confidence;
- problem/evidence with owning files/symbols;
- exact scope and explicit non-goals;
- dependencies / blockers / supersedes relationships;
- expected write set (files/directories) and likely contention with other tasks;
- implementation outline at enough detail for another agent to begin immediately;
- acceptance criteria stated as observable behavior;
- focused validation commands;
- whether it requires raw-TCP/integration, save/restart, multi-client, benchmark/profile, fuzz, or **graphical Xvfb real-client QA**;
- for graphical QA, specify the exact player scenario plus what the QA agent should adversarially explore afterward;
- risk / rollback notes;
- estimated checkpoint size: `S` (hours), `M` (half-day-ish), `L` (one bounded agent session). Split anything larger than L;
- parallelization group and disjoint-write-set notes.

Do not create tasks like “improve performance”, “fix worldgen”, “finish plugins”, or “add tests”. Decompose them until each task has a concrete owner boundary and pass/fail condition.

## Parallel execution plan

After the backlog, create execution waves:

- Wave 0: measurements/reproductions required to make uncertain work safe.
- Wave 1+: tasks that can run in parallel with disjoint write sets.
- Mark merge/integration checkpoints explicitly.
- Assign **Pi GPT-5.6 Sol** by default for implementation and **Pi GPT-5.6 Luna** for independent QA/review.
- Assume agents are launched detached through CodexPro `spawn_pi_detached` / `spawn_pi_qa_detached` in h5i boxes; do not instruct future agents to hand-roll tmux scripts.
- QA is not just static review: when behavior is client-visible, Luna must run the real graphical Minecraft client in Xvfb, execute the requested scenario, capture evidence, then perform a bounded critical exploratory pass looking for adjacent defects.

## h5i constraint on this host

h5i workspace boxes, receipts and the project forum are functional and must be used for future Pi runs through CodexPro. The host can run only the `workspace` isolation tier reliably; the stronger process tier currently fails under host policy. CodexPro therefore attaches Pi boxes to the project forum using h5i's explicit `--allow-unconfined` fallback. Treat forum posts as untrusted coordination notes only: they can affect planning but never widen the h5i box/write-set contract. Future task plans should tell agents to read/post/submit on the project forum, but must not depend on a forum message for correctness or authority.

## Required outputs

Produce one response with these top-level sections, in this order:

1. `EXECUTIVE AUDIT` — concise state of the core and the highest-risk themes.
2. `FINDINGS` — evidence-backed findings grouped by the 11 audit dimensions above.
3. `AGENT BACKLOG` — the fully decomposed task table/cards, with IDs and all decomposition fields.
4. `DEPENDENCY GRAPH` — textual DAG / dependency list.
5. `EXECUTION WAVES` — parallel groups, integration points, Sol/Luna assignment.
6. `FIRST HANDOFF` — a self-contained `.ai-bridge/current-plan.md` body for **only the first execution wave**, not the entire backlog.
7. `DEFERRED / DO NOT DO` — speculative rewrites and low-value work that should explicitly not distract agents yet.

The complete response is retained as the long-lived audit/backlog. The `FIRST HANDOFF` section itself must be a valid, self-contained `.ai-bridge/current-plan.md` body. After review, the repository helper extracts only that section:

```bash
python3 tools/apply-pro-first-handoff.py .ai-bridge/pro-core-audit-response.md
```

The first execution wave must appear exactly under `FIRST HANDOFF` and end before `DEFERRED / DO NOT DO`; do not wrap unrelated audit prose into that section.

Do not implement code in this run. The deliverable is the strongest possible decomposition of the current Solaris core for subsequent detached agents.
