# Solaris Spark Team Pack — ALL IN ONE

> Archive/search copy only. Spark workers must use one task card, not this file.

---

# FILE: README.md

# Solaris — Spark Team Pack

Основано на **2026-07-24 Repomix snapshot**. Пакет рассчитан на `codex-5.3-spark`: маленький контекст, короткие checkpoint’ы, максимум два параллельных worker’а и один независимый read-only reviewer после интеграции.

## Что видно по snapshot

Ядро уже заметно сильнее старого M100-леджера: актуальные playable/memory записи описывают две agent-driven 20-минутные survival-сессии, stone progression, reconnect/restart, hostile combat, глубокую воду, dense-entity liveness и несколько измеренных hot-path исправлений. Поэтому старые `0/46 ready` и часть очередей середины июля нельзя раздавать как текущую истину без сверки.

Главные реальные риски:

1. **Evidence отстаёт от реализации:** Q1/Q2 и статусы 46 строк не отражают весь новый код и client evidence.
2. **Нет единой low/balanced/high performance matrix и интегрированного soak.**
3. **World/storage authority ещё staged:** global `WorldHandle` остаётся на части resident-путей; cross-journal crash outcomes не закрыты системно.
4. **Broad parity нельзя выводить из supported slices:** нужны точные oracle + real-client legs и минимальные исправления только по доказанным расхождениям.

## Что внутри

- **74 карточки**: одна наблюдаемая цель, узкий read/write set, точные зависимости, locks, runtime leases, чекбоксы и YAML handoff.
- **51 dependency-valid baseline batch’ей**, из них 23 двухагентных. Perf/L2/root/coordinator gates намеренно одиночные.
- `TEMPLATE`-карточки для measured hotspots нельзя отдавать агенту, пока координатор не подставит точные пути и RED-команду.
- Машинная проверка DAG/locks/batches/statuses через `scripts/board.py`.

## Целевые гейты кампании

### `PLAYABLE-GREEN`

- свежий no-debug клиент проходит активную 2-часовую survival-дугу до железного уровня, renewable food, combat/death recovery и restart/rejoin;
- два клиента 30 минут совместно играют через shared storage, drops, block edits, reconnect и restart;
- нет crash, disconnect, duplication, invisible loss, ghost state или catastrophic stall.

### `PERF-GREEN`

- frozen low/balanced/high profiles;
- balanced: 20 clients, VD8, целевой `>18 TPS`, точные p50/p95/p99/max и chunk/lock/queue/memory/save/outbound метрики;
- нет unexplained tick/lock stall, unbounded queue/retry/memory growth;
- функциональный real-client path остаётся зелёным под нагрузкой.

### `PARITY-GREEN`

- scoped vanilla-observable semantics, не bit-perfect worldgen;
- для counted row есть runtime test **и** отдельный vanilla oracle или real-client leg;
- M100 `>=37/46` только после повторного аудита фактического дерева; missing evidence нельзя называть divergence.

## Начало работы

1. Скопировать каталог в репозиторий как `docs/spark-team/`.
2. Прочитать [`START_HERE.md`](START_HERE.md), затем [`FLOW.md`](FLOW.md).
3. Выполнить `python3 docs/spark-team/scripts/board.py validate`.
4. Выдать worker’у только `AGENTS.md`, одну карточку, base SHA, worktree и locks — без parent conversation и полного roadmap.
5. Worker меняет чекбоксы только своей карточки; `BOARD.md` меняет координатор после merge/cherry-pick.
6. После candidate diff — один независимый read-only reviewer из [`PROMPTS.md`](PROMPTS.md).

## Файлы

- [`START_HERE.md`](START_HERE.md) — первые batch’и и команды координатора.
- [`FLOW.md`](FLOW.md) — team flow, context paging, locks, leases, validation и handoff.
- [`BOARD.md`](BOARD.md) — валидный baseline schedule и полный backlog.
- [`PROMPTS.md`](PROMPTS.md) — короткие copy-paste prompts.
- [`tasks/`](tasks/) — отдельная маленькая карточка на задачу.
- [`manifest.json`](manifest.json) — machine-readable DAG.
- [`scripts/board.py`](scripts/board.py) — `validate`, `ready`, `summary`.
- [`ALL_IN_ONE.md`](ALL_IN_ONE.md) — только архив/поиск; worker’ы его не читают.

## Ограничение snapshot

Repomix может не включать untracked/ignored operator-файлы вроде активного `example.toml`, локальных runner-скриптов или credentials. Поэтому W00 фиксирует фактический HEAD/dirty tree и материализует такие пути до выдачи карточек. Пакет не предполагает, что старый snapshot равен сегодняшнему checkout.

---

# FILE: START_HERE.md

# Start Here — первые действия координатора

## Нулевая проверка

```sh
cp -R /path/to/solaris-spark-team docs/spark-team
python3 docs/spark-team/scripts/board.py validate
python3 docs/spark-team/scripts/board.py ready
```

Не запускайте worker’ов из одного checkout. Для каждого задания создайте отдельный worktree и уникальные run/world dirs.

## Первые пять batch’ей

### `B01` — coordinator truth cursor

- Worker A: `T00-01`.
- Worker B: отсутствует: все остальные задачи зависят от фактического HEAD/dirty-tree снимка.

### `B02` — frozen baseline

- Worker A: `T00-02`.
- Worker B: отсутствует: broad Cargo/Gradle baseline выполняется на неизменном дереве.

### `B03` — две независимые evidence-матрицы

- Worker A: `T00-03` — real-client scenarios/artifacts.
- Worker B: `T00-04` — vanilla oracle/replay.

### `B04` — clean-host performance inventory

- Worker A: `T00-05`.
- Worker B: отсутствует: не загрязнять workload и hardware provenance.

### `B05` — reconcile + core manifest

- Worker A: `T00-06` — stale/current reconciliation.
- Worker B: `T01-01` — scenario → ledger → evidence manifest.

После каждого batch:

```sh
python3 docs/spark-team/scripts/board.py validate
python3 docs/spark-team/scripts/board.py ready
```

`BLOCKED` не разблокирует descendants. Координатор создаёт одну child-card на точный blocker, добавляет её в `manifest.json`, регенерирует/проверяет DAG и только потом продолжает.

## Worktree convention

```sh
BASE_SHA=$(git rev-parse HEAD)
git worktree add ../solaris-worktrees/T00-03 -b agent/T00-03 "$BASE_SHA"
```

Назначения среды:

- Worker A: ports `25570-25579`, `.analysis/runs/TASK_ID-a/`;
- Worker B: ports `25580-25589`, `.analysis/runs/TASK_ID-b/`;
- `CLIENT-RIG`, `ORACLE-RIG`, `CLEAN-HOST`, `TREE-FROZEN`, `PAID-AUTH` выдаются как эксклюзивные leases из карточки.

## Минимальный launch prompt

Используйте `Coordinator → worker` из `PROMPTS.md`. Не добавляйте полный контекст проекта: карточка уже является context capsule.

---

# FILE: FLOW.md

# Spark Team Flow

## 1. Campaign override

Owner explicitly authorizes a bounded multi-agent core campaign. Repository cap still applies: **maximum two concurrent workers**, disjoint responsibilities/write sets, then one independent read-only reviewer per integrated candidate.

Campaign priority: **ordinary playability → scoped parity evidence → measured performance → only measured architecture cutovers**. Plugin/Loader breadth pauses unless it blocks those gates.

## 2. Roles

### Coordinator

- owns `BOARD.md`, DAG, path locks, runtime leases, base SHA, integration and validation cache;
- gives a worker only `AGENTS.md`, one task card, exact worktree/base SHA/ports/run dir;
- never gives Spark the parent conversation, whole roadmap, full ledger or archive;
- materializes every `TEMPLATE` card before claim;
- turns every material blocker into one child card instead of widening the active task.

### Worker A / Worker B

- one card, one worktree, one branch;
- edit only `Owned write paths`; extra path requires coordinator handoff;
- one bounded discovery batch, one edit batch, one focused validation batch, one closeout;
- update status and checkboxes in the task card as work advances.

### Reviewer

- sequential, read-only, no subagents, no edits;
- receives acceptance contract, path-limited diff and validation summary only;
- returns `pass | changes | blocked`, maximum eight findings;
- implementer fixes concrete findings; no reviewer carousel.

## 3. Status protocol

```md
Status: `QUEUED | CLAIMED | IMPLEMENTING | TESTING | REVIEW | DONE | BLOCKED`

- [ ] CLAIMED
- [ ] BASELINE / RED
- [ ] IMPLEMENTED
- [ ] TESTING
- [ ] SELF-REVIEW
- [ ] INDEPENDENT REVIEW
- [ ] DONE
```

- Claim: fill agent/worktree/base SHA/start time, set `CLAIMED`, tick `CLAIMED`.
- Implementation begins only after a reproducible RED/gap; set `IMPLEMENTING` and tick `BASELINE / RED`.
- Testing: set `TESTING`, append exact command, result and log path; tick only after command actually ran.
- Review: set `REVIEW` only after self-review and compact closeout exist.
- Blocked: set `BLOCKED`, record one blocker fingerprint, proof and exact next command. **Blocked never satisfies a dependency.**
- Done: all boxes checked, integrated diff/commit/evidence present; coordinator then checks `BOARD.md`.

## 4. Worktree/Git protocol

```sh
git worktree add ../solaris-worktrees/<TASK_ID> -b agent/<TASK_ID> <BASE_SHA>
cd ../solaris-worktrees/<TASK_ID>
```

- Never `reset`, `clean`, rewrite or stage owner/unrelated files.
- Never stage `.analysis/`, `data/vanilla/`, run dirs, Mojang bytes, local logs or secrets unless the card explicitly owns a sanitized report.
- One coherent Conventional Commit only when authorized.
- Without commit authorization, return base tree, diff hash, changed files and one next action.

## 5. Spark context budget

A worker loads:

1. `AGENTS.md` once;
2. its task card;
3. at most one small route/ADR slice named by the card;
4. normally no more than four production/test files before editing.

Hard limits:

- one behavior or one evidence leg;
- max 3 production files + 2 test files + 1 owning doc;
- 8 soft / 12 hard model roundtrips;
- 6 shell batches;
- no context compaction;
- no full repo survey, milestone range, archive sweep or `ALL_IN_ONE.md`.

### Source paging protocol

For a file over 400 lines:

```sh
rg -n 'anchor_one|anchor_two|error_code' exact/path.rs
sed -n '<line-80>,<line+160>p' exact/path.rs
```

Open at most three windows of at most 160 lines each. Never `cat` `play.rs`, `simulation.rs`, `server.rs`, `regional.rs` or large generated data. Use `rg --files` only inside the card’s declared directories.

When limits are reached, close `partial` and create a child card. Do not compress several failures into one Spark session.

## 6. Source-of-truth precedence

1. current source + focused tests + current runtime artifact;
2. `docs/MEMORY.md` and route memory;
3. exact anchored section of `docs/playable/ACTIVE.md`;
4. current review/WAL exact finding;
5. old restart checkpoints, milestone prose and validation statuses.

An old unchecked box is not work until current code/evidence confirms the gap.

## 7. Write locks

Two cards may run together only when write-lock sets are disjoint and the special rules below pass.

| Lock | Paths / meaning |
|---|---|
| `COORD` | board, DAG, global routing |
| `COORD-DOCS` | canonical memory/ledger/milestone docs |
| `VALIDATION` | broad workspace/Gradle gates; tree frozen |
| `RUST-NET-ROOT` | `play.rs`, `simulation.rs`, `server.rs`, root orchestration |
| `RUST-NET-BLOCKS` | placement, fluids, plants, toggles, scheduled blocks |
| `RUST-NET-CONTAINERS` | inventory, recipes, crafting/chest/furnace/stonecutter |
| `RUST-NET-SESSION` | session/player/entity/outbound/publication authority |
| `RUST-NET-CHUNK` | chunk stream/pipeline/view authority |
| `RUST-ENTITY` | entity runtime/regional/AI/combat/projectiles |
| `RUST-WORLD` | resident/storage/Anvil/light/dirty flush |
| `RUST-DATA` | vanilla data, loot, recipes, block/item facts |
| `RUST-HARNESS` | replay/load/real-client Rust harness |
| `CLIENT-JAVA` | Java client-agent scenarios/tools |
| `RUNNER` | Python/shell real-client orchestration |
| `ORACLE` | vanilla capture/manifests/comparison |
| `PERF` | long workload/results |
| `EXTERNAL` | paid client/external service qualification |

Special rules:

- `RUST-NET-ROOT` is exclusive with every other Rust runtime edit.
- `VALIDATION` is always singleton.
- A task with `PERF` is singleton unless it is a pure report task explicitly approved by the coordinator.
- Exact path overlap overrides lock names: overlapping writes never run together.

## 8. Runtime leases

Path-disjoint tasks can still corrupt each other’s evidence. Coordinator assigns leases:

| Lease | Rule |
|---|---|
| `CLIENT-RIG` | one actual Minecraft/client-agent run at a time |
| `ORACLE-RIG` | one vanilla capture/oracle process at a time |
| `CLEAN-HOST` | exclusive host; no compilation, IDE indexing or other workload |
| `TREE-FROZEN` | no source integration until the task closes |
| `PAID-AUTH` | owner-controlled credentials/network; secrets never enter Git/log artifacts |

Every worker also gets a unique port range, world dir, run dir and bridge secret. Shared default ports or shared `.analysis/test-world` are forbidden under parallel execution.

## 9. DAG and dispatch

- `READY`: claim only when all dependencies are integrated `DONE`.
- `TEMPLATE`: do not claim. Replace placeholders/prose paths with exact paths, one RED command, one rerun command, then validate.
- `COORDINATOR-ONLY`: umbrella/audit/release control; discovered coding work becomes child cards.

Static `BOARD.md` is a valid baseline, not an excuse to ignore live status:

```sh
python3 docs/spark-team/scripts/board.py validate
python3 docs/spark-team/scripts/board.py ready
python3 docs/spark-team/scripts/board.py summary
```

After any blocker, child task, changed dependency or cancelled hotspot, regenerate/revalidate before launch.

## 10. Validation cache

Identity: `(command, tree fingerprint, environment, covered scope)`.

- L0: exact focused tests + targeted diff/syntax.
- L1: affected crate/package tests, formatter, `cargo run -p xtask -- code-health` as required.
- L2 once per wave/commit candidate:

```sh
cargo run -p xtask -- code-health
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Never rerun an unchanged successful gate. After failure, rerun only the failed gate after a relevant edit. Long stdout goes to `.analysis/codex-logs/`; card stores status, short tail and path.

## 11. Evidence vocabulary

Never collapse these into “green”:

- `unit` — local contract only;
- `wire` — Solaris protocol/runtime path;
- `oracle` — independent vanilla capture/decompile/side-by-side comparison;
- `real-client-agent` — completed actual 26.1.2 client run;
- `owner-manual` — subjective/visual owner run;
- `performance` — reproducible workload with provenance and metrics;
- `concurrency` — exact race/pressure/ownership proof;
- `persistence` — disk/restart/crash-window proof.

A parity row needs runtime evidence plus an independent oracle or real-client leg. A faster unit benchmark is not server performance.

## 12. Worker closeout

```yaml
verdict: pass | changes | blocked
status: complete | partial | checkpoint-blocked
base_tree: <sha>
diff_hash: <sha256>
changed_files: [exact list]
validation:
  - command: ...
    result: pass | fail | degraded | skipped
    log: ...
evidence:
  - requirement: ...
    proof: ...
known_gaps: [max 8]
next: <one exact action>
```

Inline result under 1,000 characters; details live in the task card/report.

## 13. Reviewer gate

Reviewer checks only:

- acceptance requirements actually proven;
- duplicate authority / stale CAS / publication ordering;
- broader-than-needed abstraction/config/compat layer;
- missing dominant failure boundary;
- fake parity (expected facts copied from implementation);
- pass-by-construction, sleeps, polling or timeout-as-success;
- changed paths outside ownership;
- stale docs/claims.

## 14. Merge cadence

1. Freeze base SHA; issue at most two compatible cards.
2. Workers finish L1, self-review and closeout.
3. Coordinator inspects path-limited diffs and integrates.
4. One read-only reviewer checks the integrated candidate.
5. Original implementer handles concrete findings.
6. Coordinator runs only missing integration gates.
7. At wave boundary run one L2, update canonical status once, then open the next batch.

## 15. Stop rules

Stop when acceptance is met, context/tool budget is reached, artifact/service is absent, a needed lock is owned elsewhere, or discovery reveals another root cause. Record exact proof and create one new card. Never continue “helpfully” into another subsystem.

---

# FILE: BOARD.md

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

- [ ] [`T00-01` — Зафиксировать фактический HEAD, dirty tree и владельцев изменений](tasks/T00-01.md) — **P0**, rows `—`, deps `—`, locks `COORD`
- [ ] [`T00-02` — Снять честный Cargo/Gradle baseline и список реально красных гейтов](tasks/T00-02.md) — **P0**, rows `—`, deps `T00-01`, locks `VALIDATION`
- [ ] [`T00-03` — Собрать актуальную матрицу real-client сценариев и артефактов](tasks/T00-03.md) — **P0**, rows `Q2`, deps `T00-01`, locks `CLIENT-JAVA,RUNNER`
- [ ] [`T00-04` — Собрать актуальную матрицу vanilla oracle/replay](tasks/T00-04.md) — **P0**, rows `Q1,Q3`, deps `T00-01`, locks `ORACLE,RUST-HARNESS`
- [ ] [`T00-05` — Собрать текущую performance/concurrency базу и пробелы](tasks/T00-05.md) — **P0**, rows `O1,O2,O3`, deps `T00-01`, locks `PERF`
- [ ] [`T00-06` — Свернуть противоречивые/stale документы в один migration report](tasks/T00-06.md) — **P0**, rows `—`, deps `T00-01,T00-03,T00-04,T00-05`, locks `COORD-DOCS`

### W01 — Evidence laboratory: real-client/oracle/replay

- [ ] [`T01-01` — Добавить компактный core-gate manifest: scenario → ledger rows → evidence legs](tasks/T01-01.md) — **P0**, rows `Q1,Q2,Q3`, deps `T00-03,T00-04`, locks `RUST-HARNESS`
- [ ] [`T01-02` — Разрезать broad block/fluid real-client gate на независимые focused phases](tasks/T01-02.md) — **P0**, rows `B1,B2,B3,B4,Q2`, deps `T00-03`, locks `CLIENT-JAVA`
- [ ] [`T01-03` — Разрезать inventory/crafting/container real-client gate на focused phases](tasks/T01-03.md) — **P0**, rows `I1,I2,K1,Q2`, deps `T00-03`, locks `CLIENT-JAVA`
- [ ] [`T01-04` — Добавить restart invariant snapshot и строгую cross-phase валидацию](tasks/T01-04.md) — **P0**, rows `S1,Q2,Q3`, deps `T01-01`, locks `RUNNER,RUST-HARNESS`
- [ ] [`T01-05` — Добавить vanilla oracle для block transaction/rejection/resync](tasks/T01-05.md) — **P0**, rows `B1,B2,B3,Q1`, deps `T00-04,T01-01`, locks `ORACLE,RUST-HARNESS`
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
- [ ] [`T04-05` — Loot executor: random count ranges и multiple rolls core slice](tasks/T04-05.md) — **P1**, rows `L1`, deps `T00-04`, locks `RUST-DATA`
- [ ] [`T04-06` — Loot context vertical slice: Fortune/Silk/Looting/burning](tasks/T04-06.md) — **P1**, rows `L1,G1`, deps `T04-05`, locks `RUST-DATA,RUST-ENTITY`
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
- [ ] [`T06-05` — Real Anvil compression corpus + unknown NBT preservation](tasks/T06-05.md) — **P1**, rows `W2,S1`, deps `T00-04`, locks `RUST-WORLD`
- [ ] [`T06-06` — Disconnect during pending chunk prepare/load/generate](tasks/T06-06.md) — **P0**, rows `C1,S2,O2`, deps `T02-08`, locks `RUST-NET-CHUNK,RUST-HARNESS`
- [ ] [`T06-07` — Natural TCP slow-reader fairness, shedding и recovery](tasks/T06-07.md) — **P0**, rows `S2,O2,O3`, deps `T06-06`, locks `RUST-NET-SESSION,RUST-HARNESS`
- [ ] [`T06-08` — External online-mode paid-client qualification](tasks/T06-08.md) — **P0**, rows `P2,P3,O4`, deps `T00-02`, locks `EXTERNAL`; **TEMPLATE**

### W07 — Профилирование low/balanced/high

- [ ] [`T07-01` — Заморозить low/balanced/high profile configs и budgets](tasks/T07-01.md) — **P0**, rows `O1,O3`, deps `T00-05`, locks `PERF,COORD-DOCS`
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

---

# FILE: PROMPTS.md

# Spark Copy-Paste Prompts

## Coordinator → worker

```text
Owner authorizes this bounded Solaris task under the repository's two-worker cap.
Work only in the supplied worktree.

TASK: <TASK_ID>
BASE_SHA: <sha>
CARD: docs/spark-team/tasks/<TASK_ID>.md
OWNED_WRITE_PATHS: <copy exact paths from card>
ACTIVE_LOCKS: <copy from card>
RUNTIME_LEASES: <copy from card>
PORT_RANGE / RUN_DIR: <assigned unique values>

Before work, confirm Dispatch is READY and every dependency is integrated DONE.
Read AGENTS.md once, then the card. Do not read the full roadmap, ledger, ACTIVE
log, ALL_IN_ONE.md or parent conversation. Never cat a file over 400 lines: one
rg anchor batch, at most three 160-line windows. Update the card status/checks.
Do not edit outside owned paths. One RED, one edit batch, focused validation,
self-review, compact YAML closeout. Timeout only fails. No sleeps/polling. If
scope grows or the root cause needs another lock, stop partial and name exactly
one child task.
```

## Coordinator → evidence-only worker

```text
Evidence/audit only. Do not change production code. Use TASK <TASK_ID> and write
only its declared report/artifact paths. Separate unit, wire, oracle, real-client,
performance, concurrency and persistence evidence. An old doc claim is not current
evidence. Return the smallest missing leg, not a broad plan.
```

## Coordinator → reviewer

```text
Read-only review. Do not edit or spawn agents.

TASK: <TASK_ID>
ACCEPTANCE: <exact outcome + done_when>
BASE: <sha>
DIFF: <path-limited diff or commit>
VALIDATION: <compact command/results>

Check correctness, authority/CAS/publication ordering, scope, dominant failure
boundary, fake parity, pass-by-construction, sleeps/polling, path ownership and
stale claims. Return YAML only:
verdict: pass | changes | blocked
findings: [maximum 8 concise items]
validation_gaps: [...]
```

## Worker → coordinator closeout

```yaml
verdict: pass | changes | blocked
status: complete | partial | checkpoint-blocked
base_tree: ...
diff_hash: ...
changed_files: [...]
validation:
  - command: ...
    result: pass | fail | degraded | skipped
    log: ...
evidence:
  - requirement: ...
    proof: ...
known_gaps: [...]
next: ...
```

## Create one child card

```text
Create one Spark card from this blocker: one observable failure, one root
authority, max 3 production files, one success test, one dominant rejection/race
test, and one focused real-client/oracle/perf gate where relevant. Declare exact
paths, locks, leases and dependencies. Do not widen the parent card.
```

## Batch close audit

```text
Audit only this batch's done_when against the integrated tree. Do not audit the
whole north star. List complete/partial/blocked tasks, exact evidence, skipped
gates, reviewer verdict, validation-cache identity and the next compatible pair.
```

---

# FILE: tasks/T00-01.md

# T00-01 — Зафиксировать фактический HEAD, dirty tree и владельцев изменений

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `audit` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `none / campaign-level` |
| Depends on | `none` |
| Write locks | `COORD` |
| Runtime leases | `NONE` |
| Required evidence | `audit/control` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Один компактный снимок фактического дерева: branch/HEAD, tracked/untracked changes, активные процессы, артефакты и предполагаемый владелец каждого изменённого пути.

## Read-only context — do not broaden

- `AGENTS.md`
- `.memory/MEMORY.md`
- `docs/MEMORY.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/current-tree.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- git status --short --branch
- git rev-parse HEAD
- git diff --name-status; git diff --stat
- проверить только процессы/порты, относящиеся к Solaris

## Non-goals

- Не читать весь репозиторий
- Не исправлять найденные изменения
- Не обновлять канонические docs

## Required evidence legs

- `audit/control`

## Required validation

- `Проверить, что отчёт не содержит секретов и не предлагает reset/clean`
- `Ни одного исходника не менять`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T00-02.md

# T00-02 — Снять честный Cargo/Gradle baseline и список реально красных гейтов

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `validation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `none / campaign-level` |
| Depends on | `T00-01` |
| Write locks | `VALIDATION` |
| Runtime leases | `TREE-FROZEN` |
| Required evidence | `audit/control` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Список команд с точным статусом pass/fail/degraded/skipped на неизменном дереве; один раз на fingerprint.

## Read-only context — do not broaden

- `AGENTS.md`
- `README.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/baseline.md`
- `.analysis/codex-logs/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- cargo fmt --all -- --check
- cargo run -p xtask -- code-health
- cargo test --workspace
- cargo clippy --workspace --all-targets -- -D warnings
- client-mod/solaris-client-agent: ./gradlew test

## Non-goals

- Не чинить падения в этой карточке
- Не повторять успешную команду на том же tree fingerprint

## Required evidence legs

- `audit/control`

## Required validation

- `Полный stdout сохранить в .analysis/codex-logs`
- `В отчёт вынести только код возврата, failures, короткий tail и log path`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T00-03.md

# T00-03 — Собрать актуальную матрицу real-client сценариев и артефактов

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `playable` |
| Kind | `audit` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `Q2` |
| Depends on | `T00-01` |
| Write locks | `CLIENT-JAVA, RUNNER` |
| Runtime leases | `NONE` |
| Required evidence | `real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Для каждого playable/M94 scenario: implemented, declared, focused/broad, last completed artifact, pass/blocked/not-run, required manual/agent evidence.

## Read-only context — do not broaden

- `docs/playable/ACTIVE.md`
- `crates/mc-test-harness/tests/real_client_manifest.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/real-client-matrix.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Извлечь scenario IDs программно
- Сверить только последние артефакты по каждому ID
- Отдельно пометить interrupted/late-output runs

## Non-goals

- Не запускать Minecraft
- Не менять сценарии

## Required evidence legs

- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-test-harness --test real_client_manifest`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T00-04.md

# T00-04 — Собрать актуальную матрицу vanilla oracle/replay

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `parity` |
| Kind | `audit` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `Q1, Q3` |
| Depends on | `T00-01` |
| Write locks | `ORACLE, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `oracle, replay/negative` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Таблица: ledger row → существующий oracle/replay manifest → prerequisites → фактический статус → минимально недостающая нога.

## Read-only context — do not broaden

- `docs/DEFINITION_OF_DONE.md`
- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/src/replay.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/oracle-replay-matrix.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- rg по schema/scenario IDs, не печатать большие JSON
- Выделить local-artifact-dependent и silent skip

## Non-goals

- Не запускать долгий oracle suite
- Не редактировать ledger

## Required evidence legs

- `oracle`
- `replay/negative`
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.

## Required validation

- `cargo test -p mc-test-harness --test parity_oracle --no-run`
- `cargo test -p mc-test-harness --test parity_oracle -- --list`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T00-05.md

# T00-05 — Собрать текущую performance/concurrency базу и пробелы

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `audit` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `O1, O2, O3` |
| Depends on | `T00-01` |
| Write locks | `PERF` |
| Runtime leases | `NONE` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Единый список воспроизводимых workload, метрик, последних чисел, hardware/build provenance и непокрытых профилей.

## Read-only context — do not broaden

- `docs/M52_OPERATOR_PERFORMANCE_NOTES.md`
- `docs/milestones/M91.md`
- `crates/mc-test-harness/tests/load_scenarios.rs`
- `docs/MEMORY.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/performance-baseline.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Извлечь имена non-ignored/ignored load tests
- Сверить metric names с текущим кодом
- Разделить functional evidence и performance evidence

## Non-goals

- Не оптимизировать
- Не смешивать загрязнённые и чистые host runs

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-test-harness --test load_scenarios -- --list`
- `Статическая сверка названий метрик`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T00-06.md

# T00-06 — Свернуть противоречивые/stale документы в один migration report

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W00` — Фиксация текущей истины |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `docs` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `none / campaign-level` |
| Depends on | `T00-01, T00-03, T00-04, T00-05` |
| Write locks | `COORD-DOCS` |
| Runtime leases | `NONE` |
| Required evidence | `audit/control` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Отчёт с приоритетом источников: current code/tests/runtime > docs/MEMORY > ACTIVE > review/WAL > old restart/ledger; список устаревших открытых пунктов, которые нельзя раздавать агентам без проверки.

## Read-only context — do not broaden

- `docs/MEMORY.md`
- `docs/playable/ACTIVE.md`
- `REVIEW_FEEDBACK.md`
- `.analysis/restart-checkpoint.md`
- `docs/VALIDATION_LEDGER.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/status-reconciliation.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Сравнить даты и конкретные anchors
- Пометить P46/Lua restart checkpoint как условный, пока git diff не подтвердит

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `audit/control`

## Required validation

- `Проверить ссылки/пути`
- `Ни один канонический документ не переписывать в этой карточке`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-01.md

# T01-01 — Добавить компактный core-gate manifest: scenario → ledger rows → evidence legs

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `Q1, Q2, Q3` |
| Depends on | `T00-03, T00-04` |
| Write locks | `RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, oracle, real-client-agent, replay/negative` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Машиночитаемая проверка запрещает broad-row pass без перечисленных focused phases и явно фиксирует runtime/oracle/client/perf/persistence legs.

## Read-only context — do not broaden

- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/tests/real_client_manifest.rs`
- `docs/DEFINITION_OF_DONE.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/tests/real_client_manifest.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Найти текущие DTO schema anchors
- Добавить один минимальный manifest/validator, не новый framework

## Non-goals

- Не менять ledger statuses
- Не добавлять универсальный workflow engine

## Required evidence legs

- `unit`
- `wire`
- `oracle`
- `real-client-agent`
- `replay/negative`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.

## Required validation

- `cargo test -p mc-test-harness --test real_client_manifest`
- `cargo test -p mc-test-harness replay`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-02.md

# T01-02 — Разрезать broad block/fluid real-client gate на независимые focused phases

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B1, B2, B3, B4, Q2` |
| Depends on | `T00-03` |
| Write locks | `CLIENT-JAVA` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Каждая фаза имеет отдельный ID, exact observations и fail-closed result; umbrella ID только агрегирует уже зелёные фазы.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94BlocksFluidsFarmingDropsScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94BlocksFluidsFarmingDropsScenarioTest.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94BlocksFluidsFarmingDropsScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94BlocksFluidsFarmingDropsScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Сначала перечислить текущие broad blockers
- Не добавлять серверные debug команды как success path

## Non-goals

- Не чинить серверное поведение
- Не запускать длинный client gate

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94BlocksFluidsFarmingDropsScenarioTest*"`
- `cargo test -p mc-test-harness --test real_client_manifest`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-03.md

# T01-03 — Разрезать inventory/crafting/container real-client gate на focused phases

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I1, I2, K1, Q2` |
| Depends on | `T00-03` |
| Write locks | `CLIENT-JAVA` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Отдельные фазы для recipe-book/inventory craft, table craft, chest transfer, furnace-family UI, malformed rejection и reopen conservation.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenarioTest.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Использовать существующие push-driven container waits
- Каждая success фаза должна иметь dominant rejection boundary

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94InventoryCraftingScenarioTest*"`
- `cargo test -p mc-test-harness --test real_client_manifest`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-04.md

# T01-04 — Добавить restart invariant snapshot и строгую cross-phase валидацию

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S1, Q2, Q3` |
| Depends on | `T01-01` |
| Write locks | `RUNNER, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, real-client-agent, replay/negative, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Before/after фазы сравнивают typed invariant snapshot: player/inventory/world markers/container/entity/time as declared by scenario; missing field fails, timeout only fails.

## Read-only context — do not broaden

- `tools/real-client-agent-driver.py`
- `crates/mc-test-harness/tests/real_client_agent_driver.rs`
- `crates/mc-test-harness/tests/real_client_manifest.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `tools/real-client-agent-driver.py`
- `crates/mc-test-harness/tests/real_client_agent_driver.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Переиспользовать существующий marker/provenance contract
- Схема должна быть bounded и versioned

## Non-goals

- Не создавать общий database артефактов

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `replay/negative`
- `persistence`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `python -m py_compile tools/real-client-agent-driver.py`
- `cargo test -p mc-test-harness --test real_client_agent_driver`
- `cargo test -p mc-test-harness --test real_client_manifest`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-05.md

# T01-05 — Добавить vanilla oracle для block transaction/rejection/resync

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B1, B2, B3, Q1` |
| Depends on | `T00-04, T01-01` |
| Write locks | `ORACLE, RUST-HARNESS` |
| Runtime leases | `ORACLE-RIG` |
| Required evidence | `unit, wire, oracle` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Checked manifest сравнивает vanilla и Solaris для break/place/use, occupied target, out-of-reach/early-stop rejection и authoritative resync order.

## Read-only context — do not broaden

- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/src/bin/wire_probe.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/src/replay.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Сначала зафиксировать packet/layout facts локальным oracle
- Нормализовать только шум, не результат

## Non-goals

- Не копировать ожидаемые константы из Solaris implementation

## Required evidence legs

- `unit`
- `wire`
- `oracle`
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.

## Required validation

- `cargo test -p mc-test-harness --test parity_oracle <new_filter> -- --ignored --nocapture`
- `cargo test -p mc-test-harness replay`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T01-06.md

# T01-06 — Добавить vanilla oracle для inventory/crafting/container state machine

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W01` — Evidence laboratory: real-client/oracle/replay |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I1, I2, K1, Q1` |
| Depends on | `T00-04, T01-01` |
| Write locks | `ORACLE, RUST-HARNESS` |
| Runtime leases | `ORACLE-RIG` |
| Required evidence | `unit, wire, oracle` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Checked manifest сравнивает state_id/cursor/slots для craft, quick-move, stale click и close/reopen conservation.

## Read-only context — do not broaden

- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-protocol/src/packets/play.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/src/replay.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Ограничить первый slice chest + crafting table
- Furnace расширить отдельной follow-up карточкой при необходимости

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `oracle`
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.

## Required validation

- `cargo test -p mc-test-harness --test parity_oracle <new_filter> -- --ignored --nocapture`
- `cargo test -p mc-test-harness replay`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-01.md

# T02-01 — Закрыть B4: воспроизводимый real-client water/swim feel gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B4, Q2` |
| Depends on | `T01-02` |
| Write locks | `CLIENT-JAVA, RUNNER` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Fresh client proves source-water entry, ascent, dive, swim pose, fluid height, camera/eye transition, air loss/recovery and no correction/disconnect. Server edit only after reproducible red.

## Read-only context — do not broaden

- `docs/playable/ACTIVE.md`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94M40M41RouteScenario.java`
- `crates/mc-net/src/play/movement.rs`
- `crates/mc-net/src/play/player_breathing.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94M40M41RouteScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94M40M41RouteScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse deep-water MCP observations
- Record any remaining subjective-only item separately

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94M40M41RouteScenarioTest*"`
- `shortest exact real-client scenario`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-02.md

# T02-02 — Закрыть P4: полный respawn bundle + restart/rejoin evidence

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `P4, G4, S1, Q2` |
| Depends on | `T01-04` |
| Write locks | `CLIENT-JAVA, RUST-NET-SESSION` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Death screen → respawn → health/abilities/default spawn/chunk view/inventory according to mode → save/restart/rejoin without stale dead state.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94EntitiesCombatDeathRespawnScenario.java`
- `crates/mc-net/src/play/session/session_lifecycle.rs`
- `crates/mc-net/src/play/spawn.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94EntitiesCombatDeathRespawnScenario.java`
- `crates/mc-net/src/play/session/session_lifecycle.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Сначала rerun existing death/respawn scenario
- Изменять только точный missing packet/state

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `persistence`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-net respawn`
- `cargo test -p mc-test-harness --test block_edit survival_lifecycle`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94EntitiesCombatDeathRespawnScenarioTest*"`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-03.md

# T02-03 — Заменить small full-cube fallback на точный common sturdy-face contract

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B1, B2` |
| Depends on | `T01-05` |
| Write locks | `RUST-DATA, RUST-NET-BLOCKS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Common vanilla supports (full blocks plus representative irregular faces) place torches/attachments correctly; known partial/unsturdy faces reject and resync.

## Read-only context — do not broaden

- `crates/mc-data/src/block_facts.rs`
- `crates/mc-net/src/play/block_placement.rs`
- `crates/mc-net/src/play/block_placement_support_tests.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-data/src/block_facts.rs`
- `crates/mc-net/src/play/block_placement.rs`
- `crates/mc-net/src/play/block_placement_support_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Derive table from local 26.1.2 data/decompile, not wiki memory
- Start with torch/sign/door supports used in survival

## Non-goals

- Не реализовывать весь block behavior registry за один slice

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-data block_facts`
- `cargo test -p mc-net block_placement_support`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-04.md

# T02-04 — Закрыть stair neighbour-shape recomputation real-path proof

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B1` |
| Depends on | `T02-03` |
| Write locks | `RUST-NET-BLOCKS, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Placing/removing adjacent stairs updates both authoritative states, wire updates and restart state; stale dependency rejects atomically.

## Read-only context — do not broaden

- `crates/mc-net/src/play/block_placement.rs`
- `crates/mc-net/src/play/block_placement_support_tests.rs`
- `crates/mc-test-harness/tests/block_edit/stations_and_placement.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/block_placement.rs`
- `crates/mc-net/src/play/block_placement_support_tests.rs`
- `crates/mc-test-harness/tests/block_edit/stations_and_placement.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use existing corner-shape unit table
- Add one raw-TCP vertical path

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-net stair`
- `cargo test -p mc-test-harness --test block_edit stair`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-05.md

# T02-05 — Доказать door/trapdoor state convergence для двух клиентов

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B2, S2` |
| Depends on | `T01-02` |
| Write locks | `RUST-NET-BLOCKS, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Actor toggles earned door/trapdoor; observer sees exact open/facing/half state; rapid stale retry cannot diverge halves or consume item.

## Read-only context — do not broaden

- `crates/mc-net/src/play/toggles.rs`
- `crates/mc-net/src/play/block_wire.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/toggles.rs`
- `crates/mc-net/src/play/tests.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse two-client block marker pattern
- Separate door and trapdoor if production edits exceed budget

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `concurrency`

## Required validation

- `cargo test -p mc-net toggle`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`
- `focused two-client client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-06.md

# T02-06 — Закрыть scheduled fluid spread + save/restart real-client path

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B3, S1, Q2` |
| Depends on | `T01-02, T01-04` |
| Write locks | `RUST-NET-BLOCKS, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Client places source, observes delayed spread, clean restart, observes persisted source/scheduled continuation or settled exact state; no duplicate tick.

## Read-only context — do not broaden

- `crates/mc-net/src/play/fluids.rs`
- `crates/mc-net/src/play/scheduled_blocks.rs`
- `crates/mc-test-harness/tests/block_edit/fluid_scheduling.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94WaterBucketScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/block_edit/fluid_scheduling.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94WaterBucketScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94WaterBucketScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Prefer evidence-only if runtime already correct
- Include lava-water representative as separate phase only if cheap

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `persistence`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-test-harness --test block_edit fluid_scheduling`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94WaterBucketScenarioTest*"`
- `focused restart gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-07.md

# T02-07 — Зафиксировать representative movement boundary matrix

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B5` |
| Depends on | `T02-01` |
| Write locks | `RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Table-driven proof for step-up, long fall, crouch edge, swim/crouch body, powder snow/leather boots and non-finite rejection, using one authoritative geometry contract.

## Read-only context — do not broaden

- `crates/mc-net/src/play/movement.rs`
- `crates/mc-net/src/play/movement_tests.rs`
- `crates/mc-net/src/play/session/player_pose_authority.rs`
- `crates/mc-physics/src/lib.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/movement_tests.rs`
- `crates/mc-physics/src/lib.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Audit for duplicated height/shape constants
- Add tests before changing behavior

## Non-goals

- Не строить общий anti-cheat subsystem

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-physics`
- `cargo test -p mc-net movement`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T02-08.md

# T02-08 — Закрыть chunk visibility/rejoin без ghost chunks

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W02` — Обычная играбельность и client-visible блокеры |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `C1, Q2` |
| Depends on | `T01-04` |
| Write locks | `RUST-NET-CHUNK, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Move across view boundary, disconnect during/after prepare, reconnect: exact loaded set, no stale entity/block view, all required chunks eventually visible.

## Read-only context — do not broaden

- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-net/src/play/session/chunk_view_authority.rs`
- `crates/mc-test-harness/tests/chunk_stream.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94SaveRestartVisibilityScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/chunk_stream.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94SaveRestartVisibilityScenario.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- First run existing join/rejoin path
- Only edit server if exact ghost/missing chunk reproduces

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-test-harness --test chunk_stream`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94SaveRestartVisibilityScenarioTest*"`
- `focused real-client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-01.md

# T03-01 — Window-0 cursor rejection/resync conservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I1, Q3` |
| Depends on | `T01-03, T01-06` |
| Write locks | `RUST-NET-CONTAINERS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, replay/negative` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Malformed/stale click cannot move slot/cursor; exact state id and authoritative cursor/slots are resent; next valid click succeeds.

## Read-only context — do not broaden

- `crates/mc-net/src/play/inventory.rs`
- `crates/mc-net/src/play/tests/inventory_and_survival.rs`
- `crates/mc-test-harness/tests/block_edit/inventory_clicks.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/inventory.rs`
- `crates/mc-test-harness/tests/block_edit/inventory_clicks.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse existing malformed carried-item regression
- Add recovery action after rejection

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `replay/negative`

## Required validation

- `cargo test -p mc-net inventory`
- `cargo test -p mc-test-harness --test block_edit inventory_clicks`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-02.md

# T03-02 — Crafting-table max-craft и cursor conservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I1, I2` |
| Depends on | `T03-01` |
| Write locks | `RUST-NET-CONTAINERS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Normal take, shift-click/max craft, full inventory and stale state conserve inputs/output/cursor exactly; no partial craft.

## Read-only context — do not broaden

- `crates/mc-net/src/play/containers/crafting.rs`
- `crates/mc-net/src/play/containers/crafting_tests.rs`
- `crates/mc-test-harness/tests/block_edit/crafting_table.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/containers/crafting.rs`
- `crates/mc-net/src/play/containers/crafting_tests.rs`
- `crates/mc-test-harness/tests/block_edit/crafting_table.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Keep recipe authority in mc-data
- One recipe family first

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-net crafting`
- `cargo test -p mc-test-harness --test block_edit crafting_table`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-03.md

# T03-03 — Recipe-book discovery/window sync real-client gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I2, Q2` |
| Depends on | `T01-03, T03-02` |
| Write locks | `CLIENT-JAVA, RUST-NET-CONTAINERS` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Fresh client sees expected recipe display IDs, uses inventory and table recipe placement, state remains coherent after close/reopen.

## Read-only context — do not broaden

- `crates/mc-net/src/play/recipes.rs`
- `crates/mc-protocol/src/packets/play.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Read recipe book through existing MCP tool
- Server edit only after exact missing/incorrect packet is observed

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-net recipes`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94InventoryCraftingScenarioTest*"`
- `focused client phase`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-04.md

# T03-04 — Furnace/smoker/blast/campfire recipe execution representative client path

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P1` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `I2, K1, E2` |
| Depends on | `T03-03` |
| Write locks | `RUST-NET-CONTAINERS, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

At least one recipe per furnace family plus campfire uses sidecar facts, correct duration/output/fuel, close/reopen conservation and client-visible progress.

## Read-only context — do not broaden

- `crates/mc-net/src/play/containers/furnace.rs`
- `crates/mc-net/src/play/recipes.rs`
- `crates/mc-test-harness/tests/block_edit/furnaces.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/block_edit/furnaces.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Split campfire if write set collides
- Do not duplicate recipes in Rust

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-test-harness --test block_edit furnaces`
- `focused client phases`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-05.md

# T03-05 — Chest max-stack metadata + malformed edge matrix

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `K1, Q3` |
| Depends on | `T03-01` |
| Write locks | `RUST-NET-CONTAINERS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, replay/negative` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Pickup/quick-move/drag obey item max stack; impossible prediction/stale state fails closed and authoritative storage/cursor remains unchanged.

## Read-only context — do not broaden

- `crates/mc-net/src/play/containers/chest.rs`
- `crates/mc-net/src/play/containers/quickcraft.rs`
- `crates/mc-test-harness/tests/block_edit/chests_and_hoppers.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/containers/chest.rs`
- `crates/mc-test-harness/tests/block_edit/chests_and_hoppers.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use mc-data item facts, no local max=64 copy
- Cover one stack-1 and one stack-16 item

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `replay/negative`

## Required validation

- `cargo test -p mc-net chest`
- `cargo test -p mc-test-harness --test block_edit chests_and_hoppers`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-06.md

# T03-06 — Two-client shared chest concurrent-click real-client gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `K1, S2, Q2` |
| Depends on | `T03-05` |
| Write locks | `CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Two clients race deposit/withdraw; exactly one authoritative outcome per item, both state IDs converge, close/reopen shows same contents.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse playable-31 marker and extend with an actual race
- No operator item grants in final evidence

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `concurrency`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`
- `focused two-client client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-07.md

# T03-07 — Two-client shared furnace concurrent-click real-client gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P1` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `K1, S2, Q2` |
| Depends on | `T03-04` |
| Write locks | `CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Live peer updates, stale click rejection and output contention converge for both clients without duplication/loss.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenarioTest.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/M94InventoryCraftingScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use existing two-protocol-client stale-click fixtures as server baseline
- Client scenario only adds observations/actions

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `concurrency`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*M94InventoryCraftingScenarioTest*"`
- `focused two-client client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T03-08.md

# T03-08 — Two-client container save/restart convergence

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W03` — Inventory, crafting и containers |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `K1, S1, S2` |
| Depends on | `T01-04, T03-06` |
| Write locks | `CLIENT-JAVA, RUNNER` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, persistence, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

After concurrent chest/furnace activity, save/restart/rejoin yields exact backing storage and both clients see it; cursor/open-window transient state is explicitly classified.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94SaveRestartVisibilityScenario.java`
- `tools/real-client-agent-driver.py`
- `crates/mc-test-harness/tests/block_edit/persistence.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/M94SaveRestartVisibilityScenario.java`
- `tools/real-client-agent-driver.py`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse restart invariant snapshot from T01-04
- One container family may close this task; second becomes follow-up

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `concurrency`

## Required validation

- `python -m py_compile tools/real-client-agent-driver.py`
- `cargo test -p mc-test-harness --test block_edit persistence`
- `focused restart client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-01.md

# T04-01 — Dropped item merge: exact identity/count/age/version conservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L2` |
| Depends on | `T00-06` |
| Write locks | `RUST-ENTITY, RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Compatible nearby stacks merge once under owner CAS; incompatible/full stacks do not; tracker/publication removes old identity and publishes survivor.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/pickups.rs`
- `crates/mc-entity/src/runtime_26_1_2/transaction.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/session/pickups.rs`
- `crates/mc-entity/src/runtime_26_1_2/transaction.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Verify whether merge is absent or only unproved
- No full-store scans

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-entity runtime_26_1_2`
- `cargo test -p mc-net pickup`
- `cargo test -p mc-test-harness --test block_edit survival_lifecycle`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-02.md

# T04-02 — Partial pickup и overflow conservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L2, I1` |
| Depends on | `T04-01, T03-05` |
| Write locks | `RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Full/near-full inventory picks only admissible count; remainder keeps identity/count; disconnect/stale owner cannot partially commit.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/pickups.rs`
- `crates/mc-net/src/play/session/player_state.rs`
- `crates/mc-test-harness/tests/block_edit/survival_inventory.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/session/pickups.rs`
- `crates/mc-test-harness/tests/block_edit/survival_inventory.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use exact inventory transaction path
- Include stack-limit metadata

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-net pickup`
- `cargo test -p mc-test-harness --test block_edit survival_inventory`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-03.md

# T04-03 — Item despawn deadline + restart proof

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L2, S1` |
| Depends on | `T00-06` |
| Write locks | `RUST-NET-SESSION, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Persisted spawn_tick reconstructs exact deadline; before/after restart item neither despawns early nor lives forever; stale index IDs are bounded.

## Read-only context — do not broaden

- `crates/mc-net/src/play/simulation.rs`
- `crates/mc-net/src/play/persistence.rs`
- `crates/mc-test-harness/tests/persistence_inventory.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/persistence_inventory.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Current architecture memory says index exists: prefer verification-only
- Edit production only on red

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`

## Required validation

- `cargo test -p mc-net item_despawn`
- `cargo test -p mc-test-harness --test persistence_inventory`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-04.md

# T04-04 — Two-client shared pickup contention real-client gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L2, S2, Q2` |
| Depends on | `T04-02` |
| Write locks | `CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Both clients race one visible item/XP reward; exactly one credit, both see removal, reconnect/reopen inventory confirms conservation.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Extend playable-30/38 rather than create duplicate framework
- Separate XP if item path is already large

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `concurrency`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`
- `focused two-client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-05.md

# T04-05 — Loot executor: random count ranges и multiple rolls core slice

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L1` |
| Depends on | `T00-04` |
| Write locks | `RUST-DATA` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

One representative block and entity table execute bounded deterministic seeded ranges/rolls matching local vanilla oracle; unsupported shapes remain explicit.

## Read-only context — do not broaden

- `crates/mc-data/src/loot.rs`
- `crates/mc-data/src/loot/entity_26_1_2/evaluate.rs`
- `crates/mc-data/tests/loot_context.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-data/src/loot.rs`
- `crates/mc-data/src/loot/entity_26_1_2/evaluate.rs`
- `crates/mc-data/tests/loot_context.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Start from current compile/evaluate model
- Seed is test input, not production deterministic cheat

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-data loot`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-06.md

# T04-06 — Loot context vertical slice: Fortune/Silk/Looting/burning

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `L1, G1` |
| Depends on | `T04-05` |
| Write locks | `RUST-DATA, RUST-ENTITY` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Exactly one block tool context and one entity killer context use authoritative equipment/effect facts and match oracle output; unsupported combinations fail explicitly.

## Read-only context — do not broaden

- `crates/mc-data/src/loot/context.rs`
- `crates/mc-data/src/loot/entity_26_1_2/evaluate.rs`
- `crates/mc-entity/src/equipment_26_1_2/drops.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-data/src/loot/context.rs`
- `crates/mc-data/src/loot/entity_26_1_2/evaluate.rs`
- `crates/mc-data/tests/loot_context.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Do not expose live ECS handles to mc-data
- Pass immutable bounded context DTO

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-data loot_context`
- `cargo test -p mc-entity equipment`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-07.md

# T04-07 — Renewable wheat → bread lifecycle real-client gate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `F1, I2, Q2` |
| Depends on | `T03-03` |
| Write locks | `CLIENT-JAVA, RUST-NET-BLOCKS` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

No-debug client obtains seeds, plants, grows/bonemeals or waits on exact tick event, harvests mature crop, replants and crafts/eats bread.

## Read-only context — do not broaden

- `crates/mc-test-harness/tests/block_edit/wheat_seed_source.rs`
- `crates/mc-test-harness/tests/block_edit/wheat_harvest.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `client-mod/solaris-client-agent/java-agent/src/test/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenarioTest.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse playable-43
- No elapsed-time success; wait for state changes

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cargo test -p mc-test-harness --test block_edit wheat`
- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`
- `focused client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T04-08.md

# T04-08 — Sugar cane/cactus support cascade representative parity

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W04` — Drops, loot, farming и renewable progression |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `F3` |
| Depends on | `T01-05` |
| Write locks | `RUST-NET-BLOCKS, RUST-HARNESS` |
| Runtime leases | `ORACLE-RIG` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Place/grow/remove support and side-neighbor paths cascade correct states/drops; special/bucket mutation revalidates support; one oracle-backed family table.

## Read-only context — do not broaden

- `crates/mc-net/src/play/plants.rs`
- `crates/mc-test-harness/tests/block_edit/vertical_plant_growth.rs`
- `crates/mc-test-harness/tests/block_edit/plant_lifecycle.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/plants.rs`
- `crates/mc-test-harness/tests/block_edit/vertical_plant_growth.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Cactus damage/collision is a separate task if not already local
- No full plant rewrite

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-net plant`
- `cargo test -p mc-test-harness --test block_edit vertical_plant_growth`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-01.md

# T05-01 — Common damage-source matrix + exact rejection boundaries

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G1` |
| Depends on | `T00-06` |
| Write locks | `RUST-ENTITY, RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Melee, projectile, fall, fire/lava, drowning, suffocation and starvation share authoritative reductions/immunities; non-finite/unsupported paths fail closed.

## Read-only context — do not broaden

- `crates/mc-entity/src/living_26_1_2/damage.rs`
- `crates/mc-entity/src/living_26_1_2/tests.rs`
- `crates/mc-net/src/play/player_damage_adapter.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-entity/src/living_26_1_2/tests.rs`
- `crates/mc-net/src/play/player_damage_adapter.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- First audit current supported matrix
- Implement only missing common source

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-entity living_26_1_2`
- `cargo test -p mc-net player_damage`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-02.md

# T05-02 — Arrow lifecycle: spawn/flight/hit/stick/pickup representative oracle

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G2` |
| Depends on | `T05-01` |
| Write locks | `RUST-ENTITY, RUST-NET-SESSION` |
| Runtime leases | `ORACLE-RIG` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Ordinary bow arrow matches local 26.1.2 gravity/drag/hit order and visible metadata; stale target/removal/pickup cannot duplicate.

## Read-only context — do not broaden

- `crates/mc-entity/src/projectile_26_1_2/arrow.rs`
- `crates/mc-entity/src/projectile_26_1_2/arrow_tests.rs`
- `crates/mc-net/src/play/session/projectiles.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-entity/src/projectile_26_1_2/arrow.rs`
- `crates/mc-entity/src/projectile_26_1_2/arrow_tests.rs`
- `crates/mc-net/src/play/session/projectiles_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Ordinary arrow only
- Tipped/spectral/crossbow deferred

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-entity projectile_26_1_2`
- `cargo test -p mc-net projectiles`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-03.md

# T05-03 — Shield angle/timing + axe-disable representative path

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G3` |
| Depends on | `T05-01` |
| Write locks | `RUST-NET-SESSION, RUST-ENTITY` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Front-angle timely block prevents damage and consumes durability; back/late hit does not; axe disable duration comes from oracle facts and publishes correct state.

## Read-only context — do not broaden

- `crates/mc-net/src/play/combat/player_actions.rs`
- `crates/mc-net/src/play/combat/player_damage.rs`
- `crates/mc-entity/src/equipment_26_1_2/durability.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/combat/player_actions.rs`
- `crates/mc-net/src/play/tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Keep exact source/item scope bounded
- No enchantment breadth

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-net shield`
- `cargo test -p mc-entity equipment`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-04.md

# T05-04 — Player death inventory/XP drop conservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P0` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G4, L2, S1` |
| Depends on | `T02-02, T04-02` |
| Write locks | `RUST-NET-SESSION, RUST-ENTITY` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

One authoritative death commit produces exact item/XP consequences once; keepInventory/mode rules explicit; respawn cannot reclaim stale inventory.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/player_state.rs`
- `crates/mc-net/src/play/session/entity_lifecycle.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/session/player_state.rs`
- `crates/mc-test-harness/tests/block_edit/survival_lifecycle.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use one compound transaction/outbox boundary
- Do not infer killer attribution if facts absent

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`

## Required validation

- `cargo test -p mc-net death`
- `cargo test -p mc-test-harness --test block_edit survival_lifecycle`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-05.md

# T05-05 — Two-client contested death drops + restart

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P0` |
| Route | `playable` |
| Kind | `evidence-or-fix` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G4, S1, S2, Q2` |
| Depends on | `T05-04, T04-04, T01-04` |
| Write locks | `CLIENT-JAVA, RUNNER` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, real-client-agent, persistence, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Observer sees death/drops, both race pickup, exactly one credit, victim respawns, save/restart preserves final inventories and no dropped duplicate.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `tools/real-client-agent-driver.py`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `tools/real-client-agent-driver.py`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse playable-37/38 and restart snapshot
- Separate item and XP phases if needed

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `real-client-agent`
- `persistence`
- `concurrency`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `cd client-mod/solaris-client-agent && ./gradlew :java-agent:test --tests "*PlayableRealClientLoopScenarioTest*"`
- `python -m py_compile tools/real-client-agent-driver.py`
- `focused restart gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-06.md

# T05-06 — Entity snapshot version fence across owner/wire/persistence

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `N1, O2` |
| Depends on | `T00-06` |
| Write locks | `RUST-ENTITY, RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

One version-fenced snapshot is used by wire/persistence/collision/damage/equipment; stale publication/CAS/death/restart tests prove no side-map authority wins.

## Read-only context — do not broaden

- `crates/mc-entity/src/runtime_26_1_2/state.rs`
- `crates/mc-entity/src/regional.rs`
- `crates/mc-net/src/play/session/entity_owner.rs`
- `crates/mc-net/src/play/wire_entities.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-entity/src/runtime_26_1_2/tests.rs`
- `crates/mc-net/src/play/wire_entities_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Inventory current side maps first
- Delete duplicate only after callers move

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-entity runtime_26_1_2`
- `cargo test -p mc-net wire_entities`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-07.md

# T05-07 — Entity spawn/despawn cap + restart invariants

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P1` |
| Route | `playable` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `N1, S1` |
| Depends on | `T05-06` |
| Write locks | `RUST-NET-SESSION, RUST-ENTITY` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Natural spawn respects active chunks/caps; unload/despawn/restart restores exactly allowed entities without duplicate materialization.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/entity_lifecycle.rs`
- `crates/mc-net/src/play/session/herd_spawn_authority.rs`
- `crates/mc-net/src/play/persistence.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/session/entity_lifecycle.rs`
- `crates/mc-net/src/play/session/tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Representative passive + hostile class
- No species-wide AI expansion

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`

## Required validation

- `cargo test -p mc-net spawn`
- `cargo test -p mc-entity regional`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T05-08.md

# T05-08 — Representative species AI/pathing parity table + client proof

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W05` — Combat, death и entity authority |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `N1` |
| Depends on | `T05-07` |
| Write locks | `RUST-ENTITY, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Zombie, sheep/cow and fish each have oracle-backed default goal/material predicates and one bounded client motion/interaction proof under budget.

## Read-only context — do not broaden

- `crates/mc-entity/src/ai_core_26_1_2/goal_policy.rs`
- `crates/mc-entity/src/navigation_26_1_2/`
- `crates/mc-net/src/play/session/entity_goal_defaults.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-entity/src/ai_core_26_1_2/goal_policy_tests.rs`
- `crates/mc-net/src/play/session/pathing_tests.rs`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Do not chase every vanilla goal
- No full species tree

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`

## Required validation

- `cargo test -p mc-entity ai_core_26_1_2`
- `cargo test -p mc-net pathing`
- `focused client gate`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-01.md

# T06-01 — Fault injection: campfire world + entity/drop journal outcome

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `E2, S1, O2` |
| Depends on | `T00-06` |
| Write locks | `RUST-NET-ROOT, RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Crashes/failpoints before/after append, durable watermark, publication and compaction recover exactly one cooked output/state, never zero+lost or duplicate.

## Read-only context — do not broaden

- `crates/mc-net/src/play/campfire.rs`
- `crates/mc-net/src/play/world_journal.rs`
- `crates/mc-net/src/play/persistence.rs`
- `crates/mc-test-harness/tests/block_edit/campfire.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/block_edit/campfire.rs`
- `crates/mc-net/src/play/world_journal.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Prefer test failpoint hooks scoped to harness
- No sleeps/polling

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-test-harness --test block_edit campfire`
- `cargo test -p mc-net world_journal`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-02.md

# T06-02 — Fault injection: chained/simultaneous TNT world/entity outcome

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `G1, S1, O2` |
| Depends on | `T06-01` |
| Write locks | `RUST-NET-ROOT, RUST-WORLD, RUST-ENTITY` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Restart at each durable boundary yields at-most-once explosion/removal and exact block/entity damage result; chained TNT cannot replay twice.

## Read-only context — do not broaden

- `crates/mc-net/src/play/explosions.rs`
- `crates/mc-net/src/play/session/explosion_authority.rs`
- `crates/mc-net/src/play/world_journal.rs`
- `crates/mc-test-harness/tests/block_edit/explosions.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/block_edit/explosions.rs`
- `crates/mc-net/src/play/session/explosion_authority.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- One deterministic small fixture
- No broad blast parity

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-test-harness --test block_edit explosions`
- `cargo test -p mc-net explosion`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-03.md

# T06-03 — Fault injection: cross-region hopper compound commit

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `A1, K1, S1, O2` |
| Depends on | `T06-01` |
| Write locks | `RUST-NET-ROOT, RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Source/destination/cooldown commit atomically across region boundary; crash/retry cannot duplicate or lose item; stale version rejects whole mutation.

## Read-only context — do not broaden

- `crates/mc-net/src/play/simulation/regional_mutation.rs`
- `crates/mc-net/src/play/containers.rs`
- `crates/mc-test-harness/tests/block_edit/chests_and_hoppers.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/block_edit/chests_and_hoppers.rs`
- `crates/mc-net/src/play/simulation/regional_mutation.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Single-item transfer first
- No hopper minecart

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-test-harness --test block_edit chests_and_hoppers`
- `cargo test -p mc-net regional_mutation`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-04.md

# T06-04 — Shutdown while journal/checkpoint outcome is unknown

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S1, O2` |
| Depends on | `T06-01, T06-03` |
| Write locks | `RUST-NET-ROOT, RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Graceful stop drains admitted owner work, resolves/records unknown journal outcome, performs final save in correct order and restart replays exactly once.

## Read-only context — do not broaden

- `crates/mc-net/src/server.rs`
- `crates/mc-net/src/play/world_journal.rs`
- `crates/mc-net/src/play/persistence.rs`
- `crates/mc-server/tests/play.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-server/tests/play.rs`
- `crates/mc-net/src/server.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use exact notifications/failpoints
- Timeout only fails

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-server --test play shutdown`
- `cargo test -p mc-net persistence`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-05.md

# T06-05 — Real Anvil compression corpus + unknown NBT preservation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P1` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `W2, S1` |
| Depends on | `T00-04` |
| Write locks | `RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Repo-safe synthetic metadata plus owner-local real-region corpus covers zlib/gzip/uncompressed/LZ4, unknown compression fail-closed and unknown root extras through edit/flush/reopen.

## Read-only context — do not broaden

- `crates/mc-world/src/anvil/region.rs`
- `crates/mc-world/src/anvil/chunk_nbt.rs`
- `crates/mc-world/src/storage.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-world/src/anvil/region.rs`
- `crates/mc-world/src/anvil/chunk_nbt.rs`
- `crates/mc-world/src/storage/dirty_flush_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Mojang bytes stay ignored
- No Windows atomic replace in this task

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`

## Required validation

- `cargo test -p mc-world anvil`
- `cargo test -p mc-world dirty_flush`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-06.md

# T06-06 — Disconnect during pending chunk prepare/load/generate

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `C1, S2, O2` |
| Depends on | `T02-08` |
| Write locks | `RUST-NET-CHUNK, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Disconnect cancels/publication-fences pending work; permits/retries/cache entries drain; reconnect cannot receive stale chunks from old generation.

## Read-only context — do not broaden

- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-net/src/chunk_pipeline.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/chunk_stream_world_handle_tests.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Cover already-running and queued jobs separately
- No arbitrary wait

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-net chunk_stream`
- `cargo test -p mc-test-harness --test load_scenarios <filter> -- --nocapture`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-07.md

# T06-07 — Natural TCP slow-reader fairness, shedding и recovery

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S2, O2, O3` |
| Depends on | `T06-06` |
| Write locks | `RUST-NET-SESSION, RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Paused/slow real socket raises bounded pressure, healthy client keeps state-bearing progress, reliable drops remain zero or session closes by explicit policy, retry tasks drain.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/outbound.rs`
- `crates/mc-net/src/play/session/outbound_backpressure_tests.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/tests/load_scenarios.rs`
- `crates/mc-net/src/play/session/outbound_backpressure_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use natural socket behavior, not only synthetic counters
- Separate cosmetic drops

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-net outbound_backpressure`
- `cargo test -p mc-test-harness --test load_scenarios paused_reader -- --nocapture`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T06-08.md

# T06-08 — External online-mode paid-client qualification

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W06` — Durability, multiplayer pressure и online-mode |
| Priority | `P0` |
| Route | `parity` |
| Kind | `external-gate` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `P2, P3, O4` |
| Depends on | `T00-02` |
| Write locks | `EXTERNAL` |
| Runtime leases | `CLIENT-RIG, PAID-AUTH` |
| Required evidence | `real-client-agent, security/auth` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T00-01, T00-02`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Paid 26.1.2 client completes RSA/AES/session auth, signed properties, compression transition, reconnect and bounded load against public-bind-safe config; exact failures recorded.

## Read-only context — do not broaden

- `crates/mc-net/src/login.rs`
- `crates/mc-net/src/session_auth.rs`
- `crates/mc-server/tests/login.rs`
- `<owner active online-mode config path — fill before claim>`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/online-mode-qualification.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Preflight paid account, network and active config without exposing credentials
- Use the real session endpoint; redact access tokens, profile properties and server hash inputs
- Allocate a unique world/run directory and port range

## Non-goals

- Не хранить токены/профильные данные в Git
- Не считать offline-mode эквивалентом

## Required evidence legs

- `real-client-agent`
- `security/auth`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.

## Required validation

- `Owner/agent real-client run`
- `Server logs checked for disconnect/error mapping`
- `No source edit unless reproducible bug`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-01.md

# T07-01 — Заморозить low/balanced/high profile configs и budgets

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `docs-and-config` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `O1, O3` |
| Depends on | `T00-05` |
| Write locks | `PERF, COORD-DOCS` |
| Runtime leases | `NONE` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Три воспроизводимых профиля с CPU/memory/VD/client/entity/workload limits, required metrics, build mode and pass/degraded rules; без подгонки после результата.

## Read-only context — do not broaden

- `docs/milestones/M91.md`
- `docs/M52_OPERATOR_PERFORMANCE_NOTES.md`
- `crates/mc-net/src/control_plane.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `docs/performance/CORE_PROFILE_MATRIX.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use existing >18 TPS/VD8 target and current metric names
- Freeze exact p95/p99/latency budgets from clean baseline

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Статическая проверка config keys against code`
- `Markdown links/path check`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-02.md

# T07-02 — Versioned performance result schema + provenance validator

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `O1, O2, O3` |
| Depends on | `T07-01` |
| Write locks | `RUST-HARNESS` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Every run records git/tree dirty, hardware/cgroup, build, profile, samples, percentiles, lock/queue/memory/chunk/save/outbound metrics and skipped evidence; malformed result fails closed.

## Read-only context — do not broaden

- `crates/mc-test-harness/tests/load_scenarios.rs`
- `crates/mc-test-harness/src/replay.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Extend existing core replay/result DTO if suitable
- No telemetry database

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-test-harness replay`
- `cargo test -p mc-test-harness --test load_scenarios --no-run`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-03.md

# T07-03 — Solo generated-world VD8 clean-host profile

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `W3, C1, C2, O1, O2` |
| Depends on | `T07-02, T06-05` |
| Write locks | `PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Fresh generate → first join → all 289 chunks → save → restart → warm join, with tick/chunk/light/fetch/lock/dirty/memory metrics and exact chunk coverage.

## Read-only context — do not broaden

- `docs/performance/CORE_PROFILE_MATRIX.md`
- `crates/mc-test-harness/tests/load_scenarios.rs`
- `crates/mc-test-harness/tests/chunk_stream.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/solo-generated-vd8/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Run once clean host and preserve full logs
- Do not edit during measurement

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator from T07-02`
- `Artifact completeness check`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-04.md

# T07-04 — Two-client ordinary survival responsiveness profile

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S2, O1, O2` |
| Depends on | `T07-02, T03-06, T04-04` |
| Write locks | `PERF, CLIENT-JAVA` |
| Runtime leases | `CLIENT-RIG, CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Two real clients gather/craft/container/combat while chunks stream and autosave; record action latency, tick/lock/queue/outbound and visible correctness.

## Read-only context — do not broaden

- `docs/performance/CORE_PROFILE_MATRIX.md`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/two-client-survival/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use existing focused playable phases
- No debug grants in measured segment

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator`
- `Both clients remain in_play`
- `No reliable command drops`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-05.md

# T07-05 — 20-client VD8 balanced profile

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `O1, O2, O3, S2` |
| Depends on | `T07-02, T06-07` |
| Write locks | `PERF, RUST-HARNESS` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

20 active clients at VD8 execute mixed deterministic workload; >18 TPS target or exact blockers, with p50/p95/p99/max, first/full chunk, lock, queues, memory, saves and outbound.

## Read-only context — do not broaden

- `docs/performance/CORE_PROFILE_MATRIX.md`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/balanced-20-vd8/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Clean host
- At least one real client may observe if resources allow; protocol clients do load

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator`
- `Final save/restart state check`
- `No unbounded growth`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-06.md

# T07-06 — Dense entity/AI/physics profile

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `N1, O1, O2` |
| Depends on | `T07-02, T05-08` |
| Write locks | `PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Representative ordinary and overload populations quantify goal/physics/publication/death/explosion stages and client liveness; workload size and cohort budgets explicit.

## Read-only context — do not broaden

- `crates/mc-net/src/play/session/combat_load_tests.rs`
- `crates/mc-net/src/play/simulation/explosion_load_tests.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/dense-entities/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Reuse 5,132-cow and death/explosion gates as baselines
- Do not infer ordinary performance from synthetic overload only

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator`
- `Client liveness and reliable delivery`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-07.md

# T07-07 — Fluid/random/scheduled-block profile

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `B3, F1, A1, O1, O2` |
| Depends on | `T07-02, T02-06` |
| Write locks | `PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Ocean/fluid settle, crops/random ticks and 256-button scheduled workload report planning/apply/follow-up, owner responsiveness and no indefinite backlog.

## Read-only context — do not broaden

- `crates/mc-net/src/play/fluids.rs`
- `crates/mc-net/src/play/random_ticks.rs`
- `crates/mc-net/src/play/scheduled_blocks.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/fluid-random-scheduled/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use exact existing deterministic fixtures
- Separate contaminated host run

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator`
- `Backlog drains by observed completion`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T07-08.md

# T07-08 — Save/autosave/dirty-flush profile under live mutation

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W07` — Профилирование low/balanced/high |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `performance-run` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S1, O1, O2` |
| Depends on | `T07-02, T06-04` |
| Write locks | `PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Live mutations during periodic/full save quantify encode/install/fsync/lock waits, stable-region progress, dirty backlog and client responsiveness; restart validates state.

## Read-only context — do not broaden

- `crates/mc-world/src/storage/dirty_flush.rs`
- `crates/mc-net/src/server.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/perf/save-dirty-flush/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Include one stable and one continuously-mutated region
- No wall-clock success

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Profile validator`
- `Disk reopen invariant`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-01.md

# T08-01 — Ранжировать remaining WorldHandle locksites по measured impact

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `audit` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `O2` |
| Depends on | `T07-03, T07-05, T07-08` |
| Write locks | `RUST-NET-ROOT, PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Таблица production locksite → resident/disk role → call frequency → measured wait/hold → safe owner target; выбрать ровно один следующий cutover.

## Read-only context — do not broaden

- `crates/mc-net/src/server.rs`
- `crates/mc-net/src/play.rs`
- `crates/mc-net/src/play/simulation.rs`
- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-world/src/resident.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/world-handle-locksites.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use CodeGraph or bounded rg, not both broadly
- Correlate with T07 logs

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Independent read-only review of ranking`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-02.md

# T08-02 — Перевести один dominant resident mutation с global WorldHandle

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `O2` |
| Depends on | `T08-01` |
| Write locks | `RUST-NET-ROOT, RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T08-01`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Already-resident mutation takes only sorted affected regional owners; global storage coordinates disk/LRU only; CAS/publication/persistence behavior unchanged.

## Read-only context — do not broaden

- `crates/mc-world/src/resident.rs`
- `<exact top locksite from T08-01>`
- `docs/decisions/0004-staged-single-writer-simulation.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `<exact top locksite>`
- `crates/mc-world/src/resident.rs`
- `focused sibling tests`
- `docs/decisions/0004-staged-single-writer-simulation.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Task must be reissued with exact paths after T08-01
- RED lock-pressure/correctness test first

## Non-goals

- Не удалять WorldHandle целиком
- Не делать unrelated extraction

## Required evidence legs

- `unit`
- `wire`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Focused domain tests`
- `cargo test -p mc-world`
- `cargo test -p mc-net <domain>`
- `rerun exact workload`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-03.md

# T08-03 — Убрать measured chunk-stream global-lock bottleneck

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `C1, O2` |
| Depends on | `T08-01` |
| Write locks | `RUST-NET-CHUNK, RUST-WORLD` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T08-01`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Prepared/read-only resident view avoids holding global world lock during encode/compress/socket; stale version rechecks and cache ownership remain explicit.

## Read-only context — do not broaden

- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-net/src/chunk_pipeline.rs`
- `crates/mc-world/src/storage/read_view.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/play/chunk_stream.rs`
- `crates/mc-world/src/storage/read_view.rs`
- `crates/mc-net/src/play/chunk_stream_world_handle_tests.rs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Only if T07/T08 ranks this as blocker
- One stage at a time

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-net chunk_stream`
- `cargo test -p mc-world read_view`
- `rerun T07-03`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-04.md

# T08-04 — Исправить главный measured save/install/flush stall

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `S1, O2` |
| Depends on | `T07-08, T08-01` |
| Write locks | `RUST-WORLD, RUST-NET-ROOT` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T07-08, T08-01`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Один измеренный stall устранён без ослабления fsync/version fences; stable regions progress, mutated region stays dirty/replans, client remains responsive.

## Read-only context — do not broaden

- `crates/mc-world/src/storage/dirty_flush.rs`
- `crates/mc-net/src/server.rs`
- `crates/mc-net/src/play/persistence.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `exact files from profile`
- `focused fault/dirty-flush tests`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Choose exact hotspot from T07-08
- Record baseline/new numbers

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `persistence`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-world dirty_flush`
- `cargo test -p mc-net persistence`
- `rerun T07-08`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-05.md

# T08-05 — Исправить главный measured entity goal/physics/publication hotspot

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `architecture` |
| Kind | `implementation` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `N1, O2` |
| Depends on | `T07-06, T05-06` |
| Write locks | `RUST-ENTITY, RUST-NET-SESSION` |
| Runtime leases | `NONE` |
| Required evidence | `unit, wire, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T07-06`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Ускорение только выбранного stage с version/CAS/client-liveness fence; ordinary populations retain full cadence, overload stays bounded.

## Read-only context — do not broaden

- `crates/mc-entity/src/regional.rs`
- `crates/mc-net/src/play/session/entity_simulation.rs`
- `crates/mc-net/src/play/session/movement_publication.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `exact hotspot files`
- `focused load/correctness tests`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Choose from T07-06
- No operator worker percentages

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-entity`
- `cargo test -p mc-net entity`
- `rerun T07-06`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T08-06.md

# T08-06 — Autoscale recovery + slow-client shedding profile/fix

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W08` — Только измеренные оптимизации и authority cutovers |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `implementation` |
| Dispatch | `TEMPLATE — coordinator must materialize exact paths/RED command` |
| Ledger rows | `O3, S2` |
| Depends on | `T07-05, T06-07` |
| Write locks | `RUST-NET-SESSION, PERF` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `unit, wire, concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] **DO NOT CLAIM YET.** Materialize this card after `T07-05, T06-07`.
- [ ] Replace every `<placeholder>` or prose write path with exact repository paths.
- [ ] Replace generic validation text with one exact RED command and exact rerun command.
- [ ] Re-run `python3 scripts/board.py validate` after materialization.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Under CPU/memory/queue/slow-reader pressure controller degrades bounded budgets, keeps healthy clients alive, then recovers with hysteresis/headroom; drain/restart exact.

## Read-only context — do not broaden

- `crates/mc-net/src/control_plane.rs`
- `crates/mc-net/src/play/chunk_stream_autoscale_tests.rs`
- `crates/mc-net/src/play/session/outbound.rs`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-net/src/control_plane.rs`
- `focused autoscale/backpressure tests`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use existing metrics and capacity-derived limits
- No full cluster autoscaling

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `cargo test -p mc-net control_plane`
- `cargo test -p mc-net autoscale`
- `profile recovery workload`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T09-01.md

# T09-01 — Двухчасовой no-debug single-client active survival arc

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W09` — Интегрированные survival/multiplayer/soak gates |
| Priority | `P0` |
| Route | `playable` |
| Kind | `integration-gate` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `Q2, S1, O1` |
| Depends on | `T02-01, T02-02, T03-08, T04-07, T05-05, T08-04, T08-05` |
| Write locks | `CLIENT-JAVA, RUNNER, PERF` |
| Runtime leases | `CLIENT-RIG, CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `real-client-agent, persistence, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Fresh world: join/orient → wood/food/shelter/night → stone/furnace/light → iron/tools/shield/armor → renewable food → combat/death/recovery → explore → save/restart/rejoin, active actions only.

## Read-only context — do not broaden

- `docs/playable/ACTIVE.md`
- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `docs/performance/CORE_PROFILE_MATRIX.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/gates/two-hour-survival/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Subagent decisions allowed; deterministic scenario may orchestrate but no debug grants
- Record first common blocker as new microtask

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `real-client-agent`
- `persistence`
- `performance`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `All phase observations`
- `No crash/disconnect/catastrophic stall`
- `Profile result valid`
- `Final state after restart`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T09-02.md

# T09-02 — 30-minute two-client cooperative survival arc

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W09` — Интегрированные survival/multiplayer/soak gates |
| Priority | `P0` |
| Route | `playable` |
| Kind | `integration-gate` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S2, Q2, O1` |
| Depends on | `T03-06, T03-08, T04-04, T05-05, T06-07, T08-06` |
| Write locks | `CLIENT-JAVA, RUNNER, PERF` |
| Runtime leases | `CLIENT-RIG, CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `real-client-agent, concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Two fresh clients cooperate through gather/craft/shared storage/build/combat/death/drop handoff/reconnect/save/restart; all shared state converges.

## Read-only context — do not broaden

- `client-mod/solaris-client-agent/java-agent/src/main/java/dev/solaris/agent/javaagent/PlayableRealClientLoopScenario.java`
- `docs/performance/CORE_PROFILE_MATRIX.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/gates/two-client-coop/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- No operator setup in measured segment
- Slow reader is separate gate

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `real-client-agent`
- `concurrency`
- `performance`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `No duplication/loss`
- `Both remain in_play except deliberate reconnect`
- `Final restart invariant`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T09-03.md

# T09-03 — 36,000-tick / 20-client mixed soak с failing-seed manifests

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W09` — Интегрированные survival/multiplayer/soak gates |
| Priority | `P0` |
| Route | `scaling` |
| Kind | `integration-gate` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S2, O1, O2, O3, Q3` |
| Depends on | `T07-05, T07-07, T07-08, T08-06` |
| Write locks | `PERF, RUST-HARNESS` |
| Runtime leases | `CLEAN-HOST, TREE-FROZEN` |
| Required evidence | `replay/negative, concurrency, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

20 clients execute exploration/edits/crafting/containers/mobs/drops/fluids/save/reconnect/slow reader for exact simulation ticks; failures emit replay manifest and state hash.

## Read-only context — do not broaden

- `crates/mc-test-harness/tests/load_scenarios.rs`
- `crates/mc-test-harness/src/replay.rs`
- `docs/performance/CORE_PROFILE_MATRIX.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/gates/20-client-soak/`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Timeout only fails
- Do not hide degraded sidecars/artifacts

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `replay/negative`
- `concurrency`
- `performance`
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `>18 TPS target or exact blockers`
- `No deadlock/corruption/unbounded growth/reliable loss`
- `Final save/restart`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T09-04.md

# T09-04 — Единый authoritative pre-stop/post-restart state diff

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W09` — Интегрированные survival/multiplayer/soak gates |
| Priority | `P0` |
| Route | `parity` |
| Kind | `implementation` |
| Dispatch | `READY after dependencies` |
| Ledger rows | `S1, S2, Q3` |
| Depends on | `T01-04, T09-01, T09-02` |
| Write locks | `RUST-HARNESS, RUNNER` |
| Runtime leases | `CLIENT-RIG` |
| Required evidence | `unit, wire, replay/negative, persistence, concurrency` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] All dependencies are `DONE` on the integrated tree.
- [ ] Coordinator confirmed no active write-lock or runtime-lease conflict.
- [ ] Exact base SHA, worktree, port range and run directory are assigned.
- [ ] Exact commands/artifact paths for every `Required evidence` leg are pasted into `Validation log` before implementation.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Versioned normalized state snapshot compares players/inventories/chunks/block entities/entities/drops/scheduled work/time-weather in declared scope across stop/restart.

## Read-only context — do not broaden

- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`
- `tools/real-client-agent-driver.py`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `crates/mc-test-harness/src/replay.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`
- `tools/real-client-agent-driver.py`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Extend T01-04, do not create competing schema
- Unknown/unobserved fields explicit

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `unit`
- `wire`
- `replay/negative`
- `persistence`
- `concurrency`

## Required validation

- `cargo test -p mc-test-harness replay`
- `cargo test -p mc-test-harness --test load_scenarios`
- `python -m py_compile tools/real-client-agent-driver.py`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T10-01.md

# T10-01 — Пересчитать все 46 frozen rows по текущему коду и evidence

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W10` — M100 evidence closure |
| Priority | `P1` |
| Route | `parity` |
| Kind | `audit` |
| Dispatch | `COORDINATOR-ONLY — never hand directly to a worker` |
| Ledger rows | `Q1, Q2` |
| Depends on | `T09-03, T09-04` |
| Write locks | `COORD-DOCS` |
| Runtime leases | `NONE` |
| Required evidence | `oracle, real-client-agent` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] Coordinator executes this control/release gate directly.
- [ ] Any implementation work discovered here becomes separate child cards.
- [ ] Never give this umbrella card to Spark as a coding task.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Для каждой строки ровно одна минимально недостающая нога: runtime/oracle/client/perf/concurrency/persistence/scope decision; никаких status edits до аудита.

## Read-only context — do not broaden

- `docs/VALIDATION_LEDGER.md`
- `docs/DEFINITION_OF_DONE.md`
- `.analysis/spark/real-client-matrix.md`
- `.analysis/spark/oracle-replay-matrix.md`
- `.analysis/spark/performance-baseline.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `.analysis/spark/m100-row-audit.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Use current artifacts from W09
- Separate accepted divergence from missing evidence

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `oracle`
- `real-client-agent`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.

## Required validation

- `cargo run -p mc-test-harness --bin coverage-audit -- docs/VALIDATION_LEDGER.md (baseline only)`
- `Independent review of row mapping`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T10-02.md

# T10-02 — Закрыть hard required-green Q1/Q2/O1/O2/O3 blockers

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W10` — M100 evidence closure |
| Priority | `P1` |
| Route | `parity` |
| Kind | `campaign` |
| Dispatch | `COORDINATOR-ONLY — never hand directly to a worker` |
| Ledger rows | `Q1, Q2, O1, O2, O3` |
| Depends on | `T10-01` |
| Write locks | `COORD` |
| Runtime leases | `NONE` |
| Required evidence | `oracle, real-client-agent, performance` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] Coordinator executes this control/release gate directly.
- [ ] Any implementation work discovered here becomes separate child cards.
- [ ] Never give this umbrella card to Spark as a coding task.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Каждый blocker раздаётся отдельной карточкой размером ≤3 source files; закрытие только с required evidence, без umbrella pass.

## Read-only context — do not broaden

- `.analysis/spark/m100-row-audit.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `task cards generated from exact missing legs`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Coordinator creates one child task per missing leg
- Max two disjoint agents

## Non-goals

- Не выполнять несколько missing legs в одном Spark контексте

## Required evidence legs

- `oracle`
- `real-client-agent`
- `performance`
- Before claim, coordinator pastes the exact `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=... bash tools/run-real-client-regression.sh --run` command and expected artifact path from the current scenario matrix. Java tests alone do not satisfy this leg.
- Before claim, coordinator pastes the exact oracle scenario/filter and expected manifest/result path. Expected facts must not be copied from Solaris implementation.
- Record build mode, commit, hardware, profile config, seed, duration, clients/VD, p50/p95/p99/max, queues/locks/memory and result artifact path.

## Required validation

- `Per-child L1`
- `Wave-boundary L2 once`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T10-03.md

# T10-03 — Итеративно закрывать smallest missing legs до ≥37/46 ready

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W10` — M100 evidence closure |
| Priority | `P1` |
| Route | `parity` |
| Kind | `campaign` |
| Dispatch | `COORDINATOR-ONLY — never hand directly to a worker` |
| Ledger rows | `none / campaign-level` |
| Depends on | `T10-02` |
| Write locks | `COORD` |
| Runtime leases | `NONE` |
| Required evidence | `audit/control` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] Coordinator executes this control/release gate directly.
- [ ] Any implementation work discovered here becomes separate child cards.
- [ ] Never give this umbrella card to Spark as a coding task.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Coverage достигает ≥80% только через реальные legs; remaining rows owner-accepted non-goal/divergence или точный deferred debt.

## Read-only context — do not broaden

- `.analysis/spark/m100-row-audit.md`
- `docs/VALIDATION_LEDGER.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `task cards + evidence artifacts`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Sort by effort/value: evidence-only first, then small behavior fixes, then broad systems
- Re-run coverage audit after each wave, not each microtask

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `audit/control`

## Required validation

- `cargo run -p mc-test-harness --bin coverage-audit -- docs/VALIDATION_LEDGER.md`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```

---

# FILE: tasks/T10-04.md

# T10-04 — Финальный L2, M100 decision и честный closeout

Status: `QUEUED`

| Field | Value |
|---|---|
| Wave | `W10` — M100 evidence closure |
| Priority | `P1` |
| Route | `parity` |
| Kind | `release-gate` |
| Dispatch | `COORDINATOR-ONLY — never hand directly to a worker` |
| Ledger rows | `none / campaign-level` |
| Depends on | `T10-03` |
| Write locks | `VALIDATION, COORD-DOCS` |
| Runtime leases | `TREE-FROZEN` |
| Required evidence | `audit/control` |
| Agent | `UNASSIGNED` |
| Worktree / branch | `UNASSIGNED` |
| Base SHA | `UNSET` |
| Started | `UNSET` |

## Dispatch gate

- [ ] Coordinator executes this control/release gate directly.
- [ ] Any implementation work discovered here becomes separate child cards.
- [ ] Never give this umbrella card to Spark as a coding task.

## Live checklist

- [ ] CLAIMED — agent/worktree/base SHA filled
- [ ] BASELINE / RED — current behavior or evidence gap reproduced
- [ ] IMPLEMENTED — smallest task-scoped change complete, or audit artifact written
- [ ] TESTING — exact focused commands/results/log paths recorded below
- [ ] SELF-REVIEW — scope, authority, negative-code and diff inspected
- [ ] INDEPENDENT REVIEW — one read-only verdict recorded
- [ ] DONE — integrated commit/diff/evidence and one next action recorded

## Outcome

Один exact-tree L2, coverage report, client/oracle/perf/soak/persistence/security matrix; release-ready только если каждый hard gate зелёный, иначе stabilization с точным next cursor.

## Read-only context — do not broaden

- `docs/DEFINITION_OF_DONE.md`
- `docs/milestones/M100.md`
- `docs/VALIDATION_LEDGER.md`

For any file over 400 lines: never `cat` it. Use one `rg -n` anchor batch, then open at most three windows of at most 160 lines each.

## Owned write paths

- `docs/milestones/M100.md`
- `docs/VALIDATION_LEDGER.md`
- `docs/REPLACEMENT_READINESS.md`
- `docs/MEMORY.md`

Any additional path requires coordinator approval and a lock check. If the real root cause lives elsewhere, stop `partial` and request one child card.

## Bounded discovery / search anchors

- Freeze tree before L2
- No status inflation

## Non-goals

- No scope beyond the stated outcome.

## Required evidence legs

- `audit/control`

## Required validation

- `cargo run -p xtask -- code-health`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `coverage audit`
- `validate client/perf artifacts`

## Done when

- [ ] The exact outcome above is proven on the authoritative/runtime or evidence path.
- [ ] One dominant failure, stale/race/rejection or missing-artifact boundary is covered when applicable.
- [ ] No sleep, polling, elapsed-time success or timeout-as-success was added.
- [ ] No unrelated source, formatting churn, new abstraction or compatibility layer was introduced.
- [ ] Validation is classified precisely as unit/wire/oracle/real-client/performance/concurrency/persistence.
- [ ] Every `Required evidence` leg has a concrete current-tree artifact; no unit-only substitution for oracle/client/perf.
- [ ] Card contains changed files, diff/commit hash, commands, log paths, known gaps and one next action.

## Validation log

- `UNRUN`

## Review

- Reviewer: `UNASSIGNED`
- Verdict: `UNRUN`
- Findings: `[]`

## Closeout

```yaml
verdict: pending
status: queued
base_tree: UNSET
diff_hash: UNSET
changed_files: []
validation: []
evidence: []
known_gaps: []
next: claim this task
```
