# Как устроено ядро Solaris

> Срез на 2026-07-19, ветка `dev/M100-client-agent`.
>
> Это не roadmap и не обещание parity. Документ описывает фактический код в
> рабочем дереве. Для каждого крупного пути ниже явно указано состояние:
> `production`, `staged` или `oracle/test only`.

## 1. Короткая модель в голове

Solaris состоит не из одного "game loop", а из четырех контуров:

1. Tokio принимает TCP, ведет Handshake -> Status/Login -> Configuration ->
   Play и превращает пакеты в типизированные команды.
2. `SimulationOwner` сериализует игровые транзакции. Команда будит owner через
   bounded channel; owner не опрашивает очередь по таймеру.
3. Entity authority разбита по регионам. Долгоживущие owner lanes физически
   владеют `EntityStore`; coordinator хранит глобальные индексы и проводит
   cross-region prepare/commit/finalize.
4. World/chunk storage живет отдельно от ECS. Тяжелые чтения работают по
   immutable snapshots, а результат применяется только после проверки
   precondition/revision.

Сетевой task никогда не должен "просто поправить мир". Правильный путь:

```text
packet
  -> parse + immutable plan
  -> bounded command admission
  -> owner validates session/revision/world token
  -> one authoritative commit
  -> ordered publication facts/outbox
  -> per-session outbound queue
  -> packet codec + TCP
```

Tick по-прежнему нужен для времени, AI, physics, random/scheduled ticks и
периодических правил. Он не используется как polling interval для уже
пришедшей команды: пакет будит owner напрямую.

## 2. Что является authority

### 2.1 Entity state

`mc-entity::RegionalOwnerRuntime` и его lane-owned `EntityStore` являются
production authority для сущностей. Обычный `EntityStore::spawn` направляет все
семейства в ECS backend, включая mobs, items, XP, projectiles, falling blocks и
vehicles. `EntityRuntime` является единственным ECS runtime; прежние vector
comparison state, `Shadow*` API и `shadow-compare` feature удалены. Production и
tests больше не могут включить вторую authority конфигурацией.

Глобально coordinator владеет:

- `EntityId -> RegionKey`;
- `Uuid -> EntityId`;
- lease/epoch каждого региона;
- in-flight transfer metadata;
- глобальной последовательностью мутаций и durable decision boundary.

Физическим компонентным state владеет ровно один lane. Перенос региона между
lanes передает сам `EntityStore`, а не копию. Поэтому scale-up/scale-down не
создает две authority.

### 2.2 World state

`mc-world::WorldStorage` владеет chunks, sections, block entities, resident
sets, light и dirty state. Часть mutation surface уже проходит через
`SimulationOwner`, но migration еще staged: некоторые server-origin tick paths
все еще заходят в transitional world lock.

### 2.3 Player/session state

`mc-net::play::session::SessionRegistry` владеет активными session IDs,
connection-local player state, visibility, container viewers, published entity
projection и outbound endpoints. `SessionId` монотонный и служит fence:
команда от отключившегося соединения не может примениться к следующему login
того же UUID.

### 2.4 Static data

Registries, tags, recipes, block/entity/item facts загружаются при startup и
после этого читаются как immutable data. Они не должны находиться в ECS и не
должны блокироваться на каждом tick.

## 3. Главные архитектурные паттерны

### 3.1 Bounded actor вместо общего mutex API

`SimulationHandle` и regional owner handles отправляют typed command в
ограниченную очередь и ждут exact reply channel. Queue full/closed возвращается
до мутации. Owner получает команду через blocking `recv`/async `recv`: producer
сам будит consumer.

Зачем это сделано:

- одна точка линейризации;
- естественный backpressure;
- cancellation и shutdown можно доказать;
- telemetry измеряет реальную queue depth, а не косвенный lag;
- нет guessed sleep/retry windows.

### 3.2 Prepare -> commit -> finalize

Cross-region mutation сначала валидируется на каждом участвующем lane. Только
после успешного prepare coordinator разрешает commit всем участникам. Finalize
публикует глобальные индексы. Ошибка вызывает abort или rollback; speculative
semantic events обрезаются до checkpoint.

Этот протокол нужен для transfer, batch spawn/restore, vehicle groups,
conditional animal updates, damage, goal apply и save barrier.

### 3.3 Complete-snapshot CAS

Частичная проверка вроде "entity ID еще существует" недостаточна. Mutation API
обычно получает полный ожидаемый `EntitySnapshot`. Если position, lifecycle,
goal, item stack или другая fenced часть изменилась, commit отвергается целиком.

Это защищает от delayed packet work и от публикации старого результата поверх
более новой owner mutation.

### 3.4 Snapshot + compute + validated apply

Pathfinding, physics, lighting и worldgen не должны долго держать authority
lock. Owner снимает immutable view, compute pool считает результат, затем owner
применяет его только при совпадении revision/token. Устаревший результат не
"подправляется" эвристикой, а отклоняется или пересчитывается.

### 3.5 Ordered publication и outbox

Commit и видимый клиенту эффект нельзя разводить двумя независимыми
операциями. Там, где путь уже мигрирован, owner формирует ordered dispatches до
reply requester-у. Для более сложных новых kernels используется pending outbox:
state revision и publication facts фиксируются вместе, а baseline продвигается
только после ACK доставки.

### 3.6 ECS + SoA, но не "ECS для всего"

Moving entities идут через `bevy_ecs`/SoA-friendly storage. Chunks, connections,
registries и block entities остаются в специализированных структурах. Это
сохраняет последовательный доступ к компонентам и не заставляет coordinate
queries проходить через entity archetypes.

### 3.7 Fail closed на границах

Unknown packet layout, unsupported registry version, non-finite numeric state,
stale snapshot, duplicate UUID, malformed NBT и unknown persistence schema не
угадываются. Они возвращают typed error до частичной мутации.

### 3.8 Autoscale по измерениям

Процесс один раз определяет доступную CPU capacity. Operator не раздает
проценты threads подсистемам. Control plane меняет bounded admissions, chunk
work и число regional lanes по runtime measurements. Scale-down переносит
физические stores на оставшиеся lanes на idle command boundary.

### 3.9 Никакого Solaris legacy

У проекта не было релиза, поэтому старые внутренние API, persistence schemas,
duplicate authorities, adapters и feature flags не являются compatibility
surface. После cutover старый путь удаляется вместе с тестами на его прежнее
существование. Сохраняются только необходимые vanilla protocol/behavior и
vanilla world load/save contracts либо явно объявленный текущий внешний plugin
API. Это уменьшает число состояний, которые надо доказывать, и не позволяет
временно staged архитектуре стать постоянной.

## 4. Карта crates и зависимостей

| Crate | Роль | Внутренние зависимости |
| --- | --- | --- |
| `mc-server` | composition root, CLI, startup/shutdown | почти все crates |
| `mc-net` | protocol state machine, sessions, simulation orchestration | data/entity/physics/protocol/world/script |
| `mc-entity` | ECS entity state, regional ownership, AI/combat kernels | data, bevy_ecs, rayon |
| `mc-world` | chunks, sections, light, Anvil, snapshots | data, NBT |
| `mc-worldgen` | terrain/noise/ores/structures | data, world, NBT |
| `mc-data` | registries, tags, recipes, loot, vanilla facts | NBT |
| `mc-physics` | collision/fluid/projectile/block movement math | независимая pure boundary |
| `mc-protocol` | codecs, frames, packet layouts | data identifiers, NBT |
| `mc-nbt` | bounded NBT reader/writer | bytes |
| `mc-script` | bounded plugin DTO/command API, Lua runtime | изолирован от internals |
| `mc-extension` | bounded custom-client payload/event boundary | bytes/Tokio only |
| `mc-test-harness` | wire probes, replay, vanilla diff, load gates | test-side composition |
| `xtask` | code-health и repo gates | без runtime dependencies |

Направление зависимости намеренно идет от orchestration к domain. `mc-script`
и `mc-extension` не получают handles на `mc-net`, `mc-world` или `EntityStore`:
наружу выходят stable snapshots и bounded commands.

## 5. Startup и shutdown

### 5.1 `mc-server`

`main` выполняет только composition:

1. разбирает CLI/config;
2. валидирует пути, лимиты и vanilla sidecars;
3. строит registries и immutable facts;
4. открывает world metadata/chunks/entity persistence;
5. создает script boundary, control plane, simulation и regional owners;
6. передает готовые services в `mc_net::Server`;
7. на shutdown запрашивает drain и save, затем завершает runtime.

Shutdown разделен на две явные фазы. `BoundServer::serve` завершает producers,
command roots, connections, entity ticker и periodic dirty-flush worker, но не
делает final save. Только успешное completion-событие drain разрешает caller-у
выполнить один `SaveHandle::save_all_after_drain`. Ошибка или timeout drain
возвращается как ошибка shutdown и не разрешает clean-save результат; timeout
служит только ограничителем зависшей операции.

`startup_validation` держит fail-fast проверки отдельно от `main`: неверный
путь, отсутствующий обязательный sidecar или несовместимая config schema не
должны приводить к частично запущенному listener.

### 5.2 `mc-net::server`

Основные function families:

| Функция/семейство | Ответственность |
| --- | --- |
| `Server::bind` / construction helpers | создать listener и immutable shared services, не запускать gameplay |
| `serve` | accept loop, connection supervision, authoritative entity ticker и shutdown select |
| connection join/drain helpers | не оставить detached tasks и вернуть точную ошибку завершения |
| entity/world checkpoint helpers | запросить owner save barrier и записать только подтвержденный snapshot |
| autoscale/control-plane hooks | передать новое admission/CPU решение, не менять gameplay state напрямую |

Entity ticker внутри `serve` проводит simulation commands, world time,
lifecycle, AI, physics и server-authored ticks. Shutdown считается успешным не
по истекшему времени, а по exact completion events от listener, connections,
simulation owner, regional lanes и final save. Timeout может только объявить
зависание и вернуть ошибку; он не является условием успеха.

## 6. Сетевой state machine

### 6.1 `connection_driver`

`drive_connection` читает первый handshake и выбирает Status либо
Login/Transfer. После Login он последовательно проводит Configuration и Play.
Переход фазы явный: packet ID никогда не интерпретируется без текущего protocol
state.

### 6.2 `connection`, `encryption`, `login`, `configuration`, `status`

| Модуль | Function-level роль |
| --- | --- |
| `connection` | framed read/write, compression/encryption transition, bounded packet body |
| `encryption` | RSA/AES handshake primitives; не владеет login policy |
| `session_auth` | Mojang session verification и identity result |
| `login` | username/UUID validation, compression/login-success sequence |
| `configuration` | known packs, registries/tags, client information, finish ACK |
| `status` | status JSON и ping/pong без создания gameplay session |

Codec functions только преобразуют bytes <-> typed packet. Они не имеют права
мутировать session/world state.

`AuthSection::prevent_proxy_connections` напрямую включает передачу IP клиента
в Mojang `hasJoined`; default остается `false`. Это operator policy, а не
эвристика login path.

## 7. Play path

`play.rs` пока остается большим orchestration facade. Новая логика должна жить
в дочернем domain module, а `play_loop` только маршрутизировать packet к нему.

### 7.1 Главный цикл

`play_loop` одновременно слушает:

- следующий client packet;
- push-события outbound queue;
- shutdown/session invalidation;
- exact completion нужной owner request.

Он не ждет "несколько ticks, чтобы состояние успело". Packet handler строит
plan, вызывает domain/owner API, применяет только typed result и пишет packet.

### 7.2 Модули packet-authored gameplay

| Модуль | Что решают его функции |
| --- | --- |
| `movement` | decode player pose intent, numeric/bounds checks, collision/authority commit, correction |
| `block_break` | mining preconditions, progress/session, tool/drop plan, finish/abort |
| `block_placement` | hit-face/yaw state selection, replaceability/support, placement plan |
| `block_edit_commit` | единый adapter от pure edit plan к simulation owner commit |
| `bucket_interactions` | pickup/place/cauldron plan и inventory conservation |
| `plants` | hoe, seed, bonemeal, crop mutation rules |
| `beds` | occupancy/sleep/spawn semantics |
| `toggles` | doors/trapdoors/levers-like state transitions |
| `item_blocks` | item -> block mapping and state projection |
| `combat` | player attack/use actions и player damage policy |
| `player_damage_adapter` | перевод domain damage outcome в session mutation/publication |
| `recipes` | recipe selection/display, не container transaction |
| `containers/*` | per-menu state machine, click/quickcraft/crafting/furnace/enchant/stonecutter |
| `command_execution` | parsed command -> typed gameplay operation |
| `commands` | command tree construction, argument parsing и permission-facing descriptors |
| `chunk_stream` | desired chunk window, prepare/admission, ordered send/forget |
| `block_wire` | block state/change facts -> exact clientbound block packets |
| `inventory` | stack/component helpers и player inventory packet projection |
| `spawn` | initial player pose/world spawn packet sequence |
| `survival` | hunger/saturation/exhaustion/tick rules вне session mutation |
| `world_journal` | durable world decision records, replay watermark и clear protocol |
| `use_item_on_adapter` | packet hit context -> placement/bucket/toggle domain routing |
| `wire_entities` | snapshot/publication fact -> exact entity packets |

### 7.3 Server-authored gameplay

| Модуль | Function-level роль |
| --- | --- |
| `random_ticks` | выбрать bounded due block positions и подготовить crop/fire-like mutations |
| `scheduled_blocks` | выполнить due scheduled rules в authoritative order |
| `fluids` | pure spread intent + validated world apply |
| `falling_blocks` | block->entity start и entity->block landing transaction |
| `lighting` | relight plan, section masks и peer publication |
| `explosions` | ray/destroy/drop/TNT chain planning и ordered transaction |
| `campfire` / adapter | cooking state machine отдельно от owner commit/publish |

## 8. SessionRegistry и publication

`session.rs` является facade; ownership вынесено в дочерние модули.

| Модуль | Основные функции и инварианты |
| --- | --- |
| `session_lifecycle` | register/unregister, monotonic `SessionId`, stale-session fence |
| `player_state` + adapters | authoritative inventory/survival/cursor projection and CAS |
| `player_pose_authority` + adapter | owner-only pose mutation и packet-facing typed result |
| `player_item_action_authority` | held-item use/consume/durability transaction |
| `survival_action_authority` | hunger/heal/damage survival transition под session fence |
| `transactions` | общие prepare/commit outcomes для multi-state session mutations |
| `entity_owner` | exact request/reply adapter к regional runtime, local cache invalidation |
| `entity_lifecycle` | spawn/remove indexes, published snapshots, visibility lifecycle |
| `entity_simulation` | active sets, tick inputs, pure physics/goal preparation |
| `entity_simulation/persistence_projection` | entity snapshot -> versioned persistence DTO без disk IO |
| `entity_combat` | reach/target validation, owner damage transaction, rewards |
| `player_combat` | attacker session state, cooldown/weapon facts и attack admission |
| `visibility` | per-session tracked set, enter/stay/leave diff, entity-major dispatch order |
| `outbound` | bounded recipient commands и reliable delivery state |
| `outbound_publication` | reserve/dispatch/ack ordering, no state mutation under socket write |
| `container_state/views` | viewer version, open menu ownership, peer updates |
| `pickups` | item/XP candidate plan and complete-snapshot claim |
| `projectiles` | bow/arrow owner adapter and projectile target snapshots |
| `passive_mobs` | breeding/grazing planning; `authority` child commits feed/shear/breed |
| `herd_spawn_authority` | batch spawn preflight и один authoritative herd commit; entities не схлопываются в одну |
| `hostile_authority` | target/attack phase selection and committed damage/projectile output |
| `pathing` | world read-view probes and bounded path request resolution |
| `interaction_geometry` | reach, eye/target AABB и line-of-interaction checks |
| `chunk_view_authority` | exact loaded-window projection used by publication |
| `prepared_chunks` | connection-local prepared/sent chunk revisions и stale-result rejection |
| `campfire_authority` | campfire block-entity transaction и resulting publication facts |
| `explosion_authority` | world/entity consequences одной explosion decision |
| `sleep` | Minecraft bed occupancy/sleep state; название не связано с wall-clock sleep |

Нельзя публиковать snapshot, просто потому что owner когда-то принял input.
После commit publication path повторно сверяет current authoritative snapshot;
иначе delayed output может затереть новую position/lifecycle.

## 9. SimulationOwner

### 9.1 `simulation::queue`

| Функция/тип | Назначение |
| --- | --- |
| channel constructor | создает bounded queue, sequence source и metrics |
| enqueue functions | reserve capacity, stamp sequence/session fence, fail before mutation |
| owner receive/drain | push wake, bounded drain, stable sequence order |
| herd coalescing | объединяет только явно совместимые detached herd commands |
| shutdown | закрывает admission и отвечает queued requesters typed error |

### 9.2 `simulation.rs`

`SimulationHandle` содержит requester API. Каждая public `commit_*` функция:

1. получает или проверяет session fence;
2. формирует ровно один `SimulationCommand`;
3. enqueue без произвольного response timeout;
4. ждет exact oneshot reply;
5. проверяет вариант `SimulationResponse`.

`SimulationOwner` владеет command processing. `process_*`/`commit_*` branches
повторно валидируют expected snapshots/tokens непосредственно перед mutation,
применяют state и создают dispatches до reply.

### 9.3 `simulation::regional_mutation`

Здесь живет regional lane для block/container mutations: сортировка admission,
lease/token validation, WAL decision order, atomic failure и response order.
Это extraction boundary, а не завершенная world regionalization.

### 9.4 Operational-модули `mc-net`

| Модуль | Function-level ответственность |
| --- | --- |
| `blocking` | принять bounded blocking job, выполнить его вне Tokio workers и вернуть exact completion; queue rejection происходит до запуска |
| `chunk_pipeline` | подготовить load/generate/light/encode stages, сохранить chunk revision и опубликовать только актуальный результат |
| `dirty_flush` | слушать high-water/save notification, снимать plan, писать вне world owner и подтверждать conditional clean |
| `control_plane` | собирать queue/service/load samples и применять bounded admission/lane decision |
| `runtime_tick_metrics` | считать service time, lag и per-subsystem work без управления gameplay |
| `lock_metrics` | измерять acquisition/hold contention известных transitional locks |
| `memory_pressure` | оценивать resident/cache pressure и запрашивать eviction/admission reduction |
| `autoscale_soak` | workload/measurement support для длительной проверки scaler, не production policy |

`control_plane` не получает operator percentages. Он может менять лимиты и
число lanes в разрешенных bounds, но не переписывает authority state. Любая
scale операция завершается exact ACK владельцев; наблюдаемая пауза или несколько
спокойных ticks не считаются завершением.

## 10. Regional entity runtime

### 10.1 `regional::owner_lane`

Каждый worker loop блокируется на своей bounded queue. Его команды:

- install/detach физического `EntityStore`;
- exact selected reads;
- prepare/commit/finalize/abort/rollback mutation phase;
- save barrier;
- clean shutdown с возвратом stores.

Lane сортирует локальную работу по `(RegionKey, sequence)`. Rollback восстанавливает
не только components, но и ID watermark, vehicle graph и длину semantic event
queues.

### 10.2 `RegionalEntityStore`

Ключевые function families:

| Семейство | Что делает |
| --- | --- |
| `spawn*` / `restore*` | all-input preflight, UUID/ID uniqueness, global index publish after finalize |
| `snapshot*` / selected reads | fan-out по owners, merge by EntityId, index consistency check |
| `apply_*_if_current` | complete-snapshot CAS для kinematics/animal/item/goal |
| `damage_if_current` | stale-safe damage, post-finalize `EntityDamage` |
| transfer functions | source remove + target insert, atomic location publish |
| vehicle transfer | перенос exact connected group с leader delta |
| `prepare_goal*` / `apply_goal*` | compute вне actor, fenced result обратно на owners |
| `save_barrier` | exact lease set + finalized watermark + immutable snapshots |
| scale functions | detach/install stores, advance lease epoch, join retired lane |

### 10.3 Остаточная сериализация

Независимые regions уже считают goal/physics параллельно. Warm single-lane item
CAS и kinematics могут идти прямо в cached owner lane, не заходя в coordinator.
Cross-region операции, cache misses и глобальные indexes все еще проходят через
coordinator actor. Эта остаточная сериализация остается главным пределом true
multicore entity mutation.

## 11. `mc-entity`: ECS и доменные kernels

### 11.1 `EntityStore`

`EntityStore` является локальным хранилищем одного owner lane. Внутри находятся
ECS components и индексы, необходимые для быстрых адресных операций. Снаружи
код не получает `World`/query guard: API возвращает immutable `EntitySnapshot`
или выполняет целую проверенную mutation.

| Семейство функций | Контракт |
| --- | --- |
| `spawn*` | проверить ID/UUID/type/position, создать полный набор обязательных components, обновить локальные индексы |
| `restore*` | принять уже назначенный ID и persistent state; не повышать watermark при провале |
| `remove` | удалить entity и связанные индексы/vehicle edges как одну локальную операцию |
| `snapshot*` | собрать стабильную DTO-копию без выдачи ECS references наружу |
| `set_position` / `set_velocities` | простая owner-local mutation; regional facade добавляет lease и CAS |
| `apply_kinematics_if_current` | сравнить полный snapshot, затем вместе применить position/velocity/on-ground |
| animal/item/goal setters | менять только соответствующий component, но fence строить по полному expected snapshot |
| `damage*` | проверить lifecycle/health, применить bounded health transition, вернуть outcome для публикации |
| vehicle functions | валидировать граф, не допускать cycle и менять exact connected group |
| goal prepare/resolve | снять inputs, считать вне ECS borrow и вернуть revision-fenced plan |

Компоненты группируются по частоте прохода. Position/velocity/lifecycle лежат на
горячем пути; редкие persistent и gameplay поля не должны раздувать каждую
physics iteration. Snapshot сознательно дороже component query: это boundary для
сети, persistence и cross-lane coordination, а не формат внутреннего цикла.

### 11.2 Production и staged нельзя смешивать

В `mc-entity` сейчас есть два уровня:

- **production**: `EntityStore`, regional ownership, текущие movement/goal,
  animal/item/projectile snapshots и используемые `mc-net` транзакции;
- **staged**: новые точные kernels 26.1.2 для attributes, effects, synced data,
  living damage, navigation, mob controls, equipment, runtime и projectiles.

Слово `staged` здесь относится к неподключенным domain kernels, а не к второй
копии entity state. Все подключаемые kernels обязаны читать и коммитить через
тот же ECS/regional snapshot fence.

Наличие точного kernel и его unit tests не означает, что живой сервер уже
использует его. Cutover закончен только когда packet/session adapter передает
реальный state в kernel, owner атомарно применяет outcome, publication пишет
правильные пакеты, persistence сохраняет state, а harness сравнивает путь с
vanilla.

### 11.3 Living damage kernel

| Функция/тип | Роль |
| --- | --- |
| damage source constructors | задают теги/attacker/direct entity без строковых догадок в combat code |
| reduction helpers | armor, resistance, absorption и invulnerability phases в vanilla order |
| `prepare_*damage*` | pure расчет outcome из полного input; не мутирует entity |
| `apply_*damage*` | revision/state fence, затем health/absorption/lifecycle transition |
| death outcome | отделяет `Killed` от `Discarded`; rewards и публикация остаются caller-owned |

Главный инвариант: не должно быть состояния «здоровье уже сняли, а обязательный
callback/effect/death publication потеряли». Поэтому live cutover требует
outbox, а не последовательности независимых вызовов.

### 11.4 Effects kernel

`EffectInstance` хранит ID, kind, amplifier, duration, flags и скрытую цепочку.
Constructor проверяет структурные ограничения, но сохраняет Java-compatible
числовую семантику там, где vanilla разрешает casts NaN/infinity.

| Операция | Что делает |
| --- | --- |
| add/update | применяет vanilla precedence amplifier/duration и возвращает точный change outcome |
| remove | удаляет exact effect и сообщает, нужна ли attribute/publication реакция |
| tick | уменьшает duration, активирует periodic effect на нужной фазе, разворачивает hidden effect |
| ordered traversal | принимает полный уникальный порядок от caller; не притворяется порядком Java map |
| gameplay evaluator | формирует heal/damage/food-like facts, но сам не меняет внешнюю authority |

### 11.5 Synced entity data

`EntityDataAccessor` идентифицируется числовым ID; тип и serializer проверяются
при get/set. `SyncedValue` сохраняет serializer wire ID и reference identity.
Последнее важно: vanilla отмечает dirty при замене объекта равным по значению,
если reference другой.

| Функция | Роль |
| --- | --- |
| define | создать один accessor; duplicate ID отвергается |
| get | проверить ID, serializer и ожидаемый Rust type |
| set/assign | заменить текущее reference, выставить item/global dirty в vanilla порядке |
| pack dirty | вернуть ordered changed items и очистить dirty только по подтвержденному пути |
| assign batch | sequential semantics: уже примененный prefix сохраняется при ошибке позднего item |

### 11.6 Attributes

`AttributeInstance` хранит base value, permanent/transient modifiers и cached
calculated value. Modifier key соответствует vanilla resource identifier, а не
случайно выбранному UUID. Calculation выполняет операции в трех фазах:
addition, multiply-base, multiply-total. Cache инвалидируется при любой
наблюдаемой замене, включая новый reference с тем же значением.

`AttributeMap` отвечает за template lookup, lazy instance creation, dirty
tracking и assign из network/persistence snapshot. Identifier-keyed modifiers
повторяют доказанный fastutil order. Cross-attribute vanilla order зависит от
JVM identity hashes `Holder`/`AttributeInstance` и не воспроизводим между
процессами, поэтому persistence/publication используют детерминированный
`AttributeId` order, а sequential assignment сохраняет явный input prefix.
Duplicate templates сохраняют последнюю definition. Модуль остается staged.

### 11.7 Equipment

| Подмодуль | Роль |
| --- | --- |
| slot/stack model | typed slot, item/count/components/durability и exact empty semantics |
| equipment state | owned mutable stack state и per-slot revision |
| publication diff | определить изменившиеся slots без lossy fingerprint и вернуть move-only batch |
| pickup planner | eligibility, preferred slot и replacement decision |
| pickup commit | атомарно уменьшить ground stack, заменить equipment и сформировать drop/publication facts |
| death drops | chance, looting/durability inputs, atomic slot clear и spawn facts |
| durability | Java-compatible RNG bounds и damage/break transition |

Особенно опасен pickup: три эффекта должны быть одной транзакцией. Baseline
equipment diff продвигается только после явного подтверждения admission всего
batch в caller-owned reliable queue; retryable packet/effect outbox внутри
kernel удален, потому что он повторял non-idempotent actions при потере ACK.
Stack хранит owned typed/opaque components с единым 2 MiB пределом до mutation.
Cross-authority применение spawn facts, delivery/reconnect и RNG generation
остаются явной обязанностью production adapter. Модуль staged.

### 11.8 Navigation и AI

`navigation_26_1_2::SearchScratch` является переиспользуемым bounded A* scratch.
`SearchBudget` ограничивает nodes и cell evaluations; исчерпание возвращает
точную termination, а не подвешивает tick. `NodeEvaluator` поставляет neighbors,
malus и heuristic; search владеет heap, visited records и reconstruction.

Основные функции:

- `SearchGoal::new/contains/euclidean_lower_bound` задают проверяемую область
  достижения;
- `NodePos::checked_offset` предотвращает coordinate overflow;
- `SearchScratch::search` сбрасывает scratch, ведет heap, валидирует finite
  costs и возвращает reached/partial/budget result;
- `heap_*` поддерживают index-aware binary heap без allocation на каждом pop;
- `validate_cost` и `checked_accumulation` fail closed на отрицательных и
  non-finite cost.

AI core разделяет goal policy, sensing facts и mutation plan. Sensors и
pathfinding читают snapshots; только owner применяет выбранную цель. Это не
полный Brain/Behavior parity: staged слой сначала закрывает общие mob invariants,
после чего семейства сущностей подключаются по одному.

### 11.9 Mob controls

`move_control`, `look_control`, `jump_control`, `body_rotation_control` и
`flying_move_control` повторяют маленькие vanilla state machines. Каждая имеет
`prepare_*`, которая считает следующий state из immutable facts, и `apply_*`,
которая сравнивает revision/current state. Swimming добавляет `0.005` к текущей
Y velocity; отсутствие navigation/evaluator проходит по vanilla fallback.

Controls не выбирают цель и не строят path. Они превращают уже принятое AI
решение в yaw/speed/jump/velocity intent. Это граница, которая должна устранить
дерганье от нескольких независимых writers movement state.

### 11.10 Projectile kernel

Новый staged модуль разделен на:

- `lifecycle`: owner, left-owner, first-shot, hit dispatch, eligibility и
  rotation;
- `hit_order`: block ray truncation и стабильный порядок entity candidates;
- `throwable`: gravity -> inertia -> move -> common tick -> impact;
- `arrow`: in-ground/no-physics, piercing, pickup, despawn и embedding.

Kernel принимает уже собранные collision candidates. Raycast по миру, damage,
enchantments, portals, packets, sounds и subclass-specific эффекты остаются у
caller. Это намеренное разделение pure vanilla arithmetic и server authority.

### 11.11 Runtime composition

`runtime_26_1_2` связывает living/effects/attributes callbacks в одну staged
transaction. Он должен различать player и non-player gates, посылать
`onMobHurt` для активных effects только после успешного damage и выполнять
remove callbacks для переходов `Killed`/`Discarded`. Standalone kernel прошел
oracle review, включая direct live -> `Killed`, stale revisions и bounded
failures; production wiring и выполнение внешних callbacks еще отсутствуют.

## 12. `mc-world`: chunks, snapshots, light и disk

### 12.1 Block и chunk model

`BlockPos`/`ChunkPos` выполняют координатные преобразования и checked bounds.
`Chunk` хранит sections, heightmaps, block entities, scheduled ticks и revision.
`ChunkSnapshot` является immutable shared представлением для read/compute.

`ChunkSection` имеет три состояния хранения: single value, indirect palette и
direct IDs. `get` читает local index; `set` обновляет non-air count и расширяет
palette/rebits при необходимости; `from_indirect` валидирует загруженный disk
format. `PackedBitArray::{get,set,rebit}` реализуют плотное хранение без
per-block object allocation.

### 12.2 `WorldStorage`

| Функция | Ответственность |
| --- | --- |
| `open*` | открыть root/region cache с явными capacity и registry |
| `in_memory*` | deterministic test/in-memory world без disk side effects |
| `with_item_registry` | добавить item decode context для block entities |
| `with_generator` / `set_generator` | установить chunk generator; generator не получает mutable storage |
| `get_block` | load/generate chunk при необходимости и прочитать state |
| `get_cached_block` | только resident read, никогда не инициирует IO/generation |
| `get_chunk*` | resident -> disk -> optional generation в явно выбранном варианте |
| `commit_chunk_snapshot` | безусловный owner-side replace для доказанного caller path |
| `try_commit_chunk_snapshot` | revision/CAS apply после внешнего compute |
| journal restore/replay | восстановить durable decision и не задвоить уже примененный chunk |
| cache/resident snapshots | создать immutable views для network/compute/save |

`WorldMutationView` является узким mutation boundary поверх resident store. Он
дает conditional operations, а не raw `&mut Chunk`.

### 12.3 Resident transactions

| Семейство | Инвариант |
| --- | --- |
| `apply_block_edits_conditionally*` | все preconditions проверяются до первого edit; journal stamp связан с commit |
| fluid/scheduled tick commit | due tick, expected blocks и resulting edits проверяются вместе |
| furnace commit/tick | input/fuel/output/revision меняются одним outcome |
| hopper transfer | source decrement и target increment атомарны |
| chest commit | все половины/slots проверены до mutation |
| opaque block entity commit | unknown payload сохраняется без частичного decode/rewrite |
| `set_block_if_current` | compare expected state/token и вернуть applied/stale/missing |
| light publication | применить baked light только к тем же chunk revisions |

Suffix `*_journaled` означает, что durable decision stamp входит в тот же
critical section. Это не просто logging после мутации.

### 12.4 Read views

`WorldReadView` хранит sharded immutable chunk snapshots и furnace projection.
`get_cached_block`, mutation-token и snapshot functions не заходят в disk IO.
`publish_chunk/update_chunk/remove_chunk` вызываются producer-ом изменения и
сразу обновляют view: consumer не poll-ит storage.

`plan_chunk_snapshot_without_generation` возвращает либо resident snapshot,
либо `ChunkDiskLoadPlan`. `load` выполняется вне authority lock, после чего
caller применяет результат через validated commit. `ChunkSourceView` и
`ScheduledTickView` дают compute path только данные о source/due state.

### 12.5 Dirty flush

`plan_dirty_flush_at_tick_bounded` снимает ограниченный набор immutable dirty
snapshots. `DirtyFlushPlan::write` пишет временные region payloads без mutation
resident state. `commit_dirty_flush` очищает dirty только если revision все еще
совпадает; изменившийся во время IO chunk остается dirty. High-water notifier
будит flush consumer push-событием.

### 12.6 Lighting

`LightLayer` хранит nibble values; `ChunkLight` объединяет sky/block layers.
`LightWorkspace` переиспользует очереди и scratch buffers. `compute_chunk_light`
создает workspace, а `compute_chunk_light_in` использует переданный scratch для
горячего пути. `apply_block_change_to_light` инкрементально распространяет
изменение через соседние chunks. `encode_chunk_light` строит exact masks и
arrays для protocol.

`LightKernelBackend` выбирает scalar или portable-vector kernel по явной
конфигурации `SOLARIS_SIMD_BACKEND`; оба варианта проверяются на одинаковый
результат. Это пока не автоматический CPU-feature dispatcher. SIMD здесь полезен
на плотных nibble/column операциях; branch-heavy gameplay насильно в SIMD не
переводится.

### 12.7 Anvil и NBT

`read_region` до decode проверяет всю location table: ссылки на reserved header
sectors, полный extent каждого allocation и overlap между chunk slots. LZ4Block
дополнительно проверяет xxHash32 checksum каждого decoded block и end marker.
`write_region` до открытия output проверяет local coordinates, duplicate slots,
decoded budgets и точный sector count: 255 sectors допустимы,
а более крупный chunk возвращает ошибку вместо обрезания count до `u8`.
Writer собирает и синхронно записывает полный новый image, но сам по себе не
является atomic replace. Production dirty-flush пишет его через
`write_region_create_new` во временный файл и только затем делает rename;
create-new защищает recovery path от перезаписи чужого временного решения.

`chunk_from_nbt*` декодирует sections, palettes, light, heightmaps, scheduled
ticks и block entities. Production storage передаёт decoder'у абсолютный
requested chunk key, требует полного потребления NBT payload и до публикации в
cache сверяет его с `xPos/zPos`. Dirty-flush отдельно передаёт serializer'у
expected resident key и выбирает region slot только из него; mismatch не создаёт
temp file. `repair_chunk_nbt_position` — явный offline primitive для
канонизации повреждённых coordinate fields и никогда не вызывается runtime load.
`chunk_to_nbt*` делает обратное преобразование; варианты `with_items_at_tick`
получают item registry и текущий tick явно, чтобы disk формат не зависел от
скрытого global state.

## 13. `mc-worldgen`

`ChunkGenerator` является trait boundary, поэтому storage не знает тип
генератора. `TerrainGenerator` реализует pipeline: climate/noise -> base density
-> surface/biome -> ores/features -> structures. Генерация детерминирована seed
и chunk coordinates. Terrain, biome, ore и structure paths принимают явный
`ChunkGeometry`; production-код не использует глобальные Overworld `MIN_Y` /
`MAX_Y`. Absolute-Y arithmetic выполняется через checked offsets или wide
intermediates, поэтому валидная геометрия около границ `i32` не вызывает panic
или wrap.

| Модуль/функции | Решение |
| --- | --- |
| `noise::{value_noise_2d,fbm_2d}` | deterministic primitive и octave composition |
| `terrain::surface_height` | быстрый column query без materialization полного chunk |
| biome rules | скомпилированные climate predicates и deterministic selection |
| ore rules | bounded placement attempts из data facts |
| geometry regression | serialized chunk NBT fingerprint для стабильности текущего Overworld output |
| `StructureTemplate::from_nbt_file` | parse Mojang structure NBT вне hot path |
| `StructureRules` | spacing/separation/salt и template catalog |
| `MercatorProjection` | отдельный Tellus mode; не участвует в vanilla mode |

Worldgen использует data-driven facts, но пока не является полной реализацией
vanilla NoiseRouter/structure placement. Поэтому существующие vanilla custom
maps загружаются через Anvil лучше, чем воспроизводятся нашим генератором.
Fingerprint доказывает только детерминизм нашего output, не byte parity с
vanilla generator.

## 14. `mc-data`: immutable vanilla facts

`VanillaData::load` читает извлеченные registries; `solaris_required_data`
дает минимальный встроенный fallback для разработки. `required_registry_*`
являются точными lookup таблицами поддерживаемого минимума. Full parity нельзя
заявлять при fallback-only startup.

| Модуль | Функции и данные |
| --- | --- |
| `identifier` | parse namespace/path, canonical string; reject invalid resource IDs |
| `blocks/items` | load reports, build bidirectional protocol registries |
| `block_facts/mining/light/collision` | immutable per-state lookup для gameplay hot paths |
| `entity_types` + `entity_contract_26_1_2` | protocol ID/name/category/dimensions/flags contract |
| `recipes` | parse shaped/shapeless/smelting/stonecutting; built-in fallback |
| `tags` | resolve nested tags, detect missing/cycles, emit client tag payload |
| `item_components` | typed component facts для inventory/wire |
| `loot` | compile block/entity tables и evaluate against explicit context/RNG rolls |
| worldgen inventories | ores/features/structures facts для generator |

Loot разделен на `model`, `compile` и `evaluate`: loader сначала превращает JSON
в bounded internal plan, а runtime evaluator не парсит JSON и не обращается к
filesystem. RNG приходит входом, поэтому тест воспроизводит exact branch.

## 15. `mc-physics`

`mc-physics` не владеет сущностями или миром. `BlockSampler` является read-only
интерфейсом; caller подставляет snapshot/view.

| Функция | Что считает |
| --- | --- |
| `step_entity` | gravity/drag/fluid, axis collision, vanilla-like step-up, on-ground и resulting body |
| `vanilla_push_impulse` | entity contact impulse с exact recipient policy |
| `vanilla_cramming_gate` | eligibility/count gate для cramming |
| `evaluate_cramming_roll` | отделяет deterministic gate от caller RNG sample |

`BlockMaterialIds` компилирует state IDs в быстрые classification/collision
lookup. Полные collision boxes важнее одного height: stair/fence/partial blocks
должны проходить shape-aware path, иначе игроки и мобы визуально застревают.
Старые неиспользуемые `falling_block_intent`, `fluid_spread_intents` и
`ground_y_for_body` удалены; production falling/fluid planning находится в
соответствующих `mc-net::play` domain modules и не дублируется в physics crate.

## 16. `mc-protocol` и `mc-nbt`

### 16.1 Codec и framing

`ReadMc`/`WriteMc` задают primitives: VarInt/VarLong, bounded strings, arrays,
UUID, positions и NBT. Каждый decode обязан проверить оставшийся body и лимит
до allocation. `read_varint_partial` отличает incomplete input от malformed.

`try_decode_frame` разбирает length/compression и возвращает `None`, только если
не хватает bytes. `encode_frame` применяет compression threshold и packet ID.
`encoded_size_uncompressed` нужен для reservation, а не является вторым codec.
Workspace отключает default `miniz_oxide` backend у `flate2` и использует
`zlib-rs`: инициализация miniz deflate state переполняла обычный 2 MiB stack
runtime worker при сжатии chunk frame. Это не меняет zlib wire format; frame
round-trip tests остаются correctness fence для любой будущей смены backend.
Outer frame ограничен VarInt21 (`2_097_151` bytes), decompressed payload имеет
отдельный предел `8_388_608`. Serverbound Known Packs, custom payload и hashed
component collections проверяют count/length до allocation; encode обязан
применять те же пределы до записи первого byte.

`Packet` связывает typed packet с state/direction/ID. Packet modules содержат
только layout. Например staged `entity_sync_26_1_2` кодирует attributes,
equipment, effects и leash; он не решает, когда их отправлять.

### 16.2 NBT

`Tag` является полным enum формата. `read_network/write_network` работают с
безымянным network root; `read_named/write_named` с disk-style named root.
Reader ограничивает depth, lengths и remaining bytes до allocation. `ListTag`
сохраняет единый element type, включая корректный empty list.

## 17. Lua plugins и custom extension

### 17.1 Stable script boundary

Lua не получает Rust pointers, ECS queries, sockets или world locks. Host
публикует immutable `ScriptEvent` с `ScriptPlayerContext`; plugin возвращает
bounded `CommandBatch`. Каждая `ScriptCommand` имеет capability, которую host
проверяет до admission.

| API | Роль |
| --- | --- |
| `script_boundary_pair` | создать две bounded очереди: host events и server commands |
| `try_enqueue_event` | fail-fast при full/closed; не блокировать gameplay owner |
| `try_enqueue_player_command*` | route только владельцу command root, проверить verified/operator context |
| `recv_command` / `recv_event*` | exact push-driven wake следующего consumer |
| `accept_host_command` | consume bounded one-shot admission и вернуть только attested `AdmittedScriptCommand`; replay/substitution отклоняются |
| manifest builder/`validate` | нормализовать ID/events/dependencies/capabilities и атомарно отвергнуть конфликт |
| `CommandBatch::try_push_authorized` | проверить limit и capability до добавления command |
| `RuntimeControls` | enforce instruction и memory budgets плюс shutdown policy на каждом invocation |

`start_lua_host` находит plugins, читает manifests, строит isolated Lua states и
запускает host thread. `install_solaris_api` экспортирует только bounded command
constructors. `run_with_instruction_budget` умеет ограничить instruction fuel;
filesystem/process/debug libraries не выдаются. `plugin.toml` читается максимум
до 64 KiB, `main.lua` до 1 MiB, а DTO/manifest/batch limits проверяются до
allocation и queue admission. Ошибка handler или poisoned invocation authority
отключает один plugin и освобождает его command roots, не останавливая сервер.

Контракт 0.6 уже содержит persistent-storage requests, zones, inventory-menu
DTO, atomic currency/item transaction, colonies и villager-binding requests.
Открытый разрыв находится в `mc-net`: production router и domain owner adapters
ещё должны durably применить эти attested requests и вернуть targeted results.

### 17.2 `mc-extension`

Этот crate не запускает плагины и не хранит прежнюю Solaris manifest/version
схему. Он задает только immutable inbound events, bounded outbound commands,
allow-list/размерный fence для custom payload и push-уведомления для обеих
ограниченных очередей. Он намеренно не ссылается на server internals. Vanilla
client работает без этого bridge; custom client использует только явно
разрешенные payload channels.

Minecraft client MCP находится рядом, но не является частью server authority.
Fabric mod переводит MCP commands в реальные client actions и публикует
наблюдаемое состояние мира/GUI/packets. Он нужен как reusable black-box test
driver, а не как обход owner invariants.

### 17.3 Harness, oracle и repo tooling

`mc-test-harness::client` является минимальным protocol client: connect,
handshake/login/configuration/play, bounded packet read и exact packet wait.
`parity` запускает сопоставимые сценарии против Solaris и vanilla и сравнивает
нормализованные наблюдения. `ObservationSet::normalize_sequence` сохраняет
порядок и multiplicity протокольных/gameplay фактов. Отдельный
`normalize_set` сортирует и deduplicate только явно unordered наборы и не
годится как strict oracle для packet sequence. Строгие entity gates хранят
ordered transcript и используют sequence-like API.

`replay` читает зафиксированные нами input/expected outcomes и проверяет core
без Minecraft process. `wire_probe` добывает packet IDs/layouts из bundled
vanilla; `registry_data_extract` извлекает разрешенные локальные facts;
`coverage_audit` проверяет, что readiness rows имеют реальный gate;
`core_replay_validate` запускает replay corpus.

Текущий strict entity side-by-side gate уже сохраняет order/multiplicity и
намеренно остается ignored в обычном workspace run. Последний реальный запуск
дошел до сравнения и поймал production mismatch default metadata: vanilla
отправил два ordered default packets с индексами 9 и 18 (`byte:12` для sheep),
Solaris отправил один packet с индексами 16 и 18 (`byte:0`). Dirty sheared
update с индексом 18 и `byte:16` совпал. Это полезный красный oracle gate, а не
повод ослаблять normalizer.

`xtask code-health` обходит Rust AST/text rules для запрещенных ownership
imports, монолитных определений, sleeps и архитектурных лимитов. Это fail-only
tripwire: он не заменяет компилятор, tests, oracle или клиент.

## 18. Полные пути выполнения

### 18.1 Login

```text
TCP accept -> handshake state -> authentication -> configuration payloads
-> SessionRegistry::register -> player entity spawn/restore
-> initial chunks/entities/inventory -> Play loop
```

Любой провал до register закрывает только connection. Провал после частичного
spawn обязан unregister/remove либо завершиться через recovery decision.

### 18.2 Движение игрока

```text
movement packet -> finite/bounds/sequence checks -> world snapshot collision
-> SimulationCommand -> session fence + current pose check -> commit
-> visibility publication -> correction only on rejection/divergence
```

### 18.3 Ломание блока

```text
start packet -> mining facts/tool/mode -> break session + progress stages
finish packet -> expected block/token/reach recheck -> loot plan
-> atomic block edit + inventory/tool/drop consequences
-> block update + break animation clear + entity drops
```

Сервер не должен отвергать корректный finish только потому, что независимый
косметический tick поменял нерелевантное поле. Но смена блока, session или tool
должна закрывать stale plan.

### 18.4 Entity tick

```text
owner snapshots active set -> sensors/path/physics parallel compute
-> sorted regional plans -> complete-snapshot CAS
-> semantic events/outbox -> session visibility diff -> packets
```

Для idle entities cadence может быть реже, но producer event обязан немедленно
разбудить нужный work path. Cadence нельзя использовать как способ обнаружить
уже случившееся взаимодействие.

### 18.5 Save

```text
close admission/save request -> owner save barriers
-> finalized watermarks + immutable snapshots
-> disk writes outside owner loops -> conditional clean/ACK
```

Save не считается завершенным после запуска IO task. Нужны completion events от
entity journal, world dirty flush и player persistence.

## 19. Многопоточность, cache и SIMD

### 19.1 Где есть параллелизм

- Tokio: sockets, connection tasks, async composition;
- SimulationOwner: единый порядок cross-domain commits;
- regional lanes: независимые entity stores и owner-local mutation;
- Rayon/compute workers: pathfinding, physics, lighting/worldgen batches;
- disk/script threads: blocking IO и Lua изолированы от network runtime.

### 19.2 Где locks остаются

Locks допустимы для короткого доступа к sharded snapshot/index state. Они не
должны охватывать socket write, disk IO, pathfinding, Lua или ожидание другого
owner. Transitional world lock и coordinator global indexes остаются основными
местами для уменьшения critical sections.

### 19.3 Cache discipline

SoA queries, packed section arrays, precompiled data lookup и reusable scratch
сокращают pointer chasing и allocation. Snapshot DTO удобен на границах, но
массовое clone строк/maps на каждом tick является долгом. Для hot loops план:
numeric IDs, dense component columns, scratch reuse, batch-by-region/type и
publication только dirty fields.

### 19.4 SIMD discipline

SIMD включается только при измеримом плотном kernel: light nibble scans,
height/occupancy masks, homogeneous component arithmetic. Runtime dispatch
выбирает поддержанный backend; scalar implementation остается correctness
oracle. AI branching, hash lookup и сетевые state machines не становятся
быстрее от формального использования intrinsics.

## 20. Известные pitfalls и план решения

| Приоритет | Pitfall | Почему опасно | План |
| --- | --- | --- | --- |
| P0 | staged entity kernels не подключены к live path | unit parity не меняет поведение игроков/мобов | закончить review, затем vertical cutover по одному семейству: state -> owner transaction -> publication -> persistence -> strict harness |
| P0 | strict entity vanilla gate еще не закрыт | можно повторить арифметику, но ошибиться в packet order/metadata/collision | починить multiplicity/order и exact fences в harness, запустить Solaris и vanilla side-by-side, хранить только наше ожидаемое evidence |
| P0 | equipment/effects callbacks могут разойтись с commit | дюп/потеря item или невидимый state | единый transaction outcome + durable/retryable outbox; baseline только после ACK |
| P0 | часть world mutations еще под transitional lock | global contention и риск разных authority paths | переносить server-origin ticks в regional mutation commands; запретить raw mutation из network/tasks |
| P1 | coordinator сериализует global entity indexes | lanes параллельны, но throughput упирается в actor | shard location/UUID directory, оставив глобальный sequence allocator и cross-shard decision protocol |
| P1 | `play.rs` и `simulation.rs` остаются крупными facades | трудно доказать ownership и sad paths | извлекать domain modules с typed request/result; facade оставлять только router/composition |
| P1 | mob movement имеет несколько исторических writers | jitter, круги, прыжки и stale corrections | один locomotion owner; AI goal -> control -> physics -> commit; packet publication только после final pose |
| P1 | collision parity неполна для сложных shapes | entity визуально вязнет в грядках/ступенях | data-driven voxel shapes, vanilla step-up oracle, real-client traversal matrix |
| P1 | persistence schema развивается | silent unit conversion портит velocity/state | оставить один текущий explicit Solaris schema и fail closed missing/unknown/duplicate; старые unreleased schemas удалить, vanilla world fixtures проверять round trip |
| P1 | fallback data покрывает не весь vanilla catalog | custom maps получают unknown blocks/entities/items | полная извлеченная registry/data inventory, opaque preservation и явный unsupported report |
| P1 | Lua production adapters ещё неполны | 0.6 DTO принимаются host-ом, но storage/menu/zones/colonies не должны молча теряться в router | attested router, durable storage, atomic inventory transaction и event-driven zone/colony owners с targeted results |
| P1 | snapshot clones и строковые IDs на hot paths | cache misses/allocation при десятках тысяч entities | numeric interned IDs, compact SoA views, per-lane scratch, dirty-field publication; измерять до/после |
| P1 | autoscaler меняет lanes, но не вся работа региональна | scale-up не дает линейного роста | измерить queue/service time по subsystem, регионализировать dominant writers, bounded work stealing только для pure compute |
| P2 | worldgen не полный vanilla | новые миры отличаются, structures ограничены | сначала загрузка custom maps и gameplay-critical generation, затем NoiseRouter/biomes/structures по oracle |
| P2 | простые Java content mods не имеют server bridge | custom items/blocks требуют ручного порта | data-pack-like IR/transpiler для registries/recipes/loot/assets; логические mods только через явный plugin API |
| P2 | тесты могут доказать DTO, но не ощущения клиента | animation/light/menu rollback остаются незамеченными | MCP real-client scenarios с protocol/world/GUI events; screenshot только дополнительное визуальное evidence |
| P2 | редкие stalls плохо видны коротким тестом | средний TPS скрывает tail latency | bounded soak с p95/p99/max queue/service metrics и event-triggered stall dump; без sleep-based success |

Порядок выбран по Pareto: сначала live entity transaction и strict клиентский
gate, затем устранение dominant serialization/jitter, и только после этого
длинный хвост редких mechanics.

## 21. Что именно доказывают тесты

| Evidence | Что можно утверждать |
| --- | --- |
| unit/domain test | конкретное pure правило и sad branches |
| owner/concurrency test | linearization, stale rejection, cancellation/rollback, bounded queues |
| wire harness | packet layout/order и наблюдаемая server response |
| vanilla oracle | совпадение с конкретным source/capture 26.1.2 |
| real-client MCP/manual | клиент действительно принимает state и workflow работает |
| benchmark/soak | throughput/tail latency только на указанной сборке, машине и workload |

`xtask code-health` доказывает архитектурные запреты, но не gameplay. Зеленые
unit tests staged kernel не доказывают live cutover. Один успешный ручной вход
не доказывает multiplayer, persistence или autoscale.

## 22. Как читать и обновлять этот документ

Для конкретного бага сначала находить packet/domain adapter через `rg`, затем
owner mutation и publication, а не читать crate целиком. Для архитектурного
решения сверяться с ADR 0004, 0005 и 0006. Для parity смотреть local oracle и
harness evidence.

В репозитории тысячи функций, включая getters, trait implementations, codecs и
tests. Здесь каждая production/staged orchestration и domain function family
имеет явную ответственность; однотипные механические accessors и packet
`encode/decode` описаны общим контрактом соответствующего типа. При добавлении
новой authority, очереди, persistence schema или live cutover этот документ и
соответствующий ADR должны обновляться в той же работе.
