---
date: 2026-09-20T20:27:39+0700
author: kaiserproger
commit: 1d39e864
branch: main
repository: solaris
topic: "Phase 1 — актуальный вход и явный локальный словарь"
tags: [architecture, documentation, mc-net, behavior-preserving]
status: in-progress
parent: .rpiv/artifacts/architecture-reviews/2026-09-20_18-44-21_solaris-model-comprehensibility.md
phase_count: 4
phases:
  - n: 1
    title: Точки входа и история
    files: [AGENTS.md, README.md, docs/MEMORY.md, docs/AGENT_ROUTES.md, docs/memory/README.md, docs/memory/2026-09-20-pre-review-history.md, crates/mc-net/src/lib.rs, crates/mc-protocol/src/lib.rs, crates/mc-worldgen/src/terrain.rs]
    depends_on: []
  - n: 2
    title: Контракт SessionRegistry
    files: [crates/mc-net/src/play/session.rs, docs/decisions/0006-mc-net-module-boundaries.md]
    depends_on: [1]
  - n: 3
    title: Явные зависимости merchant adapter
    files: [crates/mc-net/src/play/merchant_adapter.rs]
    depends_on: [2]
  - n: 4
    title: Именованный routing key
    files: [crates/mc-net/src/play/simulation.rs]
    depends_on: [3]
unresolved_phase_count: 4
last_updated: 2026-09-20T20:27:39+0700
last_updated_by: kaiserproger
---

# Phase 1: Entry Points and Local Vocabulary — Implementation Plan

## Overview

План покрывает только Phase 1 согласованного архитектурного review: актуализирует точки входа в проект и делает существующие локальные зависимости и роли состояния явными. Production-изменения ограничены импортами merchant adapter и именованным приватным ключом группировки simulation-команд; владельцы состояния, алгоритмы и observable behavior сохраняются. Это документ планирования, не выполненная реализация.

## Requirements

- Закрыть L0-04 вместе с поглощёнными L1-04/L5-01, L2.3-01, L2.4-01 и L2.2-02 исходного review.
- Различать текущие исходники/workspace, опубликованные версии, целевую архитектуру и исторические evidence.
- Сохранить owner contract, правила работы с Git, валидацией, sibling repositories и текущие блокеры при сокращении входной документации.
- Перенести завершённую историю из MEMORY без потери текста; сохранить navigation к старым receipts. Живой курсор остаётся один.
- Описать SessionRegistry без новых владельцев, полей, locks, перестановки полей или изменения их типов.
- Удалить единственный wildcard import merchant adapter, используя существующую видимость и containers façade.
- Заменить пять позиционных bool именованным приватным ключом, сохранив predicates, adjacent-only grouping, ветвление и fail-stop cleanup.
- Не реализовывать другие семь фаз review и не изменять source tree во время Blueprint.

## Current State Analysis

HEAD проверен как `1d39e864b291eb3e8c45ba5081b6ce10d40e4c46`. Рабочее дерево содержит значительные существующие изменения, включая целевые файлы; план основан на прочитанном working tree, а не на чистом HEAD. Перед применением требуется сверить конкретные заменяемые блоки; нельзя восстанавливать файлы из HEAD или затирать чужие изменения.

### Key Discoveries

- `README.md:7,19,47,235` смешивает предварительный v0.0.6 и development tree; Cargo workspace уже 0.0.8. `docs/MEMORY.md:28` отдельно фиксирует release candidate, историю v0.0.7 и блокер публикации. Номер workspace не доказывает публикацию.
- `AGENTS.md` сохраняет устаревшую Luau-терминологию, тогда как `README.md:153–165` описывает Wasmtime components и product source в sibling repository. Правила owner/process не отменяются вместе с устаревшими описаниями.
- `docs/AGENT_ROUTES.md:44,52–53` одновременно показывает независимый Loader checkout и отрицает отдельные repositories client tooling.
- `docs/MEMORY.md` содержит 2693 строки при обещании малого курсора. `docs/memory/README.md:24–30` уже задаёт сохранение текста и исправление относительных ссылок при архивировании. Ссылок `MEMORY.md#...` целевой integration-pass не обнаружил, но document-level ссылки из ADR/playable документов семантически обещают исторические receipts — одного существования файла недостаточно.
- `crates/mc-net/src/lib.rs:7–10` описывает Login/Configuration/Play как будущую работу; текущие модули и фасад ниже уже существуют.
- `crates/mc-protocol/src/lib.rs:9–16` аналогично описывает framing/packets как будущие слои. Protocol/version constants не меняются.
- `crates/mc-worldgen/src/terrain.rs:203–205` обещает четыре block types и allocation-free generation; текущие поля и стадии шире. Меняются только комментарии, не численные параметры и не WORLDGEN_REVISION.
- `session.rs:433–509,582–735` содержит разные роли состояния: shared authoritative player state, session indexes, publications, handles, caches, world values. `lock_entities` возвращает access wrapper, а не универсальный entity mutex; `SessionEntityGuards` не является cross-store atomic transaction.
- `merchant_adapter.rs:1` импортирует parent wildcard. `containers.rs:16,64–70` сохраняет приватность merchant-модуля и существующие доступные reexports. Три entry points вызываются из одного `play.rs`; тела функций менять не требуется.
- `simulation.rs:4495–4580` группирует только соседние envelopes по пяти bool. `simulation.rs:4711–4714` содержит дополнительное tuple destructuring при fail-stop rejection; его необходимо изменить одновременно.

## Desired End State

Модель/разработчик попадает из живого курсора в текущий route, а за provenance переходит в явный исторический архив. Потребитель gameplay API не меняется; приватные routing conditions становятся именованными:

```rust
let route = SimulationRunRoute {
    requires_world,
    regional_block_edit,
    journaled_block_edit,
    block_drop_command,
    cross_region_plugin_edit,
};
```

Зависимости adapter читаются через существующие модули, без расширения их видимости:

```rust
use super::simulation::{MerchantTradeDestination, MerchantTradePlan};
```

Это иллюстрации целевой формы. Полные применяемые изменения фиксируются только в одобренных фазах ниже.

## What We're NOT Doing

- Не создаём actors, crates, services, DI, generic routing framework или новую документационную систему.
- Не превращаем SessionRegistry в новый owner и не переносим его состояние между locks/structs.
- Не меняем public façades, ABI/WIT, protocol IDs, packet layout, persisted formats, worldgen revision или generation algorithm.
- Не переносим тесты и не реализуем startup/supervisor/ingress/manifest/entity/storage extraction из следующих фаз review.
- Не запускаем в Blueprint сборки, тесты, сервер или клиента и не выдаём inspection за runtime verification.
- Не обновляем Cargo dependencies, Cargo.lock или версии workspace. `mc-net/Cargo.toml` исключён: текущее description корректно.
- Не меняем sibling repositories, Git staging/commits/tags/remotes и не публикуем релизы.
- Не делаем выводов о measured model performance, gameplay parity, production readiness или client acceptance.

## Decisions

### D1 — Scope и источники истины

Выбрана только Phase 1 review. Current source/configuration определяет описание реализации; target ADR не выдаётся за завершённую миграцию. Release status отделён от workspace version (`README.md:7`, `docs/MEMORY.md:28`, `Cargo.toml`). Metadata времени/автора сохранены из invocation; HEAD независимо проверен, dirty worktree явно оговорён.

### D2 — Архив в существующей системе

Используется `docs/memory/`, его индекс и правило сохранения текста (`docs/memory/README.md:24–30`). Новый архив `2026-09-20-pre-review-history.md` не является вторым живым курсором. Текущая работа и blockers остаются в MEMORY; исторические evidence доступны через явный маршрут. Добавление существующего archive index к forecast-paths согласовано; неактуальный кандидат на изменение `mc-net/Cargo.toml` исключён.

Исключение из формата NEW-file code fence согласовано отдельно: полный исторический текст не дублируется внутри Blueprint. Для этого одного архива приводится полная исполнимая команда переноса с точными границами, исходным SHA-256 и проверкой сохранности. Команда исполняется только при реализации, до замены MEMORY. Остальные изменения выписываются полностью в обычном формате.

### D3 — Только документирование SessionRegistry

Комментарии различают shared authoritative player values, owner handles, derived publications/indexes и собственные world values. Следуем source-local комментированию canonical/projection ролей из `mc-world/src/storage.rs:150–190`; не копируем его ownership topology. `SessionEntityGuards` и `lock_entities` описываются по текущей реализации, не по названию (`session.rs:862`, `session.rs:1582–1618`).

### D4 — Конкретные импорты через доступные границы

Следуем `player_damage_adapter.rs:1–29`: внешние dependencies, crate infrastructure, sibling modules, затем только действительно parent-owned symbols. Private `containers::merchant` не открывается; используем существующий `containers` façade (`containers.rs:16,64–70`). Prelude symbols не перечисляются искусственно. Никакого repository-wide wildcard cleanup.

### D5 — Private routing key без изменения классификации

`SimulationRunRoute` содержит те же пять bool и сравнивается по всем полям. Существующие predicates вычисляются в прежнем порядке на прежнем месте. Список runs остаётся последовательным; route не сортирует и не объединяет несоседние envelopes. Named private routing records уже существуют в `simulation.rs:3564–3574`; exact five-bool equality key — небольшая новая private форма, не заимствованный готовый helper.

### D6 — Verification и performance

Документация проверяется статически; imports и private routing refactor — существующими focused Rust tests при будущей реализации. Поведенческую корректность нельзя доказывать сравнением текста Rust statements. Полный canonical correctness запускается только после всего реализованного плана. Allocation strategy, async boundaries, locks и ordering не меняются; никаких perf claims или нового benchmark-проекта.

## Phase 1: Точки входа и история

### Overview

Согласовать входные описания и архивировать завершённую историю без потери evidence. Depends on: nothing; первый срез, остальные выполняются после его закрытия. Крупный архивный перенос намеренно целостный, а не разбит по произвольному лимиту строк.

### Changes Required:

#### 1. AGENTS.md
**File**: AGENTS.md
**Changes**: MODIFY — актуальные версии/технологии/навигация без ослабления owner contract.
```markdown
```

#### 2. README.md
**File**: README.md
**Changes**: MODIFY — разделить текущий tree, release evidence и исторические инструкции.
```markdown
```

#### 3. docs/MEMORY.md
**File**: docs/MEMORY.md
**Changes**: MODIFY — оставить текущий курсор, blockers и архивную навигацию.
```markdown
```

#### 4. docs/AGENT_ROUTES.md
**File**: docs/AGENT_ROUTES.md
**Changes**: MODIFY — исправить карту независимых repositories и разграничить current/target/history.
```markdown
```

#### 5. docs/memory/README.md
**File**: docs/memory/README.md
**Changes**: MODIFY — добавить новый архив в существующий индекс.
```markdown
```

#### 6. docs/memory/2026-09-20-pre-review-history.md
**File**: docs/memory/2026-09-20-pre-review-history.md
**Changes**: NEW — сохранённая история, индекс секций и исправленные относительные ссылки.
```sh
```

#### 7. crates/mc-net/src/lib.rs
**File**: crates/mc-net/src/lib.rs
**Changes**: MODIFY — актуальный crate intro вместо будущего Login/Configuration/Play.
```rust
```

#### 8. crates/mc-protocol/src/lib.rs
**File**: crates/mc-protocol/src/lib.rs
**Changes**: MODIFY — описать существующие codec/frame/packets без изменения constants.
```rust
```

#### 9. crates/mc-worldgen/src/terrain.rs
**File**: crates/mc-worldgen/src/terrain.rs
**Changes**: MODIFY — исправить устаревшее описание generator, только комментарии.
```rust
```

### Success Criteria:

#### Automated Verification:

#### Manual Verification:

## Phase 2: Контракт SessionRegistry

### Overview

Описать существующую authority topology рядом с кодом и в owning ADR. Depends on: Phase 1 по порядку исполнения, не по новым runtime symbols; параллельное применение не предусмотрено.

### Changes Required:

#### 1. crates/mc-net/src/play/session.rs
**File**: crates/mc-net/src/play/session.rs
**Changes**: MODIFY — ownership contract и комментарии к семантическим группам без перестановки полей.
```rust
```

#### 2. docs/decisions/0006-mc-net-module-boundaries.md
**File**: docs/decisions/0006-mc-net-module-boundaries.md
**Changes**: MODIFY — уточнить роли registry/access/publication без объявления новой миграции.
```markdown
```

### Success Criteria:

#### Automated Verification:

#### Manual Verification:

## Phase 3: Явные зависимости merchant adapter

### Overview

Сделать импортируемые зависимости видимыми без изменения adapter behavior. Depends on: Phase 2 по согласованному последовательному порядку; новые runtime interfaces других фаз не нужны.

### Changes Required:

#### 1. crates/mc-net/src/play/merchant_adapter.rs
**File**: crates/mc-net/src/play/merchant_adapter.rs
**Changes**: MODIFY — заменить use super::* полным набором явных imports.
```rust
```

### Success Criteria:

#### Automated Verification:

#### Manual Verification:

## Phase 4: Именованный routing key

### Overview

Заменить позиционный routing vocabulary именованным private key, включая все места потребления runs. Depends on: Phase 3 по согласованному последовательному порядку; никаких parallel slices.

### Changes Required:

#### 1. crates/mc-net/src/play/simulation.rs
**File**: crates/mc-net/src/play/simulation.rs
**Changes**: MODIFY — private key, run construction/grouping/execution и fail-stop cleanup.
```rust
```

### Success Criteria:

#### Automated Verification:

#### Manual Verification:

## Ordering Constraints

Все четыре среза генерируются, согласуются и применяются последовательно: 1 → 2 → 3 → 4. `depends_on` фиксирует этот execution order, а не выдуманные runtime dependencies. В каждом срезе декларации и все их потребители изменяются атомарно. Архив, живой курсор и индекс закрываются вместе. Approved phase blocks неизменяемы без явного переоткрытия пользователем.

## Verification Notes

- V1 / Phase 1: проверить все изменённые ссылки и semantic navigation к прежним receipts, а не только отсутствие broken paths. Проверить сохранность переносимого текста с учётом явно разрешённого исправления относительных links; не потерять failed/manual-pending evidence.
- V2 / Phase 1: workspace/release/target/history различимы; Luau не описывается как текущий production host; Loader/default plugins остаются независимыми repositories. Никакой новой публикации/readiness claim.
- V3 / Phase 1: изменения Rust entrypoints исключительно комментарии; protocol constants и worldgen behavior не затронуты. Owner/process правила не ослаблены.
- V4 / Phase 2: source-local текст верно различает player authority, entity owner handles, publications, indexes и world values; field order/types/locks неизменны. Не называть access wrapper mutex или атомарной транзакцией между stores.
- V5 / Phase 3: существующая privacy сохраняется, все free symbols разрешаются, функции/вызовы не меняются. Focused merchant tests в `containers/merchant.rs` и `session/villager_merchant_tests.rs` проверяют selection, stale commit, repeated trade и cursor/out-of-stock fences.
- V6 / Phase 4: сохранить все пять routing dimensions, adjacent-only grouping, cancellation/session predicates, journal requirement и branch precedence. Existing tests: `simulation.rs:8841` journal/nonjournal boundary; `:9431,9488` ordering/barriers; `:8957` fail-stop; `:10306` lock release before non-world command; `:11122,11154,19317` shutdown/cancel. Это anchors для отбора точных test names, не заявление о полном покрытии всех predicate combinations.
- V7 / Phase 4: оставшиеся runs при owner fail-stop отклоняются с OwnerStopped перед shutdown (`simulation.rs:4711–4714`); neighboring block-drop/precommit tests дополняют этот риск. Проверять behavior тестами, не source-text statement order.
- V8 / whole plan: `python3 -m tools.harness run correctness` после полной реализации, с реальным result.json. Не помещать whole-workspace gates в отдельные срезы. Blueprint проверяет документ, не выполняет этот gate.
- V9 / all phases: path-limited diff относительно зафиксированного состояния сохраняет чужой dirty worktree; любые fixing commands ограничены собственными файлами фазы. Не stage/commit/publish без отдельной команды.

## Whole-Plan Verification

После реализации всех фаз запустить `python3 -m tools.harness run correctness` и сохранить точный receipt. Проверить совместный diff против принятого scope и независимую read-only оценку. Если меняется поведение или обнаруживается клиентская регрессия, остановить этот behavior-preserving refactor и отдельно согласовать scope; unit gates не заменяют real-client acceptance. Blueprint не заявляет ни один из этих будущих gates выполненным.

## Performance Considerations

Изменение навигации и именование private key не являются измеренной оптимизацией. Сохраняются пять bool, прежние allocations/runs и последовательность вычисления predicates; не вводятся sorting, caching, extra async tasks или ownership indirection. Профилирование и оценка качества Terra вне scope.

## Migration Notes

Persisted schema, ABI, packet layout и API migration отсутствуют. Документационный перенос сохраняет provenance и ссылки; это не миграция игровых данных. Rollback реализации — обратный целевой diff только собственных изменений, не reset чужого дерева.

## Pattern References

- `crates/mc-net/src/play/player_damage_adapter.rs:1–29` — явные imports по действительным доступным модулям, parent-owned symbols отдельно.
- `crates/mc-net/src/play/containers.rs:16,64–70` — существующая privacy и merchant façade.
- `crates/mc-net/src/play/simulation.rs:3564–3574` — небольшие private named routing records; exact equality key здесь новый.
- `crates/mc-world/src/storage.rs:150–190` — комментарии distinguish canonical state, read views и compatibility snapshot handles; не ownership template для буквального копирования.
- `docs/memory/README.md:24–30`, `docs/MEMORY.md:1–25` — один живой курсор, исторический индекс, сохранение текста.
- `docs/AGENT_ROUTES.md:3–23` — route → primary doc → конкретные code/evidence entrypoints.
- `docs/decisions/0006-mc-net-module-boundaries.md` — bounded local extraction, существующие authority/publication boundaries, отсутствие speculative layers.

## Precedents & Lessons

Унаследованы из согласованного review, без отдельного git-history исследования:

- Файловая локальность не равна изоляции владельца: Phase 2 документирует, но не переопределяет authority (V4).
- Green Rust gates не доказывают survival acceptance, publication или performance: документационный перенос не повышает статус доказательств (V1–V3, V8).
- Позиционная tuple встречается и в аварийной очистке, а не только основном loop: весь локальный consumer set входит в Phase 4 (V6–V7).
- Повторная система документации создаёт вторую истину: используется существующий MEMORY/archive/route механизм (V1–V2).

## Developer Context

### Inherited scope

Пользователь выбрал «Phase 1 — Актуальный вход и явный локальный словарь» завершённого architecture review. Направления и ограничения review наследуются; другие фазы не разрешены. Подтверждение Blueprint не является разрешением реализации.

### Design checkpoint — exact question and answer

**Question:**

❓ Question: Phase 1 — актуальные точки входа и явный локальный словарь. Подтверждаете следующий дизайн перед разбиением на срезы?

• Документация: разделить текущий workspace 0.0.8, сведения о релизах и target-архитектуру; исправить устаревшие описания Luau и границ репозиториев. Не менять версии кода и не объявлять релиз опубликованным на основании номера workspace (Cargo.toml; README.md:7; docs/MEMORY.md:28).
• MEMORY: оставить текущую работу, блокеры и ссылки; завершённые записи перенести без потери текста в docs/memory/, сохранив доступ к старым evidence-ссылкам (docs/memory/README.md:28).
• SessionRegistry: только локальный контракт и комментарии к группам полей — различать authority, handles, projections, indexes и world state; не переставлять поля и не менять locks (session.rs:433,582,862).
• Merchant: явные импорты по образцу player_damage_adapter.rs:1; приватный merchant-модуль не открывать, использовать существующий containers-фасад (containers.rs:16,64).
• Simulation: private именованный ключ из пяти bool; прежние predicates, соседняя группировка, порядок ветвей и fail-stop cleanup (simulation.rs:4495–4714).
• Проверки в плане: ссылки/сохранность архива, существующие merchant и routing-регрессии; полный correctness только для завершённой реализации. Без заявлений о производительности и gameplay-паритете.

Предварительно: 1 новый архив, 12 изменяемых файлов, включая его существующий индекс. mc-net/Cargo.toml исключён: описание не устарело. Исходники сейчас не изменяем.

**Answer:** Продолжить (Recommended).

### Decomposition checkpoint — exact question and answer

**Question:** ❓ Question: Утверждаем четыре среза Phase 1 в показанном порядке: точки входа/архив → контракт SessionRegistry → merchant imports → simulation routing key? Все 13 файлов распределены; другие фазы архитектурного review не включены.

**Answer:** Утвердить (Recommended).

Предваряющее представление указало файлы всех четырёх срезов, отсутствие semantic dependencies и последовательное исполнение. Документационный срез намеренно крупнее обычного из-за целостного archive relocation.

### Archive representation exception — exact question and answer

**Question:** ❓ Question: MEMORY занимает 182 КБ / 2693 строки. При этом даже раздел «Active checkpoint» (docs/MEMORY.md:412) ссылается на уже удалённый wasm_settlement_operations.rs; актуальная поправка и блокеры находятся выше, в :62–188. Поэтому при сокращении нужно сохранить историю целиком, но не выдавать старые инструкции за текущие.

Для этого одного механического переноса допускаете исключение из формата Blueprint: вместо дублирования всего исторического текста в кодовом блоке — полная команда переноса с точными границами, проверкой исходного хеша и сохранности результата? Новый живой курсор и все остальные изменения останутся полностью выписанными. Команда будет только в плане, сейчас ничего не переносится.

**Answer:** Команда переноса (Recommended).

## Plan History

- Phase 1: Точки входа и история — pending
- Phase 2: Контракт SessionRegistry — pending
- Phase 3: Явные зависимости merchant adapter — pending
- Phase 4: Именованный routing key — pending

## References

- `.rpiv/artifacts/architecture-reviews/2026-09-20_18-44-21_solaris-model-comprehensibility.md` — ready, выбранная Phase 1 и унаследованные решения.
- `AGENTS.md`, `docs/AGENT_TOOLING.md` — owner/process и canonical validation contract.
- `docs/ARCHITECTURE.md`, `docs/decisions/0006-mc-net-module-boundaries.md` — target/current distinction и локальные boundary правила.
- Все code/route/template references указаны в Current State Analysis и Pattern References; line anchors относятся к working tree при планировании и могут сместиться.
