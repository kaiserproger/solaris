# Solaris Spark Task Board

Snapshot basis: **2026-07-24 Repomix snapshot**. Coordinator owns this file; workers update only their own task card.

## Status legend

- `[ ]` queued/not integrated
- `[x]` integrated and acceptance/evidence reviewed
- Live `CLAIMED/IMPLEMENTING/TESTING/BLOCKED` state lives in the linked task card.
- `TEMPLATE` means the card must be rewritten with exact paths and RED commands before claim.

## Scheduling contract

- The table below is dependency-valid for the snapshot: every prerequisite is in an earlier batch.
- A dependency is satisfied only by integrated `DONE`, never by `partial` or `BLOCKED`.
- After any blocker, child task or changed dependency, use `python3 scripts/board.py ready`; do not blindly continue the static table.
- Broad validation, clean-host performance, root orchestration and coordinator-only gates are deliberately single-agent batches.

## Dependency-valid baseline batches

| Batch | Worker A | Worker B | Why singleton / gate |
|---|---|---|---|
| `B01` | [`T00-01`](tasks/T00-01.md) | — | no compatible ready partner at this dependency frontier |
| `B02` | [`T00-02`](tasks/T00-02.md) | — | tree-frozen validation |
| `B03` | [`T00-03`](tasks/T00-03.md) | [`T00-04`](tasks/T00-04.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B04` | [`T00-05`](tasks/T00-05.md) | — | clean-host/perf |
| `B05` | [`T00-06`](tasks/T00-06.md) | [`T01-01`](tasks/T01-01.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B06` | [`T01-02`](tasks/T01-02.md) | [`T01-04`](tasks/T01-04.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B07` | [`T01-03`](tasks/T01-03.md) | [`T01-05`](tasks/T01-05.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B08` | [`T01-06`](tasks/T01-06.md) | [`T02-01`](tasks/T02-01.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B09` | [`T02-02`](tasks/T02-02.md) | [`T02-03`](tasks/T02-03.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B10` | [`T02-04`](tasks/T02-04.md) | [`T02-07`](tasks/T02-07.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B11` | [`T02-05`](tasks/T02-05.md) | [`T03-01`](tasks/T03-01.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B12` | [`T02-06`](tasks/T02-06.md) | [`T03-02`](tasks/T03-02.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B13` | [`T02-08`](tasks/T02-08.md) | [`T03-05`](tasks/T03-05.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B14` | [`T03-03`](tasks/T03-03.md) | [`T04-01`](tasks/T04-01.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B15` | [`T03-06`](tasks/T03-06.md) | [`T04-02`](tasks/T04-02.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B16` | [`T03-08`](tasks/T03-08.md) | [`T04-03`](tasks/T04-03.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B17` | [`T03-04`](tasks/T03-04.md) | [`T04-05`](tasks/T04-05.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B18` | [`T03-07`](tasks/T03-07.md) | [`T04-06`](tasks/T04-06.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B19` | [`T04-04`](tasks/T04-04.md) | [`T04-08`](tasks/T04-08.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B20` | [`T04-07`](tasks/T04-07.md) | [`T05-01`](tasks/T05-01.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B21` | [`T05-04`](tasks/T05-04.md) | [`T06-06`](tasks/T06-06.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B22` | [`T05-05`](tasks/T05-05.md) | [`T05-06`](tasks/T05-06.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B23` | [`T05-02`](tasks/T05-02.md) | [`T06-08`](tasks/T06-08.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B24` | [`T05-03`](tasks/T05-03.md) | [`T06-05`](tasks/T06-05.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B25` | [`T05-07`](tasks/T05-07.md) | — | no compatible ready partner at this dependency frontier |
| `B26` | [`T05-08`](tasks/T05-08.md) | [`T06-07`](tasks/T06-07.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B27` | [`T06-01`](tasks/T06-01.md) | — | root lock exclusive |
| `B28` | [`T06-02`](tasks/T06-02.md) | — | root lock exclusive |
| `B29` | [`T06-03`](tasks/T06-03.md) | — | root lock exclusive |
| `B30` | [`T06-04`](tasks/T06-04.md) | — | root lock exclusive |
| `B31` | [`T07-01`](tasks/T07-01.md) | — | clean-host/perf |
| `B32` | [`T07-02`](tasks/T07-02.md) | — | no compatible ready partner at this dependency frontier |
| `B33` | [`T07-03`](tasks/T07-03.md) | — | clean-host/perf |
| `B34` | [`T07-04`](tasks/T07-04.md) | — | clean-host/perf |
| `B35` | [`T07-05`](tasks/T07-05.md) | — | clean-host/perf |
| `B36` | [`T07-06`](tasks/T07-06.md) | — | clean-host/perf |
| `B37` | [`T07-07`](tasks/T07-07.md) | — | clean-host/perf |
| `B38` | [`T07-08`](tasks/T07-08.md) | — | clean-host/perf |
| `B39` | [`T08-01`](tasks/T08-01.md) | — | clean-host/perf, root lock exclusive |
| `B40` | [`T08-02`](tasks/T08-02.md) | — | root lock exclusive |
| `B41` | [`T08-03`](tasks/T08-03.md) | [`T08-05`](tasks/T08-05.md) | Both must reach integrated `DONE`; then run one independent review of the candidate. |
| `B42` | [`T08-04`](tasks/T08-04.md) | — | root lock exclusive |
| `B43` | [`T08-06`](tasks/T08-06.md) | — | clean-host/perf |
| `B44` | [`T09-01`](tasks/T09-01.md) | — | clean-host/perf |
| `B45` | [`T09-02`](tasks/T09-02.md) | — | clean-host/perf |
| `B46` | [`T09-03`](tasks/T09-03.md) | — | clean-host/perf |
| `B47` | [`T09-04`](tasks/T09-04.md) | — | no compatible ready partner at this dependency frontier |
| `B48` | [`T10-01`](tasks/T10-01.md) | — | coordinator-only |
| `B49` | [`T10-02`](tasks/T10-02.md) | — | coordinator-only |
| `B50` | [`T10-03`](tasks/T10-03.md) | — | coordinator-only |
| `B51` | [`T10-04`](tasks/T10-04.md) | — | tree-frozen validation, coordinator-only |

## Full backlog

### W00 — Фиксация текущей истины

- [x] [`T00-01` — Зафиксировать фактический HEAD, dirty tree и владельцев изменений](tasks/T00-01.md) — **P0**, rows `—`, deps `—`, locks `COORD`
- [x] [`T00-02` — Снять честный Cargo/Gradle baseline и список реально красных гейтов](tasks/T00-02.md) — **P0**, rows `—`, deps `T00-01`, locks `VALIDATION`
- [x] [`T00-03` — Собрать актуальную матрицу real-client сценариев и артефактов](tasks/T00-03.md) — **P0**, rows `Q2`, deps `T00-01`, locks `CLIENT-JAVA,RUNNER`
- [x] [`T00-04` — Собрать актуальную матрицу vanilla oracle/replay](tasks/T00-04.md) — **P0**, rows `Q1,Q3`, deps `T00-01`, locks `ORACLE,RUST-HARNESS`
- [x] [`T00-05` — Собрать текущую performance/concurrency базу и пробелы](tasks/T00-05.md) — **P0**, rows `O1,O2,O3`, deps `T00-01`, locks `PERF`
- [x] [`T00-06` — Свернуть противоречивые/stale документы в один migration report](tasks/T00-06.md) — **P0**, rows `—`, deps `T00-01,T00-03,T00-04,T00-05`, locks `COORD-DOCS`

### W01 — Evidence laboratory: real-client/oracle/replay

- [x] [`T01-01` — Добавить компактный core-gate manifest: scenario → ledger rows → evidence legs](tasks/T01-01.md) — **P0**, rows `Q1,Q2,Q3`, deps `T00-03,T00-04`, locks `RUST-HARNESS`
- [x] [`T01-02` — Разрезать broad block/fluid real-client gate на независимые focused phases](tasks/T01-02.md) — **P0**, rows `B1,B2,B3,B4,Q2`, deps `T00-03`, locks `CLIENT-JAVA`
- [x] [`T01-03` — Разрезать inventory/crafting/container real-client gate на focused phases](tasks/T01-03.md) — **P0**, rows `I1,I2,K1,Q2`, deps `T00-03`, locks `CLIENT-JAVA`
- [x] [`T01-04` — Добавить restart invariant snapshot и строгую cross-phase валидацию](tasks/T01-04.md) — **P0**, rows `S1,Q2,Q3`, deps `T01-01`, locks `RUNNER,RUST-HARNESS`
- [x] [`T01-05` — Добавить vanilla oracle для block transaction/rejection/resync](tasks/T01-05.md) — **P0**, rows `B1,B2,B3,Q1`, deps `T00-04,T01-01`, locks `ORACLE,RUST-HARNESS`
- [ ] [`T01-06` — Добавить vanilla oracle для inventory/crafting/container state machine](tasks/T01-06.md) — **P0**, rows `I1,I2,K1,Q1`, deps `T00-04,T01-01`, locks `ORACLE,RUST-HARNESS`

### W02 — Обычная играбельность и client-visible блокеры

- [ ] [`T02-01` — Закрыть B4: воспроизводимый real-client water/swim feel gate](tasks/T02-01.md) — **P0**, rows `B4,Q2`, deps `T01-02`, locks `CLIENT-JAVA,RUNNER`
- [ ] [`T02-02` — Закрыть P4: полный respawn bundle + restart/rejoin evidence](tasks/T02-02.md) — **P0**, rows `P4,G4,S1,Q2`, deps `T01-04`, locks `CLIENT-JAVA,RUST-NET-SESSION`
- [ ] [`T02-03` — Заменить small full-cube fallback на точный common sturdy-face contract](tasks/T02-03.md) — **P0**, rows `B1,B2`, deps `T01-05`, locks `RUST-DATA,RUST-NET-BLOCKS`
- [ ] [`T02-04` — Закрыть stair neighbour-shape recomputation real-path proof](tasks/T02-04.md) — **P0**, rows `B1`, deps `T02-03`, locks `RUST-NET-BLOCKS,RUST-HARNESS`
- [ ] [`T02-05` — Доказать door/trapdoor state convergence для двух клиентов](tasks/T02-05.md) — **P0**, rows `B2,S2`, deps `T01-02`, locks `RUST-NET-BLOCKS,CLIENT-JAVA`
- [ ] [`T02-06` — Закрыть scheduled fluid spread + save/restart real-client path](tasks/T02-06.md) — **P0**, rows `B3,S1,Q2`, deps `T01-02,T01-04`, locks `RUST-NET-BLOCKS,CLIENT-JAVA`
- [ ] [`T02-07` — Зафиксировать representative movement boundary matrix](tasks/T02-07.md) — **P0**, rows `B5`, deps `T02-01`, locks `RUST-NET-SESSION`
- [ ] [`T02-08` — Закрыть chunk visibility/rejoin без ghost chunks](tasks/T02-08.md) — **P0**, rows `C1,Q2`, deps `T01-04`, locks `RUST-NET-CHUNK,CLIENT-JAVA`

### W03 — Inventory, crafting и containers

- [ ] [`T03-01` — Window-0 cursor rejection/resync conservation](tasks/T03-01.md) — **P0**, rows `I1,Q3`, deps `T01-03,T01-06`, locks `RUST-NET-CONTAINERS`
- [ ] [`T03-02` — Crafting-table max-craft и cursor conservation](tasks/T03-02.md) — **P0**, rows `I1,I2`, deps `T03-01`, locks `RUST-NET-CONTAINERS`
- [ ] [`T03-03` — Recipe-book discovery/window sync real-client gate](tasks/T03-03.md) — **P0**, rows `I2,Q2`, deps `T01-03,T03-02`, locks `CLIENT-JAVA,RUST-NET-CONTAINERS`
- [ ] [`T03-04` — Furnace/smoker/blast/campfire recipe execution representative client path](tasks/T03-04.md) — **P1**, rows `I2,K1,E2`, deps `T03-03`, locks `RUST-NET-CONTAINERS,CLIENT-JAVA`
- [ ] [`T03-05` — Chest max-stack metadata + malformed edge matrix](tasks/T03-05.md) — **P0**, rows `K1,Q3`, deps `T03-01`, locks `RUST-NET-CONTAINERS`
- [ ] [`T03-06` — Two-client shared chest concurrent-click real-client gate](tasks/T03-06.md) — **P0**, rows `K1,S2,Q2`, deps `T03-05`, locks `CLIENT-JAVA`
- [ ] [`T03-07` — Two-client shared furnace concurrent-click real-client gate](tasks/T03-07.md) — **P1**, rows `K1,S2,Q2`, deps `T03-04`, locks `CLIENT-JAVA`
- [ ] [`T03-08` — Two-client container save/restart convergence](tasks/T03-08.md) — **P0**, rows `K1,S1,S2`, deps `T01-04,T03-06`, locks `CLIENT-JAVA,RUNNER`

### W04 — Drops, loot, farming и renewable progression

- [ ] [`T04-01` — Dropped item merge: exact identity/count/age/version conservation](tasks/T04-01.md) — **P0**, rows `L2`, deps `T00-06`, locks `RUST-ENTITY,RUST-NET-SESSION`
- [ ] [`T04-02` — Partial pickup и overflow conservation](tasks/T04-02.md) — **P0**, rows `L2,I1`, deps `T04-01,T03-05`, locks `RUST-NET-SESSION`
- [ ] [`T04-03` — Item despawn deadline + restart proof](tasks/T04-03.md) — **P0**, rows `L2,S1`, deps `T00-06`, locks `RUST-NET-SESSION,RUST-HARNESS`
- [ ] [`T04-04` — Two-client shared pickup contention real-client gate](tasks/T04-04.md) — **P0**, rows `L2,S2,Q2`, deps `T04-02`, locks `CLIENT-JAVA`
- [x] [`T04-05` — Loot executor: random count ranges и multiple rolls core slice](tasks/T04-05.md) — **P1**, rows `L1`, deps `T00-04`, locks `RUST-DATA`
- [x] [`T04-06` — Loot context vertical slice: Fortune/Silk/Looting/burning](tasks/T04-06.md) — **P1**, rows `L1,G1`, deps `T04-05`, locks `RUST-DATA,RUST-ENTITY`
- [ ] [`T04-07` — Renewable wheat → bread lifecycle real-client gate](tasks/T04-07.md) — **P0**, rows `F1,I2,Q2`, deps `T03-03`, locks `CLIENT-JAVA,RUST-NET-BLOCKS`
- [ ] [`T04-08` — Sugar cane/cactus support cascade representative parity](tasks/T04-08.md) — **P1**, rows `F3`, deps `T01-05`, locks `RUST-NET-BLOCKS,RUST-HARNESS`

### W05 — Combat, death и entity authority

- [ ] [`T05-01` — Common damage-source matrix + exact rejection boundaries](tasks/T05-01.md) — **P0**, rows `G1`, deps `T00-06`, locks `RUST-ENTITY,RUST-NET-SESSION`
- [ ] [`T05-02` — Arrow lifecycle: spawn/flight/hit/stick/pickup representative oracle](tasks/T05-02.md) — **P1**, rows `G2`, deps `T05-01`, locks `RUST-ENTITY,RUST-NET-SESSION`
- [ ] [`T05-03` — Shield angle/timing + axe-disable representative path](tasks/T05-03.md) — **P1**, rows `G3`, deps `T05-01`, locks `RUST-NET-SESSION,RUST-ENTITY`
- [ ] [`T05-04` — Player death inventory/XP drop conservation](tasks/T05-04.md) — **P0**, rows `G4,L2,S1`, deps `T02-02,T04-02`, locks `RUST-NET-SESSION,RUST-ENTITY`
- [ ] [`T05-05` — Two-client contested death drops + restart](tasks/T05-05.md) — **P0**, rows `G4,S1,S2,Q2`, deps `T05-04,T04-04,T01-04`, locks `CLIENT-JAVA,RUNNER`
- [ ] [`T05-06` — Entity snapshot version fence across owner/wire/persistence](tasks/T05-06.md) — **P0**, rows `N1,O2`, deps `T00-06`, locks `RUST-ENTITY,RUST-NET-SESSION`
- [ ] [`T05-07` — Entity spawn/despawn cap + restart invariants](tasks/T05-07.md) — **P1**, rows `N1,S1`, deps `T05-06`, locks `RUST-NET-SESSION,RUST-ENTITY`
- [ ] [`T05-08` — Representative species AI/pathing parity table + client proof](tasks/T05-08.md) — **P1**, rows `N1`, deps `T05-07`, locks `RUST-ENTITY,CLIENT-JAVA`

### W06 — Durability, multiplayer pressure и online-mode

- [ ] [`T06-01` — Fault injection: campfire world + entity/drop journal outcome](tasks/T06-01.md) — **P0**, rows `E2,S1,O2`, deps `T00-06`, locks `RUST-NET-ROOT,RUST-WORLD`
- [ ] [`T06-02` — Fault injection: chained/simultaneous TNT world/entity outcome](tasks/T06-02.md) — **P0**, rows `G1,S1,O2`, deps `T06-01`, locks `RUST-NET-ROOT,RUST-WORLD,RUST-ENTITY`
- [ ] [`T06-03` — Fault injection: cross-region hopper compound commit](tasks/T06-03.md) — **P0**, rows `A1,K1,S1,O2`, deps `T06-01`, locks `RUST-NET-ROOT,RUST-WORLD`
- [ ] [`T06-04` — Shutdown while journal/checkpoint outcome is unknown](tasks/T06-04.md) — **P0**, rows `S1,O2`, deps `T06-01,T06-03`, locks `RUST-NET-ROOT,RUST-WORLD`
- [x] [`T06-05` — Real Anvil compression corpus + unknown NBT preservation](tasks/T06-05.md) — **P1**, rows `W2,S1`, deps `T00-04`, locks `RUST-WORLD`
- [ ] [`T06-06` — Disconnect during pending chunk prepare/load/generate](tasks/T06-06.md) — **P0**, rows `C1,S2,O2`, deps `T02-08`, locks `RUST-NET-CHUNK,RUST-HARNESS`
- [ ] [`T06-07` — Natural TCP slow-reader fairness, shedding и recovery](tasks/T06-07.md) — **P0**, rows `S2,O2,O3`, deps `T06-06`, locks `RUST-NET-SESSION,RUST-HARNESS`
- [ ] [`T06-08` — External online-mode paid-client qualification](tasks/T06-08.md) — **P0**, rows `P2,P3,O4`, deps `T00-02`, locks `EXTERNAL`; **TEMPLATE**

### W07 — Профилирование low/balanced/high

- [x] [`T07-01` — Заморозить low/balanced/high profile configs и budgets](tasks/T07-01.md) — **P0**, rows `O1,O3`, deps `T00-05`, locks `PERF,COORD-DOCS`
- [ ] [`T07-02` — Versioned performance result schema + provenance validator](tasks/T07-02.md) — **P0**, rows `O1,O2,O3`, deps `T07-01`, locks `RUST-HARNESS`
- [ ] [`T07-03` — Solo generated-world VD8 clean-host profile](tasks/T07-03.md) — **P0**, rows `W3,C1,C2,O1,O2`, deps `T07-02,T06-05`, locks `PERF`
- [ ] [`T07-04` — Two-client ordinary survival responsiveness profile](tasks/T07-04.md) — **P0**, rows `S2,O1,O2`, deps `T07-02,T03-06,T04-04`, locks `PERF,CLIENT-JAVA`
- [ ] [`T07-05` — 20-client VD8 balanced profile](tasks/T07-05.md) — **P0**, rows `O1,O2,O3,S2`, deps `T07-02,T06-07`, locks `PERF,RUST-HARNESS`
- [ ] [`T07-06` — Dense entity/AI/physics profile](tasks/T07-06.md) — **P0**, rows `N1,O1,O2`, deps `T07-02,T05-08`, locks `PERF`
- [ ] [`T07-07` — Fluid/random/scheduled-block profile](tasks/T07-07.md) — **P0**, rows `B3,F1,A1,O1,O2`, deps `T07-02,T02-06`, locks `PERF`
- [ ] [`T07-08` — Save/autosave/dirty-flush profile under live mutation](tasks/T07-08.md) — **P0**, rows `S1,O1,O2`, deps `T07-02,T06-04`, locks `PERF`

### W08 — Только измеренные оптимизации и authority cutovers

- [ ] [`T08-01` — Ранжировать remaining WorldHandle locksites по measured impact](tasks/T08-01.md) — **P0**, rows `O2`, deps `T07-03,T07-05,T07-08`, locks `RUST-NET-ROOT,PERF`
- [ ] [`T08-02` — Перевести один dominant resident mutation с global WorldHandle](tasks/T08-02.md) — **P0**, rows `O2`, deps `T08-01`, locks `RUST-NET-ROOT,RUST-WORLD`; **TEMPLATE**
- [ ] [`T08-03` — Убрать measured chunk-stream global-lock bottleneck](tasks/T08-03.md) — **P0**, rows `C1,O2`, deps `T08-01`, locks `RUST-NET-CHUNK,RUST-WORLD`; **TEMPLATE**
- [ ] [`T08-04` — Исправить главный measured save/install/flush stall](tasks/T08-04.md) — **P0**, rows `S1,O2`, deps `T07-08,T08-01`, locks `RUST-WORLD,RUST-NET-ROOT`; **TEMPLATE**
- [ ] [`T08-05` — Исправить главный measured entity goal/physics/publication hotspot](tasks/T08-05.md) — **P0**, rows `N1,O2`, deps `T07-06,T05-06`, locks `RUST-ENTITY,RUST-NET-SESSION`; **TEMPLATE**
- [ ] [`T08-06` — Autoscale recovery + slow-client shedding profile/fix](tasks/T08-06.md) — **P0**, rows `O3,S2`, deps `T07-05,T06-07`, locks `RUST-NET-SESSION,PERF`; **TEMPLATE**

### W09 — Интегрированные survival/multiplayer/soak gates

- [ ] [`T09-01` — Двухчасовой no-debug single-client active survival arc](tasks/T09-01.md) — **P0**, rows `Q2,S1,O1`, deps `T02-01,T02-02,T03-08,T04-07,T05-05,T08-04,T08-05`, locks `CLIENT-JAVA,RUNNER,PERF`
- [ ] [`T09-02` — 30-minute two-client cooperative survival arc](tasks/T09-02.md) — **P0**, rows `S2,Q2,O1`, deps `T03-06,T03-08,T04-04,T05-05,T06-07,T08-06`, locks `CLIENT-JAVA,RUNNER,PERF`
- [ ] [`T09-03` — 36,000-tick / 20-client mixed soak с failing-seed manifests](tasks/T09-03.md) — **P0**, rows `S2,O1,O2,O3,Q3`, deps `T07-05,T07-07,T07-08,T08-06`, locks `PERF,RUST-HARNESS`
- [ ] [`T09-04` — Единый authoritative pre-stop/post-restart state diff](tasks/T09-04.md) — **P0**, rows `S1,S2,Q3`, deps `T01-04,T09-01,T09-02`, locks `RUST-HARNESS,RUNNER`

### W10 — M100 evidence closure

- [ ] [`T10-01` — Пересчитать все 46 frozen rows по текущему коду и evidence](tasks/T10-01.md) — **P1**, rows `Q1,Q2`, deps `T09-03,T09-04`, locks `COORD-DOCS`; **COORDINATOR**
- [ ] [`T10-02` — Закрыть hard required-green Q1/Q2/O1/O2/O3 blockers](tasks/T10-02.md) — **P1**, rows `Q1,Q2,O1,O2,O3`, deps `T10-01`, locks `COORD`; **COORDINATOR**
- [ ] [`T10-03` — Итеративно закрывать smallest missing legs до ≥37/46 ready](tasks/T10-03.md) — **P1**, rows `—`, deps `T10-02`, locks `COORD`; **COORDINATOR**
- [ ] [`T10-04` — Финальный L2, M100 decision и честный closeout](tasks/T10-04.md) — **P1**, rows `—`, deps `T10-03`, locks `VALIDATION,COORD-DOCS`; **COORDINATOR**

## Explicitly deferred unless evidence reopens them

- Bit-perfect Mojang worldgen/NoiseRouter parity (`W3` stays deliberate divergence).
- Broad vehicles/minecarts, all advanced stations and full species AI before common survival/perf gates.
- Windows existing-region atomic replacement unless Windows is an owner target.
- Native/WASM extension host, cluster/shared-world autoscaling and decorative parity.
- Rare save interleavings that do not affect ordinary save corruption or the selected gate.

## Batch close checklist

- [ ] Both task cards are `DONE` or the blocked branch has an explicit child task; blocked is not a satisfied dependency.
- [ ] Integrated candidate has one independent read-only reviewer verdict.
- [ ] No active write lock or runtime lease remains orphaned.
- [ ] Validation cache records tree/environment/scope.
- [ ] Canonical owner doc is updated once, not by both workers.
- [ ] `python3 scripts/board.py validate` passes before the next launch.
