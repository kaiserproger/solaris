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


## Автономный `/goal` режим

Для one-shot запуска используйте [`AUTOPILOT.md`](AUTOPILOT.md) и paste-ready команду из корня комплекта `PASTE_THIS_GOAL.md`. Главный поток остаётся координатором, выбирает следующую finite-card через `scripts/autopilot.py`, запускает не более двух изолированных Spark-субагентов, проводит один независимый review, интегрирует и продолжает без ручного `continue`.

После установки:

```sh
python3 docs/spark-team/scripts/autopilot.py doctor --init-state
python3 docs/spark-team/scripts/autopilot.py next --limit 2 --json
```

Текущая checkbox-сводка генерируется в [`AUTOPILOT_STATUS.md`](AUTOPILOT_STATUS.md):

```sh
python3 docs/spark-team/scripts/autopilot.py dashboard
```

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
