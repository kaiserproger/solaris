# M4 session — kick-off prompt

Скопируй этот файл целиком в начало новой Claude-сессии. Промпт self-contained: ничего из текущей сессии не помнится, кроме того, что в `~/.claude/projects/-home-kaiserroman-solaris/memory/`.

---

## 1. Контекст

Ты ассистент кайзера на проекте **Solaris** — Rust-сервер для Minecraft Java Edition 26.1.2. Я только что закрыл M3 (chunk streaming + Update Tags) и тегнул `m3` на `main`. Хочу спланировать и реализовать **M4**.

Поэтому первое, что сделай **до любого кода**:

1. Прочитай `MEMORY.md` целиком и подтянутые из него файлы (`project-status`, `feedback-*`, `reference-*`, `project-adrs`). Они описывают как со мной работать, последние решения, и где остановился проект.
2. Прочитай `docs/PROJECT_SPEC.md` (или его §9 «Milestone roadmap» как минимум) и `docs/CLAUDE_CODE_PROMPTS.md` (там шаблон промпта milestone-документа).
3. Прочитай `docs/milestones/M3.md` (особенно «What landed where», «Status / validation appendix», «Open follow-ups (handed to M4)»).
4. `git log --oneline main..HEAD` (должно быть пусто — мы на main, `m3` уже тегнут).

После этого ты будешь знать: где мы стоим, что я не люблю (см. feedback-memory), и какие хвосты M3 явно передал в M4.

## 2. Что должен сделать ты в этой сессии

### Step 1. Scope-предложение по M4

Спека (§9) формально называет **M4 = «Worldgen baseline» (120-180h)** — terrain noise, biomes, simple structures, no vanilla parity. Но текущая практика реальная: предыдущие милестоны мы делали более узкими (M3 формально был «Empty world ready, walk, no crash for 1 hour», а по факту — chunk streaming, и закрывал acceptance criterion 6 через M3.i, который изначально не планировался).

Открытый список follow-ups, которые M3.md явно передал в M4:
1. Реальное освещение. С `LightData::empty()` вандилла-клиент рендерит лит только под игроком, остальное чёрное. **Это самая заметная дыра** для пользователя сейчас.
2. Worldgen. Сейчас мы стримим только то, что есть в `.mca`; за пределами региона — `None` (клиент видит «провал»).
3. Opaque "extras" channel для полей чанка, которые M2-кодек дропает (structures, block/fluid ticks, PostProcessing, per-section light, InhabitedTime, LastUpdate, DataVersion).
4. Direct-mode рост палитры сверх 256.
5. LZ4 read path.
6. Heightmap recompute.
7. Set Compression.
8. Spiral chunk-iteration order.
9. SetDefaultSpawnPosition re-introduction.

**Твоя первая задача:** прочитай контекст и предложи мне 2-3 варианта scope для M4 (1-2 предложения каждый, плюс одна строчка про tradeoff). Например:

- **A. M4 = Lighting** — узко, быстро, закрывает самую заметную пользовательскую дыру; worldgen откладывается на M5.
- **B. M4 = Worldgen baseline (по спеке)** — большой milestone, делает мир бесконечным, но освещение остаётся пробитым.
- **C. M4 = Lighting + чанк modification («can break a block»)** — комбинация, позволяет M4 действительно реализовать «can walk and interact» как изначально хотел spec M3.

Выдай варианты, **жди моего сигнала**, какой из них берём. Не пиши `docs/milestones/M4.md` и не пиши код, пока не согласовали scope.

### Step 2. После согласования

Когда я выбрал вариант:

1. Создай ветку `dev/M4-<short-name>` из `main` (там `m3` уже тегнут).
2. Напиши `docs/milestones/M4.md` по образцу `docs/milestones/M3.md`: цель, стратегия, oracle-ссылки (если нужны — javap-дамп, wiki.vg, и т.д.), sub-милестоны (M4.a, M4.b, …), acceptance criteria (включая mechanical-verifiable и manual-gate, если уместен), pitfalls, open questions. Закоммить **только** план как `docs: M4 plan — <title>`. Жду ревью.
3. Когда я ок'нул план — реализуй sub-милестоны по одному коммиту на каждый, в том же стиле что M3.a–M3.i (`feat:` / `refactor:` / `docs:` префиксы, conventional commits).
4. Каждый коммит должен оставлять `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` зелёными. Если не зелёные — фикси в том же коммите или в следующем `fix:`, **не оставляй сломанный baseline**.
5. CI-gate в `mc-test-harness/tests/` если уместен (как M3.g).
6. Manual-gate (если уместен): я сам соединяюсь PrismLauncher 26.1.2 клиентом. Когда milestone готов к manual-gate, поднимай сервер через `cargo run --bin mc-server -- --config example.toml` (debug-сборка — release застревала в прошлый раз) и говори «готов, подключайся».
7. По завершении — обнови `docs/milestones/M4.md` («What landed where» + status appendix), `README.md` (Status paragraph), и `memory/project-status.md` (через memory-tool). Не merge'й в main и не тегай — это моя работа.

## 3. Что точно НЕ делай без моего разрешения

- Не push на удалёнку. Не merge в `main`. Не ставь тег `m4`.
- Не редактируй файлы вне рабочего scope M4 без объяснения.
- Не апдейть `Cargo.lock` без причины (если новых depend нет — `cargo update` не нужен).
- Не пушай данные Mojang (`data/vanilla/*`, `.analysis/*`) — gitignore'ы уже стоят, но не ослабляй их.
- Не используй release-сборку для дев-цикла — она медленная и **в прошлой сессии она застряла**, я её прибил. Debug build всегда.
- Не задавай мне вопросы каждые 5 минут. Если затупил — попробуй сам разобраться, и только потом спрашивай конкретно.

## 4. Технические заметки (унаследовано из M3)

- **Rust toolchain:** `1.94` пин в `rust-toolchain.toml`. В прошлой сессии rustup self-update сломал toolchain — если cargo выдаёт `could not compile sharded-slab` или `cannot find module u32x4x2_avx2` — toolchain полу-удалён, делай `rustup toolchain uninstall 1.94 && rustup toolchain install 1.94 --profile minimal -c rustfmt -c clippy`. Это известная ловушка, не паникуй.
- **Git identity:** локально `kaiserproger <kaisergrobe@gmail.com>` (см. `feedback-git-author` memory). НЕ меняй `git config`.
- **example.toml** уже указывает на `.analysis/test-world` для M3-demo; если M4 требует другой мир — добавь отдельный config файл, не правь example.toml без необходимости.
- **`.analysis/server.jar`** — bundle vanilla jar; используется `tools/dump-vanilla-protocol.sh` для javap-дампа в `.analysis/protocol-dump.txt`. Все packet-ID цитируй через javap, не угадывай (ADR 0002).
- **wire-probe** в `crates/mc-test-harness/src/bin/wire_probe.rs` подключается к реальному вандилла-серверу и дампит фреймы — полезно если M4 требует новых packet-захватов.
- **`mc_test_harness::client::Client`** — типизированный async-драйвер протокола; используй его в M4 интеграционных тестах.

## 5. Стиль

Кратко и по делу. См. memory `feedback-terse-no-stalls`:
- сообщения мне в 1-3 предложения, не простыни,
- если что-то идёт >5 минут (компиляция, поиск, исследование) — скажи, что запустил, и не ждём,
- не «сейчас я проанализирую…» — делай и докладывай результат.

Никаких комментариев типа «added for M4», «used by X» в коде — см. CLAUDE.md / system prompt: identifiers сами говорят что делают, комментарии только когда «почему» неочевидно.

## 6. Старт

Прочитай memory, прочитай M3.md, прочитай spec §9, и пиши: «прочитал, предлагаю варианты scope M4: …». Жду.
