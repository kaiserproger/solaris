# M6 session — kick-off prompt

Скопируй этот файл целиком в начало новой Claude-сессии. Промпт self-contained: ничего из текущей сессии не помнится, кроме того, что в `~/.claude/projects/-home-kaiserroman-solaris/memory/`.

---

## 1. Контекст

Ты ассистент кайзера на проекте **Solaris** — Rust-сервер для Minecraft Java Edition 26.1.2. Я только что закрыл M5 (chunk modification: break + place + dynamic light) и тегнул `m5` на `main`. Хочу спланировать и реализовать **M6**.

Поэтому первое, что сделай **до любого кода**:

1. Прочитай `MEMORY.md` целиком и подтянутые из него файлы (`project-status`, `feedback-*`, `reference-*`, `project-adrs`). Они описывают как со мной работать, последние решения, и где остановился проект.
2. Прочитай `docs/PROJECT_SPEC.md` (или его §9 «Milestone roadmap» как минимум).
3. Прочитай `docs/milestones/M5.md` целиком (а особенно "What landed where", "Status / validation appendix", "Open follow-ups").
4. `git log --oneline main..HEAD` (должно быть пусто — мы на main, `m5` уже тегнут).

После этого ты будешь знать: где мы стоим, что я не люблю (см. feedback-memory), и какие хвосты M5 явно передал в M6.

## 2. Что должен сделать ты в этой сессии

### Step 1. Scope-предложение по M6

Spec §9 формально называет:
- **M5 = «Block physics + fluids» (80-150h)** — gravity, water/lava flow, basic block updates.
- **M6 = «Player actions» (60-100h)** — block break/place, inventory, basic survival.
- **M7 = «First playable demo» (80-150h)** — can run/explore/survive.

**Но фактически:** M4 = lighting, M5 = chunk-modification = break/place. Мы съели обе половины опции «C» из M4-kick-off, плюс spec'овский M6 (block break/place). По-настоящему **не-сделанные** биг-куски — worldgen (spec M4) и block physics + fluids (spec M5). Плюс много мелких follow-up'ов.

Открытые follow-up'ы, унаследованные после M5 (полный список в `docs/milestones/M5.md` § Open follow-ups):

1. **Worldgen baseline.** Spec'овский M4 (120-180h). Terrain noise, biomes, simple structures. Без него мир обрывается на границе test-world'а.
2. **Persistence к `.mca`.** M5 держит edit'ы только в LRU; на выходе всё откатывается. Extras-channel из M5.c уже подложен под lossless round-trip.
3. **Inventory + held-item + hotbar.** Сейчас place всегда кидает stone. Реальный hotbar/слот/Container packets — это full milestone сам по себе.
4. **Block physics + fluids.** Spec'овский M5 (80-150h). Gravity, water/lava flow, simple block updates типа sand falling.
5. **Incremental relight.** Edit лагает ~150ms в debug потому что мы пересчитываем 5 чанков с нуля. Diff-based BFS убьёт лаг.
6. **Survival break-timing + tool damage.** Сейчас break сразу применяется на START_DESTROY_BLOCK независимо от типа блока/инструмента.
7. **`SectionBlocksUpdate` packet** для bulk-edit'ов.
8. **M3 carry-over'ы (всё ещё)**: spiral chunk-iteration, Set Compression, LZ4 read path, `SetDefaultSpawnPosition`.
9. **Reach validation / anti-cheat.**

**Твоя первая задача:** прочитай контекст и предложи мне 2-3 варианта scope для M6 (1-2 предложения каждый, плюс одна строчка про tradeoff). Например:

- **A. M6 = Persistence + Inventory.** Edit'ы переживают рестарт, place использует hotbar-слот клиента. Логичный шаг после M5: делает игру «alpha-ready».
- **B. M6 = Worldgen baseline (по spec'у M4).** Мир становится бесконечным. Большой милестон, но spec этого ждёт давно.
- **C. M6 = Block physics + fluids (по spec'у M5).** Gravity, sand-fall, water-spread. Делает мир живым.
- **D. M6 = perf + polish.** Incremental relight, spiral iteration, Set Compression, SetDefaultSpawnPosition, persistence-light. Закрывает накопившиеся хвосты без больших новых фич.

Выдай варианты, **жди моего сигнала**, какой берём. Не пиши `docs/milestones/M6.md` и не пиши код, пока не согласовали scope.

### Step 2. После согласования

Когда я выбрал вариант:

1. Создай ветку `dev/M6-<short-name>` из `main` (там `m5` уже тегнут).
2. Напиши `docs/milestones/M6.md` по образцу `docs/milestones/M5.md`: цель, стратегия, oracle-ссылки (если нужны — javap-дамп, wiki.vg), sub-милестоны (M6.a, M6.b, …), acceptance criteria (включая mechanical-verifiable и manual-gate, если уместен), pitfalls, open questions. Закоммить **только** план как `docs: M6 plan — <title>`. Жду ревью.
3. Когда я ок'нул план — реализуй sub-милестоны по одному коммиту на каждый, в том же стиле что M5.a–M5.g (`feat:` / `refactor:` / `docs:` префиксы, conventional commits).
4. Каждый коммит должен оставлять `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` зелёными. Если не зелёные — фикси в том же коммите или в следующем `fix:`, **не оставляй сломанный baseline**.
5. CI-gate в `mc-test-harness/tests/` если уместен (как M3.g / M4.f / M5.f).
6. Manual-gate (если уместен): я сам соединяюсь PrismLauncher 26.1.2 клиентом. Когда milestone готов к manual-gate, поднимай сервер через `cargo run --bin mc-server -- --config example.toml` (debug-сборка — release застревала в прошлой сессии) и говори «готов, подключайся».
7. По завершении — обнови `docs/milestones/M6.md` («What landed where» + status appendix), `README.md` (Status paragraph), и `memory/project-status.md` (через memory-tool). Не merge'й в main и не тегай — это моя работа (либо я сам, либо явно попрошу тебя — как в конце M4/M5).

## 3. Что точно НЕ делай без моего разрешения

- Не push на удалёнку. Не merge в `main`. Не ставь тег `m6`.
- Не редактируй файлы вне рабочего scope M6 без объяснения.
- Не апдейть `Cargo.lock` без причины.
- Не пушай данные Mojang (`data/vanilla/*`, `.analysis/*`) — gitignore'ы уже стоят, не ослабляй их.
- Не используй release-сборку для дев-цикла — debug всегда.
- Не задавай мне вопросы каждые 5 минут. Если затупил — попробуй сам, и только потом спрашивай конкретно.

## 4. Технические заметки (унаследовано из M3-M5)

- **Rust toolchain:** `1.94` пин в `rust-toolchain.toml`. Если cargo выдаёт `could not compile sharded-slab` или `cannot find module u32x4x2_avx2` — toolchain полу-удалён, делай `rustup toolchain uninstall 1.94 && rustup toolchain install 1.94 --profile minimal -c rustfmt -c clippy`. Это известная ловушка, не паникуй.
- **Git identity:** локально `kaiserproger <kaisergrobe@gmail.com>` (см. `feedback-git-author` memory). НЕ меняй `git config`.
- **example.toml** указывает на `.analysis/test-world` для демо.
- **Test-world geometry:** flat preset, bedrock Y=-64, dirt Y=-63..-62, grass Y=-61. `SPAWN_Y` в `mc_net::play` хардкоден на `-59.0`. Если M6 трогает спавн или геометрию мира — учти.
- **`.analysis/server.jar`** — bundle vanilla jar; `tools/dump-vanilla-protocol.sh` пишет javap-дамп в `.analysis/protocol-dump.txt`. Packet-ID цитируй через javap, не угадывай (ADR 0002).
- **`.analysis/protocol-dump.txt`**: ID-таблицы для game-CB и game-SB уже есть. Если M6 нужен новый packet — посмотри там перед тем как лезть в javap.
- **`data/vanilla/reports/block_light.json`** (M4.b) — per-state emission/opacity/sky-prop, опционально (если нет — load fall-back на `LightData::empty()`).
- **wire-probe** в `crates/mc-test-harness/src/bin/wire_probe.rs` подключается к реальному вандилла-серверу и дампит фреймы — полезно если M6 требует новых packet-захватов.
- **`mc_test_harness::client::Client`** — типизированный async-драйвер протокола; используй в M6 интеграционных тестах (см. tests/chunk_stream.rs и tests/block_edit.rs как образцы).
- **Закрытые M2 follow-up'ы (после M5):** opaque extras channel (#2), direct palette growth (#3), heightmap recompute (#5). Остались открытыми (#1) region cache и (#4) LZ4 read path.
- **Engine relight workspace:** `mc_world::light::LightWorkspace` reusable, см. M4.f. M5.d/e уже использует. Если M6 трогает relight — pool через workspace.

## 5. Стиль

Кратко и по делу. См. memory `feedback-terse-no-stalls`:
- сообщения мне в 1-3 предложения, не простыни,
- если что-то идёт >5 минут (компиляция, поиск, исследование) — скажи, что запустил, и не ждём,
- не «сейчас я проанализирую…» — делай и докладывай результат.

Никаких комментариев типа «added for M6», «used by X» в коде — см. CLAUDE.md / system prompt: identifiers сами говорят что делают, комментарии только когда «почему» неочевидно.

## 6. Старт

Прочитай memory, прочитай M5.md, прочитай spec §9, и пиши: «прочитал, предлагаю варианты scope M6: …». Жду.
