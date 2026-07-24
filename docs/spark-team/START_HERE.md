# Start Here — первые действия координатора


## Полностью автономный запуск

После установки `.codex/agents` и project config не раздавайте batch вручную. Откройте Codex в корне репозитория и вставьте команду из `PASTE_THIS_GOAL.md`. Runbook: [`AUTOPILOT.md`](AUTOPILOT.md). Он сам выполняет preflight, выбирает compatible pair, создаёт worktrees, требует карточечные checkboxes/evidence, вызывает read-only reviewer, интегрирует и берёт следующий batch.

```sh
python3 docs/spark-team/scripts/autopilot.py doctor --init-state
python3 docs/spark-team/scripts/autopilot.py dashboard
```

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
