---
template_version: 1
date: 2026-09-20T18:44:21+0700
author: kaiserproger
commit: 1d39e864
branch: main
repository: solaris
target: .
target_kind: directory
layer_count: 7
phases:
  - n: 1
    title: "Актуальный вход и явный локальный словарь"
    depends_on: []
    blast_radius: internal
    effort: M
  - n: 2
    title: "Startup/config: тесты, затем локальные реализации"
    depends_on: []
    blast_radius: internal
    effort: L
  - n: 3
    title: "Supervisor: physics и checkpoint"
    depends_on: []
    blast_radius: internal
    effort: L
  - n: 4
    title: "Семейства play ingress"
    depends_on: []
    blast_radius: internal
    effort: M
  - n: 5
    title: "Локальность simulation tests"
    depends_on: []
    blast_radius: internal
    effort: M
  - n: 6
    title: "Native manifest отдельно от admission"
    depends_on: []
    blast_radius: internal
    effort: M
  - n: 7
    title: "Entity: тестовые контракты и две локальные границы"
    depends_on: []
    blast_radius: internal
    effort: L
  - n: 8
    title: "Storage tests без изменения persistence"
    depends_on: []
    blast_radius: internal
    effort: M
unresolved_finding_count: 0
status: ready
tags: [architecture-review, solaris, model-comprehensibility, modularity]
last_updated: 2026-09-20T18:44:21+0700
last_updated_by: kaiserproger
---

# Architecture review — Solaris: понятность для моделей

Независимый структурный обзор: насколько легко модели вроде Terra найти нужную реализацию, понять владельца состояния, границы ответственности и безопасный контекст изменения. Это не поиск багов и не проверка поведения. Пользователь разрешил создавать и обновлять только этот Markdown-отчёт; исходники не меняются, сборки, тесты, harness и приложения не запускаются.

## Краткая оценка

**Solaris не выглядит бесструктурным монолитом: границы между crates и многие локальные контракты понятны. Основная трудность для модели — стоимость навигации внутри нескольких runtime hubs и устаревшего входного контекста.** Самые трудные точки выбранного scope — play/simulation/regional roots: там требуется одновременно удерживать policy, state ownership, publication и тестовые fixtures. Разделение файлов уже местами опережает явное описание authority.

Наибольший практический выигрыш дают актуальные entry docs, тематические tests и согласованные извлечения существующих concern groups. Не требуется тотальная перестройка на actors/crates/services. SessionRegistry нуждается прежде всего в точном ownership contract; прямые regional handle paths нельзя принять за пассивный transport. mc-script/host/SDK имеют полезное разделение native admission, Wasmtime execution и WIT ABI. StartupData, QuickCraftState и комментарии WorldStorage — положительные ориентиры.

Это качественная оценка ограниченной выборки, **не измерение качества Terra**, не bug/security audit и не свидетельство gameplay/parity/performance готовности. Результат triage: **20 accepted, 0 rejected, 0 deferred, 0 withdrawn**; 2 accepted absorbed into L0-04, то есть 18 самостоятельных рекомендаций. Новых методологических принципов — 0; подтверждённых общих тем — 5. План предлагает 8 фаз с **101 уникальным именованным planned path** (включая новые test/module files и условные consumer/doc updates); фактический implementation footprint уточняет blueprint, а этот review изменяет только один Markdown.

## Scope and coverage

Пользователь согласовал ограниченный сквозной проход: **26 исходных файлов, 7 слоёв, 104 467 строк с inline-тестами**. Каждый выбранный файл должен быть прочитан полностью до выдвижения находок его слоя. Соседние исходники используются для проверки потребителей и границ, но не считаются полностью проверенными. Документация и manifests — контекст; harness и соседние репозитории — вне глубокого разбора.

Навигационный инвентарь: `crates/**/*.rs` — 850 файлов / 589 069 строк; дополнительно SDK — 16 Rust-файлов, `tools/harness` — 23 Python/shell-файла, WIT — 15 файлов. Это **не** заявка на полное ревью этих 904 файлов. Числа получены статическими `find`/`wc`, без запуска проектного кода. Git metadata скопированы из вызова скилла; предмет чтения — текущие файлы рабочего дерева, не независимо проверенный снимок коммита.

Предварительные выборочные чтения до согласования слоёв — ориентирование, а не основание считать слои завершёнными. Граф обычных Cargo-зависимостей исследован отдельно от dev-dependencies. Предварительная карта: `mc-server` соединяет `mc-net` и `mc-plugin-host` через `mc-script`; gameplay пока остаётся внутри `mc-net`. Целевое состояние из `docs/ARCHITECTURE.md` не подменяет текущее.

## Conventions

### Finding shape

Каждая находка имеет ID `L<слой>-<номер>` и поля:

| Поле | Содержание |
|---|---|
| Evidence | `file:line` и короткая дословная цитата |
| Current state | Наблюдаемая структура сегодня |
| Desired state | Желаемая понятность и граница |
| Proposed improvement | Конкретное локальное изменение |
| Severity | Low / Med / High: влияние на понимание, не тяжесть бага |
| Effort | S / M / L |
| Blast radius | internal / public-API / on-disk / cross-module |
| Class | polish / redesign |
| Status | open / accepted / rejected / deferred / withdrawn |
| Depends on | ID обязательных предшественников |
| Cross-cut tag | Тема для последующего синтеза |

`accepted` означает согласованную рекомендацию, **не выполненное изменение**. `deferred` — post-release. Любое решение фиксируется после явного triage пользователя; находки не принимаются автоматически. Размер файла — сигнал для исследования, не самостоятельное доказательство плохой архитектуры. Никакие измерения качества Terra не проводятся.

### Layers (top → down)

| # | Слой | Полностью читаемые файлы |
|---|---|---|
| 0 | Запуск и композиция | `crates/mc-server/src/main.rs`, `crates/mc-server/src/lib.rs`, `crates/mc-server/src/startup_data.rs` |
| 1 | Сеть и lifecycle | `crates/mc-net/src/lib.rs`, `crates/mc-net/src/connection_driver.rs`, `crates/mc-net/src/server.rs` |
| 2 | Gameplay | 8 файлов в подслоях ниже |
| 2.1 | Connection-local gameplay | `crates/mc-net/src/play.rs` |
| 2.2 | Simulation admission / ordering | `crates/mc-net/src/play/simulation.rs`, `crates/mc-net/src/play/simulation/save_barrier.rs` |
| 2.3 | Shared owner and player state | `crates/mc-net/src/play/session.rs`, `crates/mc-net/src/play/session/player_state.rs`, `crates/mc-net/src/play/session/player_item_action_authority.rs` |
| 2.4 | Domain adapter and local state machine | `crates/mc-net/src/play/merchant_adapter.rs`, `crates/mc-net/src/play/containers/quickcraft.rs` |
| 3 | Plugin contract / host / SDK | `crates/mc-script/src/lib.rs`, `crates/mc-plugin-host/src/lib.rs`, `crates/mc-plugin-host/src/adapter.rs`, `sdk/rust/solaris-plugin-sdk/src/lib.rs` |
| 4 | Entity ownership | `crates/mc-entity/src/lib.rs`, `crates/mc-entity/src/regional.rs` |
| 5 | Data / protocol / generation | `crates/mc-data/src/lib.rs`, `crates/mc-protocol/src/lib.rs`, `crates/mc-worldgen/src/lib.rs`, `crates/mc-worldgen/src/terrain.rs` |
| 6 | World storage | `crates/mc-world/src/lib.rs`, `crates/mc-world/src/storage.rs` |

Общие contextual anchors: `AGENTS.md`, `README.md`, `docs/MEMORY.md`, `docs/AGENT_ROUTES.md`, `docs/AGENT_TOOLING.md`, `docs/ARCHITECTURE.md`, `docs/decisions/0006-mc-net-module-boundaries.md` и manifests workspace/crates/SDK. Полное чтение длинных contextual документов не заявляется.

## Methodology principles

Step 6 checkpoint выполнен после закрытия всех слоёв. Решение пользователя: **Нет нового принципа (Recommended)**. Новых M-принципов не заявлено; конкретные ограничения остаются в принятых находках.

## Layer 0 — Запуск и композиция

**Coverage:** все три выбранных файла прочитаны полностью, включая inline-тесты: `main.rs` (4189 строк по wc), `lib.rs` (2671), `startup_data.rs` (405). Read автоматически обрезает большие ответы; непрочитанные хвосты дочитаны последовательно до EOF, без пропусков. Ни один тест не запускался.

**Consumer verification:** статические reference-поиски с разрешением одноимённых типов/методов: `mc_server::ServerConfig` — 12 исходных файлов, включая определение; его `to_network` — 5; `manage_operator_file` — 5; `load_access_control_files` — 6. `StartupData` — 2 файла с исполняемыми ссылками; основные `load_effective_{protocol_data,tags,loot,recipes}` — по 3. Это counts файлов, не call sites, и не утверждение о внешних checkout. Проверены root-зависимости console и существующих test-модулей. `OperatorFileResult` и `AccessControlLoadReport` используются через вывод типов: одна явная декларация имени не означает отсутствие потребителей.

**Ten-dimension sweep:** boundary/coherence/granularity/intention — main совмещает композицию с реализациями deployment, startup world preparation и check-output, lib совмещает конфигурацию с файловой политикой доступа. Public surface — реальные binary/integration consumers есть, удаление экспортов не предлагается. DRY — оснований вводить общий startup framework нет. DDD/naming — исторические process-документы расходятся с текущим component-контрактом. Error posture — отдельная унификация ошибок не обоснована этой задачей. Module graph — основной seam `StartupData` полезен; новая crate или DI не требуются.

**Keep:** `StartupData` — цельный именованный этап загрузки и проверки immutable-данных (`startup_data.rs:7`: `Immutable gameplay data validated before world preparation or network startup.`). Его ~405 строк не являются самостоятельной причиной дробить загрузчики по одному на файл. `PreparedComponent::take_host` и `Drop` документируют одного владельца остановки — перенос обязан сохранить их вместе.

**Triage:** четыре кандидата рассмотрены; все четыре рекомендации приняты. Никакие source-изменения не выполнены.

### L0-01 — Выделить существующие startup-подсистемы из main

**Evidence:** `crates/mc-server/src/main.rs:200`: `struct PreparedComponent`; `:489`: `fn operator_warnings`; `:936`: `async fn serve`; `:1896`: `fn generate_chunk_positions`; `:2111`: `fn bake_spawn_window_light_for_positions`. `main` расположен на строке 2449, до большого inline test-блока.

**Current state:** единая точка входа содержит startup composition, component deployment/reload, check-output, materialization terrain rules, генерацию чанков и light-bake workers. Для понимания порядка запуска приходится отделять последовательность этапов от их реализаций внутри одного корня.

**Desired state:** корень показывает CLI и порядок запуска; имена модулей непосредственно указывают владельцев component preparation, terrain assembly, spawn preparation и config check.

**Proposed improvement:** разделить `startup/{components,terrain,spawn,check}.rs` по уже существующим группам функций. Сохранить `serve` как явную композицию, а не заменить его framework/pipeline-DSL. `PreparedComponent`, `take_host`, `Drop` и остановку не переданного серверу host переносить одним блоком; остановка после bind/run сохраняет прежнего владельца. Не менять configure/init, pre-world validation, world identity, порядок публикации/сохранения, worker budgets или сетевое поведение. Перенаправить root-зависимости console и существующих `component_startup_tests`/`structure_rules_tests`; не разносить код по файлам произвольной длины.

- **Severity:** High
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить startup/ (Recommended).
- **Depends on:** L0-03 — тематическая организация тестов задаёт сохранённые проверки переносимых обязанностей.
- **Cross-cut tag:** `explicit-composition`

### L0-02 — Развести configuration schema, access-control I/O и runtime assembly

**Evidence:** `crates/mc-server/src/lib.rs:46`: `pub struct ServerConfig`; `:600`: `fn manage_access_file`; `:948`: `fn write_access_profiles`; `:1363`: `pub fn to_network`.

**Current state:** schema/defaults, файловые lock/replace/permission-правила ops/whitelist и перевод в `mc_net::ServerConfig` располагаются в библиотечном корне. Эти обязанности читаются и изменяются по разным причинам.

**Desired state:** модель находит конфигурационный словарь, файловую политику и runtime translation отдельно, а публичный вход библиотеки остаётся обозримым.

**Proposed improvement:** выделить `config/{mod,access_control,network}.rs`: schema/defaults, существующие методы и private helpers управления файлами, существующий runtime translation. `lib.rs` оставить явным фасадом. Сохранить текущие публичные имена/методы через нормальные facade reexports, не вводя второй реализации или obsolete compatibility path. Файловая политика остаётся цельной: sidecar lock, identity normalization, metadata preservation и durable replacement не разъединяются новыми сервисами. TOML/JSON/формат на диске, ошибки и семантика не меняются. Учитывать 12 файлов-потребителей `mc_server::ServerConfig` и 5 файлов с его `to_network`, не путая последний с одноимёнными section-методами.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить config/ (Recommended).
- **Depends on:** L0-03 — сохранить тематические проверки config/access-control/network translation.
- **Cross-cut tag:** `responsibility-locality`

### L0-03 — Отделить тематические тесты от точек входа

**Evidence:** `crates/mc-server/src/main.rs:2498` и `crates/mc-server/src/lib.rs:1470`: `mod tests {`. Эти inline-блоки занимают суммарно около 2900 строк. Уже существуют sibling-модули `structure_rules_tests`, `component_startup_tests`, `access_control_file_tests`; `startup_data.rs:404–405` использует `#[path = "startup_data_tests.rs"] mod tests;`.

**Current state:** production-корни одновременно служат крупными коллекциями проверок CLI, TOML, запуска мира, pre-generation и файлов доступа. Загрузка корневого файла в контекст приносит большой объём несвязанных с текущей задачей fixtures.

**Desired state:** тесты остаются близко к соответствующим владельцам поведения, но не увеличивают объём чтения production-корня.

**Proposed improvement:** тематически перенести main-тесты в `main_tests/{cli,world_startup,pregeneration}.rs`, config-тесты — в `config/tests/{parsing,network}.rs`; access-control проверки собрать у access-control модуля. Добавить только необходимые test-module declarations; существующие assertions, fixtures, #[ignore] и сценарии не ослаблять. Точные подключения согласовать с L0-01/L0-02. Это организационный перенос, не новая архитектура и не доказательство поведения.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разнести tests/ по темам (Recommended).
- **Depends on:** none; production-переносы используют выбранную тематическую группировку.
- **Cross-cut tag:** `test-locality`

### L0-04 — Согласовать и сократить входной контекст

**Evidence:** `AGENTS.md:219`: `Workspace version is \`0.0.6\`; the release target is \`v0.0.6\`.`; `AGENTS.md:252`: `First-party Luau packages`; `Cargo.toml:6`: `version = "0.0.8"`; `crates/mc-server/src/main.rs:270`: `async fn prepare_component_deployment`. `docs/MEMORY.md:3–4` обещает маленький живой курсор, но файл содержит 2677 строк, включая секции с заголовком `Historical checkpoint` (например, строка 1255). `docs/ARCHITECTURE.md:3` прямо отмечает `Status: new target architecture`.

**Current state:** стартовые инструкции смешивают прежние версии/runtime-термины, действующий код, целевое устройство и подробную историю. Модель вынуждена сначала разрешать эти противоречия, а не изучать нужный домен.

**Desired state:** один короткий актуальный маршрут чтения, честно различающий текущий код, опубликованную версию, целевую архитектуру и исторические receipts.

**Proposed improvement:** сверить AGENTS/README с текущим кодом, не подменяя сведения об опубликованном релизе workspace-версией; оставить в MEMORY live cursor и ссылки, историю перенести в существующий `docs/memory/`. Обновить существующую карту `docs/AGENT_ROUTES.md` там, где runtime seams уже изменились. Не создавать параллельный набор архитектурных инструкций и не удалять historical evidence. Это ограниченная проверка конкретных входных утверждений, не полный аудит contextual-документов.

- **Severity:** High
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Сверить и сократить вход (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `current-state-navigation`

### Layer 0 — tally

| Status | Count |
|---|---:|
| accepted | 4 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: `test-locality`, `current-state-navigation`, `explicit-composition`, `responsibility-locality`.
Cross-cutting tags reused: none yet.
Dependency edges: L0-01 depends on L0-03; L0-02 depends on L0-03. L0-04 is independent.

## Layer 1 — Сеть и lifecycle

**Coverage:** `lib.rs` и `connection_driver.rs` прочитаны полностью в основном контексте. `server.rs` целиком прочитан отдельным read-only codebase-analyzer: диапазоны 1–1455, 1456–2851, 2852–4188, 4189–5488, 5489–6788, 6789–8063, 8064–9346, 9347–EOF (Read показывает последнюю строку 10044; wc — 10043). Основной контекст дополнительно проверил physics sampling и save-barrier фрагменты. Тесты прочитаны, не запускались. Дочерние модули — только reference/context checks.

**Consumer verification:** для `SaveHandle` / `SaveAllReport` / `SaveAllTimings` найдено 9 разных source-файлов-потребителей вне определения/facade; вместе с `save_periodic_checkpoint` и `save_all_after_simulation_barrier` — 12. Включены реальные production/test ссылки; excluded xtask string inventory и comment-only упоминания. Это explicit-token file counts, не counts inferred method calls или внешних checkout. Shared fixture `server::tests::save_all_test_config` имеет потребителей вне inline-блока и требует сохранения явного test-support seam.

**Ten-dimension sweep:** boundary — `BoundServer` владеет listener/simulation, handles предоставляют ограниченные возможности; public surface — сгруппированные facade exports полезны и не подлежат механическому удалению. Coherence/granularity/intention — ~5 тыс. строк production server совмещают supervision с physics adapter и full checkpoint machinery; ещё ~4,9 тыс. — inline tests. DRY — full save и pressure-only flush намеренно различны, общий framework не предложен. DDD/naming — owner fence, snapshot watermark и типизированные uncertainty-errors несут полезный смысл; устарела crate-level вводная. Error posture — не предлагается унификация ordinary disconnect и uncertain authority failures. Module graph — полезны существующие `entity_ticker`, `natural_spawn_ticker`, `runtime_control`, `operator_control`; root-imports при переносе должны стать явными.

**Keep:** `connection_driver.rs` читается как последовательность handshake → status либо login → configuration → play. Это полезная композиция; новый protocol-state framework не нужен. Порядок bind/recovery/drain остаётся видимым в supervisor; дробить его на generic builders не предлагается. Physics sampling не становится entity authority; full checkpoint не сливается с dirty-pressure flush.

### L1-01 — Извлечь цельный physics adapter из supervisor

**Evidence:** `crates/mc-net/src/server.rs:2826`: `fn prepare_entity_physics_inputs`; `:3808`: `fn step_sampled_entity`. Перед sampling строится опубликованный world snapshot, а перед apply проверяется его актуальность.

**Current state:** sampling, snapshot/cache, collision и projectile translation занимают самостоятельную группу внутри server supervisor.

**Desired state:** физический adapter имеет один видимый дом, не превращаясь в нового владельца entity state.

**Proposed improvement:** выделить `server/entity_physics.rs` с sampling/cache/stepping и существующими fences. Связанные helpers перемещаются вместе. Сохранить snapshot sharing, CPU permits, порядок результатов, owner/current-query fencing и передачу falling-block landing симуляции. Scheduled-block jobs не поглощать этим модулем. Поправить реальные imports `entity_ticker` и тематические тесты; не создавать micro-module на каждый helper.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Вынести physics adapter (Recommended).
- **Depends on:** L1-03 — тематическая test/support организация до production-переноса.
- **Cross-cut tag:** `responsibility-locality`

### L1-02 — Назвать и локализовать checkpoint boundary

**Evidence:** `crates/mc-net/src/server.rs:4586`: `let _save_guard = coordinator.lock().await;`; `:4725`: `async fn save_all_with_context_snapshot_locked_impl`.

**Current state:** save handles/reports, coordinator, bounded dirty-pressure flush и многофазный checkpoint находятся среди supervision-кода. Один полноразмерный save проходит barrier, write/install/sync/finalize и acknowledgement нескольких владельцев.

**Desired state:** последовательность сохранения читается в одном специализированном модуле; supervisor сохраняет видимый drain/final-save lifecycle.

**Proposed improvement:** выделить `server/checkpoint.rs`, сохранив текущие facade exports и различие full-save/pressure-only semantics. Не менять coordinator-before-barrier, освобождение locks перед blocking I/O, sync-before-finalize, conditional WAL checkpoint и acknowledgement ordering. Не объединять отчёты успешного owner commit и внешней доставки. Реализация в будущем требует owning ADR 0004/0005 review; текущее ревью не меняет persistence-политику и не доказывает её поведение.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal (файловая граница; on-disk format не меняется)
- **Class:** polish
- **Status:** **accepted** — Выделить checkpoint (Recommended).
- **Depends on:** L1-03 — сохранить тематические persistence/lifecycle tests и shared fixtures.
- **Cross-cut tag:** `explicit-composition`

### L1-03 — Разделить server tests по проверяемой обязанности

**Evidence:** `crates/mc-net/src/server.rs:5160`: `pub(crate) mod tests {`; `:8347`: shared fixture `save_all_test_config`. Inline-блок занимает около 4900 строк; часть collision/player-session tests уже подключается отдельными модулями на строках 5151–5157.

**Current state:** admission, physics, lifecycle, saving/recovery и security fixtures читаются единым блоком. Некоторые потребители обращаются к `server::tests`, поэтому слепое перемещение блока не закрывает навигационный долг.

**Desired state:** тесты группируются по ответственности; действительно общие fixtures имеют небольшой явный test-only дом.

**Proposed improvement:** `server/tests/{lifecycle,physics,checkpoint,admission}.rs` и маленький `server/test_support.rs` для используемых несколькими группами fixtures. Сохранить целиком каждый транзакционный сценарий, семантику assertions и существующие test flags. Реальные test-потребители переводятся на новые пути, production API ради тестов не расширяется.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить server/tests/ (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `test-locality`

### L1-04 — Crate introduction должна описывать текущий hub

**Evidence:** `crates/mc-net/src/lib.rs:7–10`: `At M1.c` и `The Login → Configuration → Play path arrives in M1.d / M1.e / M1.g.`; `connection_driver.rs:110–187` уже последовательно выполняет login, configuration и play.

**Current state:** crate-level вводная описывает ранний server-list ping и обещает ныне реализованные фазы в будущем. Она не называет фактическое присутствие gameplay/simulation integration в mc-net.

**Desired state:** краткая вводная показывает нынешнюю ответственность и отдельно отмечает целевой adapter-only cutover как ещё не завершённый.

**Proposed improvement:** исправить вводную и при необходимости Cargo description в той же документационной фазе, что L0-04. Никакого переименования crate или фиктивного утверждения об уже отделённой simulation.

- **Severity:** Med
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted (absorbed into L0-04)** — Включить в L0-04 (Recommended).
- **Depends on:** L0-04 (совместное выполнение, не отдельная фаза).
- **Cross-cut tag:** `current-state-navigation`

### Layer 1 — tally

| Status | Count |
|---|---:|
| accepted | 4 (включая 1 absorbed) |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `responsibility-locality`, `explicit-composition`, `test-locality`, `current-state-navigation`.
Dependency edges: L1-01 → L1-03; L1-02 → L1-03 (стрелка означает «depends on»). L1-04 исполняется внутри L0-04.

## Layer 2 — Gameplay

### Layer 2.1 — Connection-local gameplay

**Coverage:** `play.rs` полностью прочитан read-only analyzer, включая inline cfg(test): 1–1361, затем непрерывные диапазоны по 1000 строк 1362–15361 и 15362–EOF. Финальная декларация `mod tests;` — строка 15721; внешние test-модули не считаются прочитанными полностью. Главный контекст проверял InteractionState, food-use commit/resync, ingress context и outbound match точечно.

**Ten-dimension sweep:** boundary/public surface — root совмещает connection driver, gameplay adapters, simulation-owned world work и contracts/reexports; не весь его объём — policy. Coherence/granularity/intention — семь ingress Context/classifier/handler-групп уже определены на 12109–13355, тогда как `play_loop_inner` занимает ~731 строк, `handle` ~556. DRY — похожие container handlers имеют разные stale-state правила, общего универсального handler не предлагается. Domain language/naming — expected/updated и committed/projection полезны; `InteractionState.inventory` является connection baseline, не автоматически второй mutable authority. Error posture — malformed input, отказ gameplay с resync, slow-client closure и journal fail-stop не объединяются. Module graph — `super::*` в merchant/session/chunk_stream скрывает реальные root dependencies; новое извлечение требует явных imports.

**Consumer verification:** движение использует `PlayerMovementIngressContext` / `handle_accepted_absolute_movement` в одном другом файле (`movement_tests.rs`); семь classifiers — каждый в одном другом файле (`liveness.rs` test section). Остальные шесть Context, семь handlers и `refresh_live_permissions` не имеют иных source-файлов-потребителей вне root; root dispatch остаётся потребителем после переноса. `pub(crate)` surface данным переносом не затрагивается.

**Keep / correction to preliminary impression:** outbound match на 14677 — не только wire serialization: он также принимает damage, pickup, script inventory/teleport work. Поэтому механически называть его wire-only adapter неверно; такой перенос здесь не предлагается. World tick/journal coordination также не дробится по числу строк. `InteractionState` и общие gameplay helpers пока остаются на месте; authoritative owner results обновляют connection baseline и при commit, и при reject.

### L2.1-01 — Извлечь существующие ingress families

**Evidence:** `crates/mc-net/src/play.rs:12109`: `struct PlayerMovementIngressContext`; `:12639`: `async fn handle_serverbound_container`; `:12719`: `Keep the large click future out of the enclosing ingress/play-loop frames.` Существующий dispatch использует эти семейства на 15064–15280.

**Current state:** семь уже именованных Context/classifier/handler-групп находятся внутри 15,7-тысячного play root вместе с несвязанными world work и delivery concerns.

**Desired state:** семейство ingress находится по имени задачи и читается без остального play root; порядок работы и текущие authority seams остаются прежними.

**Proposed improvement:** `play/ingress.rs` с явными family exports и `play/ingress/{movement,player_state,use_interaction,containers,player_control,client_metadata,chat_commands}.rs`. Переместить существующие группы, а не вводить второй dispatcher. Оставить `tokio::select!`, admission/liveness ordering, keepalive/teleport special handling, boxed futures и gameplay helpers на текущих границах. Минимальная visibility — внутри play; не расширять crate API. Перенаправить classifiers из liveness tests и movement test contexts. Использовать явные imports вместо зависимости от root `super::*`.

- **Severity:** High
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить play/ingress/ (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `responsibility-locality`

### Layer 2.1 — tally

| Status | Count |
|---|---:|
| accepted | 1 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `responsibility-locality`. Dependency edges: none.

### Layer 2.2 — Simulation admission / ordering

**Coverage:** analyzer полностью прочитал `simulation.rs`: 1–1327; 1328–2727; 2728–4005; 4006–5280; 5281–6601; 6602–7614; 7615–8949; 8950–10176; 10177–11395; 11396–12653; 12654–13911; 13912–15217; 15218–16468; 16469–17787; 17788–19043; 19044–EOF. `save_barrier.rs` прочитан полностью (91 строк по Read). Основной контекст отдельно проверил private routing run с пятью bool на 4481–4582. Никаких исполнений.

**Ten-dimension sweep:** boundary — handle/request, routing policy и execution принадлежат одной существующей owner-модели; перенос impl не меняет владельца. Public surface — root contracts реально используются play/session/script adapters и частично переэкспортированы через play. Coherence/granularity/intention — 11,7 тыс. строк inline tests отделимы механически; run routing представлено позиционным tuple из пяти bool. DRY — похожие item response handlers обслуживают разные gameplay-транзакции и их postcommit publication; generic handler/framework не обоснован. Vocabulary/naming — Plan/Committed/Rejected/Precommit и typed warehouse outcomes полезны. Error posture — admission/owner lifecycle/structural validation/stale rejection не объединять. Module graph — `queue`, `request_wait`, `regional_mutation`, `precommit`, `save_barrier` уже представляют реальные seams, хотя используют root vocabulary.

**Consumer verification:** request types имеют production/test consumers; удаления или public-path migrations не предлагаются. Для потенциального moved `AnimalFeedTargets` найдено 3 внешних production-файла, но отдельная техническая корзина `item_interactions` не выбрана: сходство response-формы само по себе не доказывает общую доменную ответственность. Routing tuple/private predicates не являются public API. `simulation/tests/precommit_tests.rs` использует `super::*` и shared fixtures inline-блока; физическое перемещение тестов должно сохранить этот test-only seam.

**Keep:** save barrier сохраняет owner fence, даже после drop world storage; это не независимый persistence service. Queue admission, lease/phase acquisition, journal append, cancellation rechecks и postcommit publication остаются у существующих владельцев. Не извлекать все validators в бездоменный validation.rs и не вводить trait per command.

### L2.2-01 — Отделить тематические simulation tests

**Evidence:** `crates/mc-net/src/play/simulation.rs:8110`: `mod tests {`; `simulation/tests/precommit_tests.rs:1`: `use super::*`. Inline-тесты занимают около 11,7 тыс. из 19,8 тыс. строк корня.

**Current state:** чтение request/owner orchestration физически связано с большим набором routing, player-operation и durable-composite tests; вложенный precommit test-модуль использует shared fixtures этого блока.

**Desired state:** тесты находятся по проверяемой обязанности; root production-файл не содержит весь набор fixtures/scenarios.

**Proposed improvement:** тематические `simulation/tests/{routing,player_actions,world_edits,inventory_recovery}.rs`, с узким test-support seam только для общих fixtures. Существующие `precommit_tests`, `block_drop_tests`, `inventory_recovery_tests`, `player_teleport_tests` использовать/перегруппировать без дублей: выбранное имя группы не предписывает создание второй одноимённой suite. Не ослаблять сценарии потери requester, furnace newer-state preservation, TNT publication order, append-before-publish и recovery. Перенос тестов не означает изменения authority.

- **Severity:** High
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить simulation/tests/ (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `test-locality`

### L2.2-02 — Назвать поля execution run

**Evidence:** `crates/mc-net/src/play/simulation.rs:4495`: `Vec<(bool, bool, bool, bool, bool, Vec<SimulationCommandEnvelope>)>`; построение, сравнение и деструктуризация на 4497–4582 повторяют один позиционный порядок.

**Current state:** пять разных признаков маршрута представлены одинаковыми bool-позициями; смысл и соответствие приходится удерживать в памяти между grouping и execution.

**Desired state:** имя поля непосредственно показывает, какое условие маршрутизации проверяется, без новой routing-архитектуры.

**Proposed improvement:** одна private named struct для `requires_world`, `regional_block_edit`, `journaled_block_edit`, `block_drop_command`, `cross_region_plugin_edit` и последовательности envelopes. Сохранить predicates, grouping только соседних совместимых runs, branch precedence, sequence/order и existing executors. Не выделять `routing.rs`, не вводить новый scheduler или enum framework в этой рекомендации.

- **Severity:** Med
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Именованная private struct (Recommended).
- **Depends on:** none; при последовательном landing рядом с L2.2-01 не считать перемещение тестов поведенческой проверкой.
- **Cross-cut tag:** `explicit-state-vocabulary`

### Layer 2.2 — tally

| Status | Count |
|---|---:|
| accepted | 2 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: `explicit-state-vocabulary`. Reused: `test-locality`. Dependency edges: none.

### Layer 2.3 — Shared owner and player state

**Coverage:** read-only analyzer полностью прочитал session.rs непрерывными диапазонами 1–650, 651–1300, 1301–1950, 1951–EOF; `player_state.rs` и `player_item_action_authority.rs` — целиком. Основной контекст ранее читал structs/коммиты и повторно сверил точные anchors поиска. Дополнительные entity_owner/visibility/persistence/pickup/sleep/weather фрагменты — targeted reference checks, не расширение полного scope.

**Ten-dimension sweep:** boundary — Registry является широким coordinating facade, но не одним недифференцированным владельцем всего состояния. Public surface — 6 item-action commit methods имеют direct production consumer в одном файле simulation.rs и direct test consumers в двух; это не полный consumer audit всех методов Registry. Coherence/granularity — extracted impls по-прежнему обращаются к shared state; физическое разбиение уже есть, новой изоляции не создаёт. Intention/naming/domain vocabulary — одинаковое соседство owner handles, projections, indexes и world values затрудняет чтение фактических владельцев. DRY — нет основания дублировать авторитетные snapshots ради новой границы. Error posture — stale/inapplicable None, typed errors и rejection snapshots имеют разные роли. Module graph — player impls уже имеют explicit imports; root и некоторые siblings используют wildcard imports.

**Ownership correction:** `SessionRegistry.entities` — доступ к regional owners, а `published_entity_snapshots` — публикационная проекция. `lock_entities` конструирует owner-access wrapper, не берёт второй глобальный entity mutex. Player inventory хранится в shared `Arc<Mutex<PlayerPersistedState>>`; некоторые connection script/grant paths используют тот же Arc. Deadline maps — scheduling/claim indexes с последующей owner revalidation. World time/weather values и per-session last-broadcast markers также нельзя считать одной обязанностью. Поэтому автоматический вывод «всё это duplicate authority, надо разделить на actors» был бы неверен.

**Keep:** authoritative commit → projection update → delivery сохраняются; `SessionEntityGuards` не объявляется универсальной транзакцией через world/regional owners. Перенос реального player inventory ownership затронул бы connection, grants, persistence, death/drop, simulation и является отдельным redesign, не простой file split.

### L2.3-01 — Сделать действительное ownership различимым в исходнике

**Evidence:** `crates/mc-net/src/play/session.rs:609`: `entities: SessionEntityOwners`; `:458`: `published_entity_snapshots`; `:484`: `player_persistence: HashMap<SessionId, Arc<Mutex<PlayerPersistedState>>>`; `:862`: `struct SessionEntityGuards`. `player_state.rs:22` и `player_item_action_authority.rs:27` продолжают `impl SessionRegistry`.

**Current state:** один coordinating facade объединяет owner-access handles, shared authoritative player state, derived publications, scheduling indexes и world values. Типы несут часть различий, но краткого source-local описания writers/derived data/guard scope нет; одно лишь имя Registry или lock_entities легко прочитать неверно.

**Desired state:** модель узнаёт owner, писателя и назначение каждой смысловой группы до углубления в многочисленные impl-файлы и не принимает snapshot/index за duplicate authority.

**Proposed improvement:** короткий module-level контракт и смысловые группы полей: session membership; shared authoritative player state; regional owner access; visibility/publication projections; scheduling/claim indexes; world clock/weather; test-only probes. Указать, что lock_entities создаёт access wrapper, а combined guard не является универсальным atomic commit через regional/world storage. Сослаться на реальные commit/publication модули вместо подробного пересказа функций. Не вводить substate types, новые locks, actors или ownership cutover в этой рекомендации.

- **Severity:** High
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Карта ownership в коде (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `explicit-state-vocabulary`

### Layer 2.3 — tally

| Status | Count |
|---|---:|
| accepted | 1 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `explicit-state-vocabulary`. Dependency edges: none. Более широкий ownership redesign явно не выбран.

### Layer 2.4 — Domain adapter and local state machine

**Coverage:** `merchant_adapter.rs` и `containers/quickcraft.rs` полностью прочитаны в основном контексте. Три merchant entry functions имеют один внешний source-файл-потребитель — play.rs (imports и три реальные call sites). Quickcraft family имеет девять других source-файлов со ссылками в play subtree, включая façade и tests; это family-level reach, не per-symbol consumer count. Ничего не удаляется на основании этого поиска.

**Ten-dimension sweep:** boundary/coherence — merchant adapter переводит menu input в expected-state plans и применяет owner results; quickcraft — локальная state machine, без socket/world owners. Public surface — visibility ограничена play, внешняя API-миграция не нужна. Granularity/intention — три merchant действия в ~279 строках остаются цельным concern; дробление по формальному порогу 200 строк не нужно. DRY — repeated resync/clear paths несут локальную политику, generic container framework не предлагается. Domain naming — QuickCraftStep/Outcome явно называют состояния. Error posture — committed/rejected/runtime-unavailable сохранены как разные результаты. Module graph — единственный `use super::*` скрывает весь набор зависимостей merchant, хотя алгоритм уже вынесен.

**Keep / positive precedent:** `QuickCraftState` владеет только status/kind/slots и выдаёт `QuickCraftStep`; отдельный sibling test module. Это реальная bounded responsibility, а не файл с methods над чужим гигантским state. Сам merchant adapter также не нуждается в дроблении: проблема здесь — не длина, а неявные imports.

### L2.4-01 — Сделать imports merchant adapter явными

**Evidence:** `crates/mc-net/src/play/merchant_adapter.rs:1`: `use super::*;`; файл использует owner plans/results, window helpers и wire types из общего родительского scope.

**Current state:** функциональный concern вынесен удачно, но его конкретные зависимости не перечислены, что заставляет искать происхождение имён в большом play root.

**Desired state:** список imports показывает потребляемые контракты и helpers, не меняя responsibilities и public visibility.

**Proposed improvement:** заменить wildcard конкретными imports в merchant adapter; использовать реальные module paths там, где они доступны без искусственного расширения visibility. Сохранить обращения к root helpers, пока они действительно принадлежат root. Тот же приём применять внутри уже согласованных извлечений, без repo-wide wildcard campaign и без нового helpers/services слоя.

- **Severity:** Low
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Явные imports здесь (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `responsibility-locality`

### Layer 2.4 — tally

| Status | Count |
|---|---:|
| accepted | 1 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `responsibility-locality`. Dependency edges: none.

### Layer 2 — roll-up

| Status | Count |
|---|---:|
| accepted | 5 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Полностью прочитаны все 8 выбранных файлов gameplay. Практический итог — ingress extraction, thematic simulation tests, именованные routing fields, source-local ownership map и explicit merchant imports. Изъятие SessionRegistry state в новые actors/owners и wire-only extraction outbound не предписаны.

## Layer 3 — Plugin contract / host / SDK

**Coverage:** analyzer полностью прочитал mc-script/lib.rs диапазонами 1–1738, 1739–3123, 3124–4523, 4524–5880, 5881–7280, 7281–8680, 8681–EOF; mc-plugin-host/lib.rs, adapter.rs и SDK/lib.rs — unrestricted reads до EOF (382, 539, 579 строк по reader). Основной контекст отдельно сверил ledger/admitted wrapper/ScriptBoundary и manifest anchors. WIT/glue/instance/host/package — targeted context, не отдельный полный слой.

**Ten-dimension sweep:** boundary — WIT задаёт ABI; native constructors/admission принадлежат mc-script; Wasmtime execution/staging — host; SDK строит requests. Public surface — SDK generated bindings и host/native conversion entrypoints реально потребляются, не удаляются по низкому token count. Coherence/granularity — mc-script root объединяет DTOs, ledger/queue/routes/reload, manifest/capabilities и ~2 тыс. inline tests. Intention — configure/init separation и native ticket authority хорошо задокументированы. DRY — host conversion и native admission повторно проверяют разные доверительные границы, это не две authority. Domain vocabulary/naming — stable plugin/player UUID и session ID не смешивать; будущие RuntimeControls нельзя автоматически принять за текущие Wasmtime limits. Error posture — DTO/adapter/guest/queue errors различают стадии, массовая унификация не нужна. Module graph — host и SDK генерируют ABI из одного WIT; SDK не зависит от native mc-script Rust API, dev host edges не являются production cycles.

**Consumer verification:** `to_script_batch` имеет 10 других consumer source-файлов (2 production, 8 test), без definition/root reexport. Поиск семейства `ScriptPluginManifest`/`ValidatedScriptPluginManifest`/`ScriptPluginManifestError` дал 24 source-файла включая определение и tests; `mc-plugin-host/src/{package,host}.rs` — реальные production-потребители. Это explicit family-token reach, не per-symbol downstream census. Предлагаемый перенос сохраняет текущие root paths, не сужает public API.

**Keep:** один `HostAdmissionLedger` выдаёт и потребляет точные tickets; `AdmittedScriptCommand` не клонируется. Atomic batch admission не означает общую world transaction для разнородных команд. Не разносить issue/accept/reload/route publication по независимым services. Adapter уже использует domain-specific conversion modules; его ~539 строк не требуют самостоятельного generic converter framework. SDK export macro требует generated binding surface. Не переименовывать mc-script в рамках этого polish.

### L3-01 — Отделить native manifest vocabulary от admission execution

**Evidence:** `crates/mc-script/src/lib.rs:3494`: `struct HostAdmissionLedger`; `:4639`: `pub struct ScriptBoundary`; `:6033`: `pub struct ScriptPluginManifest`; `:6743`: `pub struct ValidatedScriptPluginManifest`.

**Current state:** native manifest, capabilities и их валидация находятся в том же большом root-файле, что ticket authority, очереди и reload publication.

**Desired state:** manifest/capability contract читается отдельно от исполнения trusted boundary, без второго validation/admission authority.

**Proposed improvement:** `mc-script/src/manifest.rs` с Manifest/ValidatedManifest, dependency/load-phase/capability vocabulary и относящимися к ним проверками. Сохранить root exports и opaque validated/provenance constructors; не расширять production visibility ради переноса или тестов. Existing `plugin_metadata.rs` сохраняет собственную package/resource роль: новый manifest module не дублирует его структуры. Ledger, issue/accept, queue и reload остаются вместе. WIT, plugin package metadata format, capabilities semantics и ABI не меняются; consumer paths host/package остаются действующими.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal (public facade paths сохраняются)
- **Class:** polish
- **Status:** **accepted** — Выделить native manifest (Recommended).
- **Depends on:** L3-02 — тематически сохранить contract/admission private tests.
- **Cross-cut tag:** `responsibility-locality`

### L3-02 — Сгруппировать root contract/admission tests

**Evidence:** `crates/mc-script/src/lib.rs:7733`: `mod tests {`; существующие отдельные domain test modules подключены на 127–156.

**Current state:** около 2 тыс. строк оставшихся tests/fixtures находятся в root рядом с ~7,7 тыс. строк production, хотя часть доменов уже имеет отдельные suites.

**Desired state:** тестовые примеры конкретной contract/admission обязанности находятся рядом и не дублируются между root и domain suites.

**Proposed improvement:** `manifest_tests.rs` и `boundary_tests.rs` для оставшихся соответствующих групп; domain cases присоединять к существующим suites. Оставить private-authority tests внутри логической privacy boundary, не делать constructors/ledger public для test access. Сохранить ticket identity, noncloneable admission, bounded batching и reload publication assertions.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Собрать tests по темам (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `test-locality`

### Layer 3 — tally

| Status | Count |
|---|---:|
| accepted | 2 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `responsibility-locality`, `test-locality`. Dependency edges: L3-01 depends on L3-02.

## Layer 4 — Entity ownership

**Coverage:** два read-only analyzer прочитали файлы полностью. lib.rs: 1–1796, 1797–3418, 3419–4882, 4883–EOF (закрывающая строка 5506). regional.rs: начальное чтение 1–1599, продолжения с 1600, 2800, 4086, 5344, 6644, 7838, 9113, 10374, 11674, 13078, 14453, 15826, 17182, 18510, 19874, 21069, 22353 до EOF (22366). Включены ignored benchmarks и disabled legacy-test block. Первый объединённый reader завершился transport error без результата; последующие два чтения закрыли весь scope. Основной контекст сверил module graph/anchors/consumer references.

**Ten-dimension sweep:** boundary — EntityStore является фасадом над EntityRuntime, а regional lanes физически владеют stores. Public surface — root reexports отделены от implementation location; перенос private kernel не затрагивает public types. Coherence/granularity — regional объединяет handle direct paths, runtime dispatcher, coordinator transactions, explicit-phase store, authority adapter и ~10,2 тыс. tests; root ещё ~2,1 тыс. tests. Intention — capture/resolve/apply отделяет probing от mutation. DRY — direct handle и coordinator оба участвуют в общей commit authority, их нельзя объединить как одинаковый RPC boilerplate. Domain vocabulary — lease epoch, phase, lifecycle epoch и journal sequence разные fences; retained state не равно persisted state. Naming — route cache содержит route/lease eligibility, не вторую entity population. Error posture — safe journal rollback и unknown outcome различаются намеренно. Module graph — owner_lane/tick/projections уже отдельны; новый generic owner layer не нужен.

**Consumer verification:** три regional facade names найдены в 8 source-файлах включая определение, reexport и tests; production consumers вне entity — session.rs и session/entity_owner.rs. Это family-token reach, не полный аудит всех региональных exports. Предложения не меняют этих путей. `bounded_pathing_step` имеет два direct callers внутри lib.rs; public types для его extraction не перемещаются.

**Keep / correction:** `RegionalOwnerHandle` — не только transport/projection facade: bounded direct routes совершают mutations через общий `Arc<RegionalOwnerCommitState>`. Runtime extraction ниже НЕ делает его sole mutation owner. Lease/admission order, prepare→commit→journal→finalize и topology/publication fences остаются неизменными. Relocation tests уменьшит regional.rs примерно на 46%, но оставит ~12 тыс. строк до test block: тесты не объясняют всю production сложность. Contiguous villager methods пока не разносить в новые domain services без необходимости.

### L4-01 — Разделить entity/regional tests по архитектурным обязанностям

**Evidence:** `crates/mc-entity/src/lib.rs:3384` и `crates/mc-entity/src/regional.rs:12168`: `mod tests {`; около 2,1 и 10,2 тыс. строк соответственно.

**Current state:** store/pathing и ownership/journal/direct-route/publication examples объединены в большие inline blocks; разные cfg/ignored группы соседствуют с production.

**Desired state:** нужный тестовый контракт находится тематически, при неизменных проверках и execution modes.

**Proposed improvement:** root `tests/{entity_store,goal_tick,pathing}.rs` и `regional/tests/{ownership,journal,direct_routes,publication}.rs` с компактным support и минимальным module wiring. Сохранить existing vehicle suite, все assertions, private access и `ignore`/`cfg(any())` режимы. Disabled legacy tests не объявлять действующей проверкой; benchmarks не превращать в default tests. Не расширять production visibility ради fixtures.

- **Severity:** High
- **Effort:** L
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Разделить tests по темам (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `test-locality`

### L4-02 — Выделить regional runtime lifecycle/dispatch concern

**Evidence:** `crates/mc-entity/src/regional.rs:1701`: `RegionalOwnerRuntime`; implementation 4728–5490 и связанные route-cache helpers до 5716.

**Current state:** запуск/join/shutdown, runtime command dispatch и route-cache publication находятся вместе со store и coordinator transaction policy.

**Desired state:** runtime lifecycle/dispatch читается как единая source-local обязанность, не искажая реальную authority topology.

**Proposed improvement:** `regional/runtime.rs` для существующих lifecycle/dispatcher и связанных publication helpers, с сохранением публичных root paths. Общий commit state, lock/admission order и lane ownership неизменны. Не объявлять runtime единственным mutation owner: direct handle paths продолжают общий commit/journal protocol. Краткий module contract обозначает это различие. Не вводить новый actor или универсальный dispatcher framework.

- **Severity:** High
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Выделить regional/runtime.rs (Recommended).
- **Depends on:** L4-01 — сохранить lifecycle/authority regressions при переносе.
- **Cross-cut tag:** `responsibility-locality`

### L4-03 — Локализовать private ground-probe kernel

**Evidence:** `crates/mc-entity/src/lib.rs:2928`: `fn bounded_pathing_step(`; coherent private helper block до 3291, два direct callers в root.

**Current state:** bounded ground probing/candidate generation делит большой root с EntityStore и entity vocabulary.

**Desired state:** алгоритм spatial probing имеет собственную точку чтения, сохраняя непосредственную связь с goal-resolution caller.

**Proposed improvement:** private `mc-entity/src/pathing.rs`; публичные `PathingProbe`/`PathingBudget` пути остаются прежними. Сохранить probe ordering/budget, retained-target lifecycle и aquatic delegation; не объединять с другой navigation policy лишь из-за общего слова pathing.

- **Severity:** Med
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Выделить private pathing.rs (Recommended).
- **Depends on:** L4-01 — сохранить тематические probing/budget assertions.
- **Cross-cut tag:** `responsibility-locality`

### Layer 4 — tally

| Status | Count |
|---|---:|
| accepted | 3 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `test-locality`, `responsibility-locality`. Dependency edges: L4-02 и L4-03 depend on L4-01.

## Layer 5 — Data / protocol / generation

**Coverage:** все четыре выбранных файла полностью прочитаны в основном контексте. mc-data/lib.rs, mc-protocol/lib.rs и mc-worldgen/lib.rs — unrestricted до EOF; terrain.rs — 1–1328, затем 1329–EOF (2653 по reader). После agent transport failure локальные чтения закрыли scope. Дочерние worldgen/data/protocol implementations не объявлены полностью проверенными.

**Ten-dimension sweep:** boundary — data indexes/sidecar loaders, codec/frame/packets и deterministic chunk assembly различимы. Public surface — root reexports сохраняют полезный entrypoint; narrowing/deletion не предлагается. Coherence — mc-data root сосредоточен на registry index/loading, прочие facts вынесены; terrain связывает column/cave/ore/structure/decor stages. Granularity/intention — явный `ChunkGenerator::generate` задаёт порядок, terrain tests уже в sibling modules. DRY — cached/uncached cave queries обеспечивают одну генерационную модель, это не повод вводить общий cache service. Domain vocabulary/naming — base surface/biome и plan-adjusted surface различены; comments объясняют recursion boundary village lookup. Error posture — required blocks fail construction, optional blocks имеют явные fallback semantics; loader errors не смешиваются с config. Module graph — terrain/biome_routing, ore_rules, overworld, trees и sediments уже задают частичные предметные границы.

**Keep:** не дробить численный алгоритм по произвольному LOC. terrain root ещё крупный, но pipeline и размещённые рядом cave/ore cache contracts дают читаемый маршрут; здесь не найдено первоочередного extraction, сопоставимого с play/regional. Public roots/data registry vocabulary не требуют реструктурирования только ради единообразия. В этом слое нет предложения перемещать/удалять public symbols; consumer census не служит основанием для изменений. WIT/packets/registry IDs, seed/noise order, WORLDGEN_REVISION и persisted chunks остаются вне polish.

### L5-01 — Актуализировать protocol/terrain entry comments

**Evidence:** `crates/mc-protocol/src/lib.rs:10–16`: «framing, packet structs … arrive in later sub-milestones»; `crates/mc-worldgen/src/terrain.rs:203–205`: «four … block types» (полная фраза пересекает строки).

**Current state:** вводные comments описывают раннюю реализацию, хотя frame/packets и существенно более богатый terrain generator уже присутствуют.

**Desired state:** первая точка чтения даёт актуальную карту реализации, а не историческое обещание будущих стадий.

**Proposed improvement:** включить эти два entry comment в L0-04. Перечислить текущие responsibilities и при необходимости ссылку на историю; не менять packet numbers, numeric algorithms, WORLDGEN_REVISION или save format.

- **Severity:** Low
- **Effort:** S
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted (absorbed into L0-04)** — Включить в L0-04 (Recommended).
- **Depends on:** L0-04 (same delivery, not a second patch).
- **Cross-cut tag:** `current-state-navigation`

### Layer 5 — tally

| Status | Count |
|---|---:|
| accepted | 1 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Accepted включает один absorbed finding. Cross-cutting tags introduced: none. Reused: `current-state-navigation`. L5-01 rides L0-04.

## Layer 6 — World storage

**Coverage:** lib.rs полностью в одном unrestricted read; storage.rs полностью: 1–1483, 1484–2772, 2773–4114, 4115–EOF (4263 по reader). Inline tests, ignored local-world probes и benchmark прочитаны, но не запускались. dirty_flush/read_view/resident bodies вне полного scope.

**Ten-dimension sweep:** boundary — WorldStorage координирует lease/load/admission/LRU, canonical resident storage и immutable read publications различены. Public surface — root reexports дают общий вход без требования знать storage internals; перенос/удаление public API не предлагается. Coherence/granularity — ~1,3 тыс. строк до inline tests, ~2,9 тыс. tests; budget/dirty_flush/read_view/world_lease/block_edits уже вынесены. Intention — load vs no-generation vs cached lookup названы отдельно. DRY — похожие block/fluid tick methods сохраняют предметные типы и publication obligations, generic tick framework не нужен. Domain vocabulary — dirty/journal-pending/read-only/absent не взаимозаменяемы. Naming — borrowed_chunk документирован как snapshot handle, не canonical store. Error posture — pressure deferral (Option/bool), absent chunks и IO/decoding failures осмысленно различаются. Module graph — façade направляет к resident and persistence concerns; inherited impl files не доказывают отдельных owners.

**Keep / positive precedent:** `storage.rs:159–165` явно описывает canonical resident, immutable read snapshots и scheduled-work hints; `:187–189` объясняет borrowed snapshot. Это готовый стиль source-local ownership map для L2.3-01. Не предлагать second cache owner или менять journal/flush protocol по одному facade. Consumer audit для переноса public symbols неприменим: их moved set пуст; runtime consumers не должны меняться при test-only action.

### L6-01 — Перенести storage tests по темам

**Evidence:** `crates/mc-world/src/storage.rs:1337`: `mod tests {`; `:41`: `mod test_support;`.

**Current state:** около 2,9 тыс. строк inline tests объединяют loading/cache, resident mutation и journal/flush при уже выделенных production submodules.

**Desired state:** каждый тестовый контракт читается отдельно от общего storage facade и соседних предметных suites.

**Proposed improvement:** `storage/{loading_tests,resident_mutation_tests,journal_tests}.rs`, reuse `test_support` и existing suites вместо дублирования fixtures. Сохранить round-trip, stale/atomicity, journal-pending, pressure, no-generation и ignored-local-world scenarios. Production ownership, lock order, Anvil bytes/save format и публичные paths не меняются.

- **Severity:** Med
- **Effort:** M
- **Blast radius:** internal
- **Class:** polish
- **Status:** **accepted** — Тематические storage tests (Recommended).
- **Depends on:** none.
- **Cross-cut tag:** `test-locality`

### Layer 6 — tally

| Status | Count |
|---|---:|
| accepted | 1 |
| rejected | 0 |
| deferred | 0 |
| withdrawn | 0 |

Cross-cutting tags introduced: none. Reused: `test-locality`. Dependency edges: none.

## Cross-cutting themes

Step 7: пользователь подтвердил группировку пяти тем. Все 20 finding IDs распределены ровно один раз.

### T1 — Локальность тестовых контрактов (active)

**Findings:** L0-03, L1-03, L2.2-01, L3-02, L4-01, L6-01.

Существенная часть крупных roots — tests/fixtures, но это не объясняет всю production сложность. Тематические sibling suites сокращают обязательный контекст чтения и дают примеры поведения рядом с ответственностью. Assertions, private access, ignored/disabled режимы и shared support сохраняются; сам перенос не считается поведенческой проверкой.

### T2 — Актуальный входной маршрут (active)

**Findings:** L0-04, L1-04, L5-01.

Убрать противоречия runtime/version/history в существующих entry docs и вводных comments, не создавать вторую документационную систему. L1-04 и L5-01 absorbed into L0-04: одна поставка, три source-grounded наблюдения. Тема не закрыта: пока принят отчёт, не выполнены исправления.

### T3 — Явная композиция и жизненный цикл (active)

**Findings:** L0-01, L1-02.

CLI/startup и save/checkpoint должны читаться как последовательность именованных этапов с видимым владельцем завершения. Выделяются существующие реализации, не появляются generic pipeline/DI frameworks и новые владельцы commit. Сохраняются startup order, save barrier/durability/finalize/ack sequence.

### T4 — Предметная локальность исходников (active)

**Findings:** L0-02, L1-01, L2.1-01, L2.4-01, L3-01, L4-02, L4-03.

Config I/O, physics adapter, ingress families, native manifest, regional runtime и private probing получают непосредственные точки чтения и явные imports. Это ограниченные internal cuts, не декомпозиция на новые crates/actors/services. Публичные facade paths сохраняются, а wildcard cleanup ограничен согласованными модулями.

### T5 — Явный словарь состояния и authority (active)

**Findings:** L2.2-02, L2.3-01.

Именованные routing fields и source-local ownership map уменьшают риск неверного прочтения bool-позиций, published snapshots и scheduling indexes. Ни named struct, ни новый файл с impl не создают новый runtime owner; допустимый объём здесь — представление существующей модели.

## Consolidated polish plan

Step 8: пользователь выбрал **Утвердить (Recommended)**; названия/номера/радиусы/effort фаз перенесены в frontmatter, межфазные semantic dependencies отсутствуют. `status: ready` означает готовность review к per-phase blueprint, не готовность runtime к релизу.

План ниже — scope для последующего per-phase blueprint, **не разрешение на реализацию**. Все изменения относятся к `polish`; новые runtime owners, actors, API/ABI и форматы сохранения не предлагаются. Порядок фаз отражает приоритет чтения, а не искусственную зависимость каждого слоя от предыдущего.

**О файлах:** перечислен плановый именованный контур (existing + new): 107 phase-path entries, 101 уникальный path, не уже изменённые файлы. Support/wiring и названия новых thematic suites blueprint уточняет по Rust privacy и реальным consumers, без новых обязанностей и без массовой правки соседей. Если перечисленный existing suite не нуждается в переносе cases/imports, менять его ради списка нельзя. Runtime facade reexports — единственный публичный путь, не compatibility implementation.

**Общие критерии будущей реализации:** сохранить assertions и execution modes; перечислить moved test identities, включая изменившиеся module paths, чтобы CLI filters не стали пустыми; проверить actual consumers после переноса. Source-only review не подтверждает сборку или gameplay. На последующей реализации — focused gates затронутого scope и канонический L2 на значимом завершении по AGENTS; клиентские gates нужны при затронутой client-visible поверхности, но не заменяются unit pass. Сейчас никакие gates не запускались и не предлагаются к запуску в рамках этого отчёта. Owning ADR в затронутых net/entity фазах обновляется только для действительных module-boundary изменений, без выдачи target migration за текущую архитектуру.

### Phase 1 — Актуальный вход и явный локальный словарь

**Findings:** 6 — L0-04, L1-04, L5-01, L2.3-01, L2.4-01, L2.2-02 (4 самостоятельных действия + 2 absorbed).
**Files touched:** 13 плановых paths:
  - `AGENTS.md`
  - `README.md`
  - `docs/MEMORY.md`
  - `docs/AGENT_ROUTES.md`
  - `docs/memory/2026-09-20-pre-review-history.md`
  - `crates/mc-net/src/lib.rs`
  - `crates/mc-net/Cargo.toml`
  - `crates/mc-protocol/src/lib.rs`
  - `crates/mc-worldgen/src/terrain.rs`
  - `crates/mc-net/src/play/session.rs`
  - `crates/mc-net/src/play/merchant_adapter.rs`
  - `crates/mc-net/src/play/simulation.rs`
  - `docs/decisions/0006-mc-net-module-boundaries.md`

**Blast-radius mix:** internal 6; public-API/on-disk/cross-module 0. **Coordination:** none. **Class mix:** polish 6 / redesign 0. **Effort:** M. **Depends on:** none.

**Delivery:** сначала current/release/target/history distinctions и ownership map, затем explicit merchant imports и named private routing struct. Archive path — предложенное место в существующем docs/memory, не второй live cursor; historical receipts сохраняются. Cargo description править только если действительно устарела, manifest dependencies/version не менять.

**Success criteria:** routing predicates, adjacent grouping и branch precedence неизменны; guard не описан как universal transaction; нет второго runtime truth. Документированный version/runtime соответствует проверенным источникам; symbols в imports имеют доступ без расширения public visibility. Введённые helper names объясняют существующее поведение, не маскируют новую policy.

### Phase 2 — Startup/config: тесты, затем локальные реализации

**Findings:** 3 — L0-03 → L0-01 и L0-02.
**Files touched:** 22 плановых paths:
  - `crates/mc-server/src/main.rs`
  - `crates/mc-server/src/lib.rs`
  - `crates/mc-server/src/startup.rs`
  - `crates/mc-server/src/startup/components.rs`
  - `crates/mc-server/src/startup/terrain.rs`
  - `crates/mc-server/src/startup/spawn.rs`
  - `crates/mc-server/src/startup/check.rs`
  - `crates/mc-server/src/config/mod.rs`
  - `crates/mc-server/src/config/access_control.rs`
  - `crates/mc-server/src/config/network.rs`
  - `crates/mc-server/src/main_tests.rs`
  - `crates/mc-server/src/main_tests/cli.rs`
  - `crates/mc-server/src/main_tests/world_startup.rs`
  - `crates/mc-server/src/main_tests/pregeneration.rs`
  - `crates/mc-server/src/config/tests.rs`
  - `crates/mc-server/src/config/tests/parsing.rs`
  - `crates/mc-server/src/config/tests/network.rs`
  - `crates/mc-server/src/access_control_file_tests.rs`
  - `crates/mc-server/src/component_startup_tests.rs`
  - `crates/mc-server/src/structure_rules_tests.rs`
  - `crates/mc-server/src/console.rs`
  - `docs/AGENT_ROUTES.md`

**Blast-radius mix:** internal 3; остальные 0. **Coordination:** none. **Class mix:** polish 3 / redesign 0. **Effort:** L. **Depends on:** none (внутрифазные edges показаны выше).

**Delivery:** тематические tests сначала, затем два независимых production cuts; CLI/serve сохраняет явную композицию. Access-control tests при необходимости подключаются внутри нового owner module с прежним physical file, не через publicized helpers.

**Success criteria:** public ServerConfig paths/defaults/translation и config/file formats прежние; PreparedComponent/take_host/Drop имеют того же shutdown owner; startup validation/deployment/order и access-file lock/normalization/metadata/durable replace сохранены. Tests перенесены без потери сценариев; console/root consumers разрешаются. StartupData не дробится.

### Phase 3 — Supervisor: physics и checkpoint

**Findings:** 3 — L1-03 → L1-01 и L1-02.
**Files touched:** 14 плановых paths:
  - `crates/mc-net/src/server.rs`
  - `crates/mc-net/src/server/entity_ticker.rs`
  - `crates/mc-net/src/server/entity_physics.rs`
  - `crates/mc-net/src/server/checkpoint.rs`
  - `crates/mc-net/src/server/tests.rs`
  - `crates/mc-net/src/server/tests/lifecycle.rs`
  - `crates/mc-net/src/server/tests/physics.rs`
  - `crates/mc-net/src/server/tests/checkpoint.rs`
  - `crates/mc-net/src/server/tests/admission.rs`
  - `crates/mc-net/src/server/test_support.rs`
  - `crates/mc-net/src/play/persistence/inventory_recovery_tests.rs`
  - `docs/decisions/0004-staged-single-writer-simulation.md`
  - `docs/decisions/0005-regional-simulation.md`
  - `docs/decisions/0006-mc-net-module-boundaries.md`

**Blast-radius mix:** internal 3; остальные 0. **Coordination:** none. **Class mix:** polish 3 / redesign 0. **Effort:** L. **Depends on:** none.

**Delivery:** test/support seam, затем cohesive physics adapter и checkpoint module; root supervision/drain остаётся видимым. Imports private consumers корректируются, внешние save-handle/report paths сохраняются.

**Risk fence:** касается persistence orchestration, но **не** on-disk-format migration. Если перенос требует новой lock/transaction policy, это выходит за scope и требует нового решения пользователя.

**Success criteria:** snapshot/current-query fencing, CPU permits, result ordering, falling-block handoff сохранены. Coordinator→barrier→write/install/sync/finalize→ack order не меняется; locks не удерживаются дольше, full save не объединяется с pressure flush. save_all_test_config потребители работают через узкий test-support seam, без obsolete duplicates.

### Phase 4 — Семейства play ingress

**Findings:** 1 — L2.1-01.
**Files touched:** 12 плановых paths:
  - `crates/mc-net/src/play.rs`
  - `crates/mc-net/src/play/ingress.rs`
  - `crates/mc-net/src/play/ingress/movement.rs`
  - `crates/mc-net/src/play/ingress/player_state.rs`
  - `crates/mc-net/src/play/ingress/use_interaction.rs`
  - `crates/mc-net/src/play/ingress/containers.rs`
  - `crates/mc-net/src/play/ingress/player_control.rs`
  - `crates/mc-net/src/play/ingress/client_metadata.rs`
  - `crates/mc-net/src/play/ingress/chat_commands.rs`
  - `crates/mc-net/src/play/liveness.rs`
  - `crates/mc-net/src/play/movement_tests.rs`
  - `docs/decisions/0006-mc-net-module-boundaries.md`

**Blast-radius mix:** internal 1; остальные 0. **Coordination:** none. **Class mix:** polish 1 / redesign 0. **Effort:** M. **Depends on:** none.

**Success criteria:** семь существующих family contracts доступны по именам; classifiers/test contexts перенаправлены. select/admission/liveness, keepalive/teleport branches и boxed futures сохранены. Не добавлен второй dispatcher, не расширена crate visibility. Outbound gameplay work не объявлено pure wire serialization, authoritative commit и connection resync остаются на прежних границах.

### Phase 5 — Локальность simulation tests

**Findings:** 1 — L2.2-01.
**Files touched:** 11 плановых paths:
  - `crates/mc-net/src/play/simulation.rs`
  - `crates/mc-net/src/play/simulation/tests.rs`
  - `crates/mc-net/src/play/simulation/test_support.rs`
  - `crates/mc-net/src/play/simulation/tests/routing.rs`
  - `crates/mc-net/src/play/simulation/tests/player_actions.rs`
  - `crates/mc-net/src/play/simulation/tests/world_edits.rs`
  - `crates/mc-net/src/play/simulation/inventory_recovery_tests.rs`
  - `crates/mc-net/src/play/simulation/block_drop_tests.rs`
  - `crates/mc-net/src/play/simulation/player_teleport_tests.rs`
  - `crates/mc-net/src/play/simulation/tests/precommit_tests.rs`
  - `docs/decisions/0006-mc-net-module-boundaries.md`

**Blast-radius mix:** internal 1; остальные 0. **Coordination:** none. **Class mix:** polish 1 / redesign 0. **Effort:** M. **Depends on:** none.

**Delivery:** inventory/recovery group использует существующий inventory_recovery_tests.rs вместо второй suite; root production orchestration не дробится в этой фазе.

**Success criteria:** preserved requester-loss, furnace newer-state, TNT ordering, append-before-publication и recovery scenarios; precommit fixtures доступны с минимальным test-only scope. Runtime authority и save barrier не меняются. Private routing struct из Phase 1 не является обязательным условием test relocation.

### Phase 6 — Native manifest отдельно от admission

**Findings:** 2 — L3-02 → L3-01.
**Files touched:** 11 плановых paths:
  - `crates/mc-script/src/lib.rs`
  - `crates/mc-script/src/manifest.rs`
  - `crates/mc-script/src/manifest_tests.rs`
  - `crates/mc-script/src/boundary_tests.rs`
  - `crates/mc-script/src/player_inventory_tests.rs`
  - `crates/mc-script/src/player_teleport_tests.rs`
  - `crates/mc-script/src/entity_interaction_tests.rs`
  - `crates/mc-script/src/host_control_tests.rs`
  - `crates/mc-script/src/precommit_tests.rs`
  - `crates/mc-script/src/custom_payload_tests.rs`
  - `crates/mc-script/src/player_query_tests.rs`

**Blast-radius mix:** internal 2; остальные 0. **Coordination:** none — sibling SDK/package release не требуется. **Class mix:** polish 2 / redesign 0. **Effort:** M. **Depends on:** none.

**Success criteria:** Manifest/ValidatedManifest/capabilities и их проверки имеют один native дом; root facade paths и opaque constructors сохранены. Ledger/issue/accept/queue/reload publication остаются единым authority cluster. Ticket identity, single-use admission и atomic batch-admission tests сохранены без объявления общей world transaction. Host adapter повторно проверяет native контракты, SDK/WIT/Wasmtime не меняются. Existing domain suites получают только принадлежащие им cases, не общую тестовую свалку.

### Phase 7 — Entity: тестовые контракты и две локальные границы

**Findings:** 3 — L4-01 → L4-02 и L4-03.
**Files touched:** 17 плановых paths:
  - `crates/mc-entity/src/lib.rs`
  - `crates/mc-entity/src/regional.rs`
  - `crates/mc-entity/src/tests.rs`
  - `crates/mc-entity/src/tests/entity_store.rs`
  - `crates/mc-entity/src/tests/goal_tick.rs`
  - `crates/mc-entity/src/tests/pathing.rs`
  - `crates/mc-entity/src/test_support.rs`
  - `crates/mc-entity/src/regional/tests.rs`
  - `crates/mc-entity/src/regional/tests/ownership.rs`
  - `crates/mc-entity/src/regional/tests/journal.rs`
  - `crates/mc-entity/src/regional/tests/direct_routes.rs`
  - `crates/mc-entity/src/regional/tests/publication.rs`
  - `crates/mc-entity/src/regional/tests/vehicle_transfer_tests.rs`
  - `crates/mc-entity/src/regional/test_support.rs`
  - `crates/mc-entity/src/regional/runtime.rs`
  - `crates/mc-entity/src/pathing.rs`
  - `docs/decisions/0005-regional-simulation.md`

**Blast-radius mix:** internal 3; остальные 0. **Coordination:** none. **Class mix:** polish 3 / redesign 0. **Effort:** L. **Depends on:** none.

**Delivery:** самый большой test move; blueprint разбивает внутри на bounded slices без изменения всей authority model. Test relocation предшествует runtime и probe cuts.

**Risk fence / success criteria:** direct handle mutations и coordinator используют прежнюю shared commit authority; route caches не становятся mutable entity owners. Lease/phase/lifecycle/sequence fences, topology/admission lock order, journal rollback/unknown outcome и shutdown store recovery сохранены. Ground probe order/budget/retained-target/aquatic boundary прежние. Public root exports и disabled/ignored modes не меняются; не требуется новый runtime или отдельные actors на domain methods.

### Phase 8 — Storage tests без изменения persistence

**Findings:** 1 — L6-01.
**Files touched:** 7 плановых paths:
  - `crates/mc-world/src/storage.rs`
  - `crates/mc-world/src/storage/loading_tests.rs`
  - `crates/mc-world/src/storage/resident_mutation_tests.rs`
  - `crates/mc-world/src/storage/journal_tests.rs`
  - `crates/mc-world/src/storage/test_support.rs`
  - `crates/mc-world/src/storage/admission_tests.rs`
  - `crates/mc-world/src/storage/dirty_flush_tests.rs`

**Blast-radius mix:** internal 1; остальные 0. **Coordination:** none. **Class mix:** polish 1 / redesign 0. **Effort:** M. **Depends on:** none.

**Success criteria:** preserved load/no-generation/cache-pressure, stale atomic mutation, journal-pending/exact-clear и flush/reopen round trips. Reuse существующих fixtures/dirty-flush tests, ignored real-world probes не объявлены исполненными. Canonical resident storage/read snapshots/borrowed snapshot contracts, locks и Anvil bytes неизменны.

### Dependency graph

Межфазных semantic dependencies нет; обязательные finding-level edges замкнуты внутри фаз:

```text
P1: L0-04 включает L1-04 + L5-01; остальные 3 действия независимы
P2: L0-03 ──► L0-01, L0-02
P3: L1-03 ──► L1-01, L1-02
P4: L2.1-01
P5: L2.2-01
P6: L3-02 ──► L3-01
P7: L4-01 ──► L4-02, L4-03
P8: L6-01
```

Стрелки здесь означают «выполнить до». Рекомендуемый landing order P1→P2→P3→P4→P5→P6→P7→P8 — порядок приоритетов, не обязательная цепочка. Независимость не разрешает параллельную запись: P1/P2 делят routes, P1/P5 — simulation.rs, несколько фаз — ADRs. Реализацию/blueprint нельзя запускать автоматически.

## Independent report check

Read-only reviewer полностью прочитал draft (1–914), проверил finding/theme/phase coverage, counts и dependency directions. **Verdict: pass; findings: none.** Подтверждены 20 accepted / 18 independent actions, 5 тем, 8 фаз, 107 path entries / 101 unique planned paths. Reviewer не менял файлы и не запускал проектный код; это проверка согласованности отчёта, не повторное полное source review и не runtime validation. На момент проверки phase approval ещё ожидалось; затем пользователь утвердил тот же план без изменения границ или порядка.

## Closeout and next step

Изменён только этот Markdown-отчёт. Исходники, конфигурация, Git metadata, коммиты и публикации не менялись этим ревью; builds/tests/harness/client/scripts не запускались. Статические проверки: полное чтение выбранного scope по указанным ranges, targeted references/anchors, сверка tallies, уникальных plan paths, finding routing и body/frontmatter phases. Git base `1d39e864` — supplied, не independently verified; diff hash не собирался. Никакого runtime pass не заявлено.

Для отдельной, явно запрошенной следующей стадии: `/skill:blueprint .rpiv/artifacts/architecture-reviews/2026-09-20_18-44-21_solaris-model-comprehensibility.md` с аргументом `Implement Phase 1: Актуальный вход и явный локальный словарь`. Для каждой следующей фазы — отдельный вызов; желательно свежая сессия `/new`. Автоматическая цепочка не запускалась.
