//! Settlement dialog text in English and Russian.
//!
//! The edge carries the client's `ClientInformation.language` into the core as
//! [`PlayerInput::SetLocale`](crate::boundary::PlayerInput::SetLocale); [`Lang`]
//! selects Russian for every `ru*` locale and English for everything else.
//! One table, one key per user-visible string: `fill` substitutes `{name}`
//! placeholders, so translated word order and punctuation stay in the table.
//!
//! Game data (item, blueprint, profession and settlement names, certificates'
//! missing-material descriptions) is never translated here.

/// Dialog language chosen from the client locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    En,
    Ru,
}

impl Lang {
    /// `ru`, `ru_ru`, `ru_RU` select Russian; any other or unknown locale is English.
    #[must_use]
    pub(crate) fn of(locale: Option<&str>) -> Self {
        match locale.and_then(|locale| locale.get(..2)) {
            Some(prefix) if prefix.eq_ignore_ascii_case("ru") => Self::Ru,
            _ => Self::En,
        }
    }
}

/// Every user-visible settlement dialog string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Close,
    Back,
    ViewOnly,
    Denied,
    PositionUnknown,
    NoSettlement,
    AdmissionUnavailable,
    Pages,
    SettlementGone,

    KindHouse,
    KindStorage,
    KindMine,
    KindLumberCamp,
    KindBarracks,
    KindWall,
    KindTower,
    KindGate,
    KindWorkshop,
    CategoryHousing,
    CategoryStorage,
    CategoryProduction,
    CategoryMilitary,
    CategoryFortification,

    TitleSettlement,
    Autonomous,
    UnclaimedBody,
    Claim,
    Buildings,
    ResidentsJobs,
    Projects,
    ProductionOrders,
    Army,
    Settings,
    NoneYet,
    MainBody,
    Stranded,
    ViewOnlyOwner,
    StorageNone,
    Empty,
    StorageCount,
    ItemCount,

    TitleBuildings,
    JobsRow,
    MaxLevel,
    ReadyFor,
    NeedsNext,
    UpgradeTo,
    More,
    NoBuildings,
    CertifyNew,

    TitleCertify,
    Certify,
    TipCorners,
    Corner1,
    TipCorner1,
    Corner2,
    TipCorner2,
    LabelKind,
    LabelFrom,
    LabelTo,
    LabelExtra,
    ChooseKind,
    BadCorners,
    InvalidCorners,
    Certified,
    CannotCertify,
    BuildingGone,
    Upgraded,
    CannotUpgrade,
    CornerSet,

    SoldiersRow,
    JobsFilledRow,
    FreeSlots,
    NoJob,
    WorksAt,
    SoldierDuty,
    TitleResidents,

    JobCancelled,
    JobDone,
    JobPaused,
    JobWaiting,
    JobWorking,
    OrderRow,
    Resume,
    Pause,
    Cancel,
    NoOrders,
    NewOrderHelp,
    NewOrder,
    TitleOrders,
    NoStation,
    StationRow,
    ChooseStation,
    WhereWork,
    StationGone,
    StationUnloaded,
    NoFreeResident,
    TitleNewOrder,
    PickProduct,
    Quantity,
    Resident,
    FailureBusy,
    FailureNotFound,
    FailureForbidden,
    FailureBlocked,
    FailureUnloaded,
    FailureOther,
    OrderIncomplete,
    WorkshopGone,
    WrongStation,
    NoStand,
    OrderStarted,
    OrderGone,
    OrderUpdated,

    NoSoldiers,
    Recruit,
    FollowMe,
    GuardHere,
    Patrol,
    Return,
    Dismiss,
    FollowMeAll,
    GuardHereAll,
    PatrolAll,
    ReturnAll,
    TitleArmy,
    TitleSoldier,
    SoldierButton,
    Recruited,
    Dismissed,
    SoldierGone,
    ChooseOrder,
    Ordered,

    OrderBlueprint,
    UpgradeRow,
    TitleProjects,
    ChooseBlueprint,
    PendingOrder,
    PendingBusy,
    PendingMaterials,
    PendingSite,
    CancelPending,
    PendingCancelled,
    Status,
    TitleProjectStatus,

    SettingsBody,
    Rename,
    TitleSettings,
    NewName,
    EnterName,
    Renamed,
}

/// English and Russian text for one key.
const fn pair(key: Key) -> (&'static str, &'static str) {
    match key {
        Key::Close => ("Close", "Закрыть"),
        Key::Back => ("Back", "Назад"),
        Key::ViewOnly => ("View only.", "Только просмотр."),
        Key::Denied => (
            "Only the owner or an operator can change this settlement.",
            "Менять это поселение может только владелец или оператор.",
        ),
        Key::PositionUnknown => ("Your position is unknown.", "Ваша позиция неизвестна."),
        Key::NoSettlement => (
            "No settlement here. Stand inside one, or found one with /settlement create <name> [radius].",
            "Здесь нет поселения. Встаньте внутрь или основайте своё: /settlement create <name> [radius].",
        ),
        Key::AdmissionUnavailable => (
            "Settlement command admission is unavailable.",
            "Приём команд поселения недоступен.",
        ),
        Key::Pages => (
            "Pages: main, buildings, certify, residents, production, army, projects, settings.",
            "Страницы: main, buildings, certify, residents, production, army, projects, settings.",
        ),
        Key::SettlementGone => (
            "That settlement no longer exists.",
            "Это поселение больше не существует.",
        ),

        Key::KindHouse => ("House", "Дом"),
        Key::KindStorage => ("Storage", "Склад"),
        Key::KindMine => ("Mine", "Шахта"),
        Key::KindLumberCamp => ("Lumber camp", "Лесопилка"),
        Key::KindBarracks => ("Barracks", "Казарма"),
        Key::KindWall => ("Wall section", "Секция стены"),
        Key::KindTower => ("Tower", "Башня"),
        Key::KindGate => ("Gate", "Ворота"),
        Key::KindWorkshop => ("{profession} workshop", "Мастерская: {profession}"),
        Key::CategoryHousing => ("Housing", "Жильё"),
        Key::CategoryStorage => ("Storage", "Склад"),
        Key::CategoryProduction => ("Production", "Производство"),
        Key::CategoryMilitary => ("Military", "Военное"),
        Key::CategoryFortification => ("Fortification", "Укрепления"),

        Key::TitleSettlement => ("Settlement {name}", "Поселение {name}"),
        Key::Autonomous => ("Autonomous (unclaimed)", "Автономное (без владельца)"),
        Key::UnclaimedBody => (
            "Autonomous (unclaimed)\n{residents} residents · {buildings} buildings\nClaim while inside this village.",
            "Автономное (без владельца)\nЖителей: {residents} · построек: {buildings}\nПрисвойте, стоя внутри деревни.",
        ),
        Key::Claim => ("Claim settlement", "Присвоить поселение"),
        Key::Buildings => ("Buildings", "Постройки"),
        Key::ResidentsJobs => ("Residents & jobs", "Жители и работа"),
        Key::Projects => ("Projects", "Стройка"),
        Key::ProductionOrders => ("Production orders", "Заказы производства"),
        Key::Army => ("Army", "Армия"),
        Key::Settings => ("Settings", "Настройки"),
        Key::NoneYet => ("none yet", "пока нет"),
        Key::MainBody => (
            "Owner: {owner}\nRadius: {radius} blocks around {x} {y} {z}\nResidents: {residents} ({working} working, {idle} idle)\nBuildings: {certified}\n{storage}",
            "Владелец: {owner}\nРадиус: {radius} блоков вокруг {x} {y} {z}\nЖители: {residents} ({working} работают, {idle} без дела)\nПостройки: {certified}\n{storage}",
        ),
        Key::Stranded => (
            "{stranded} workers need a certified workshop — open Buildings → Certify",
            "{stranded} рабочим нужна сертифицированная мастерская — откройте «Постройки» → «Сертифицировать»",
        ),
        Key::ViewOnlyOwner => (
            "View only: the owner or an operator manages this settlement.",
            "Только просмотр: поселением управляет владелец или оператор.",
        ),
        Key::StorageNone => ("Storage: none certified", "Склад: нет сертифицированного"),
        Key::Empty => ("empty", "пусто"),
        Key::StorageCount => (
            "Storage: {containers} containers; {items}",
            "Склад: контейнеров {containers}; {items}",
        ),
        Key::ItemCount => ("{name} x{count}", "{name} ×{count}"),

        Key::TitleBuildings => ("Buildings - {name}", "Постройки — {name}"),
        Key::JobsRow => (" jobs {filled}/{slots}", " рабочие места {filled}/{slots}"),
        Key::MaxLevel => (" - max level", " — максимальный уровень"),
        Key::ReadyFor => (" - ready for L{next}", " — готово к L{next}"),
        Key::NeedsNext => (
            " - L{next} needs: {missing}",
            " — для L{next} нужно: {missing}",
        ),
        Key::UpgradeTo => ("Upgrade #{seq} to L{next}", "Улучшить #{seq} до L{next}"),
        Key::More => ("(+{n} more)", "(ещё {n})"),
        Key::NoBuildings => (
            "No certified buildings yet. Build freely, then certify the finished building: the settlement checks its blocks and size.",
            "Сертифицированных построек пока нет. Стройте свободно, затем сертифицируйте готовую постройку: поселение проверит её блоки и размер.",
        ),
        Key::CertifyNew => ("Certify new building", "Сертифицировать постройку"),

        Key::TitleCertify => ("Certify building", "Сертификация постройки"),
        Key::Certify => ("Certify", "Сертифицировать"),
        Key::TipCorners => (
            "Corners are two opposite corners of the finished building, inclusive. Height adds blocks above the higher corner.",
            "Углы — две противоположные вершины готовой постройки, включительно. Высота добавляет блоки выше верхнего угла.",
        ),
        Key::Corner1 => ("Corner 1 here", "Угол 1 = моя позиция"),
        Key::TipCorner1 => (
            "Stand in a corner block and press this; walk to the opposite corner, then /settlement menu certify and press Corner 2 here.",
            "Встаньте в угловой блок и нажмите; перейдите в противоположный угол, затем /settlement menu certify и нажмите «Угол 2 = моя позиция».",
        ),
        Key::Corner2 => ("Corner 2 here", "Угол 2 = моя позиция"),
        Key::TipCorner2 => (
            "Stand in the opposite corner block and press this.",
            "Встаньте в противоположный угловой блок и нажмите.",
        ),
        Key::LabelKind => ("Building type", "Тип постройки"),
        Key::LabelFrom => ("Corner 1 (x y z)", "Угол 1 (x y z)"),
        Key::LabelTo => ("Corner 2 (x y z)", "Угол 2 (x y z)"),
        Key::LabelExtra => ("Height above corners", "Высота над углами"),
        Key::ChooseKind => ("Choose a building type.", "Выберите тип постройки."),
        Key::BadCorners => (
            "Corners must be three whole numbers: x y z.",
            "Углы задаются тремя целыми числами: x y z.",
        ),
        Key::InvalidCorners => ("Invalid corners.", "Неверные углы."),
        Key::Certified => (
            "Certified {kind} #{id} (level 1).",
            "Постройка сертифицирована: {kind} #{id} (уровень 1).",
        ),
        Key::CannotCertify => (
            "Cannot certify {kind}. Missing: {missing}.",
            "Не удалось сертифицировать {kind}. Не хватает: {missing}.",
        ),
        Key::BuildingGone => (
            "That building no longer exists.",
            "Эта постройка больше не существует.",
        ),
        Key::Upgraded => (
            "Building #{id} upgraded to level {level}.",
            "Постройка #{id} улучшена до уровня {level}.",
        ),
        Key::CannotUpgrade => (
            "Cannot upgrade #{id}. Missing: {missing}.",
            "Не удалось улучшить #{id}. Не хватает: {missing}.",
        ),
        Key::CornerSet => ("Corner {n} = {at}.", "Угол {n} = {at}."),

        Key::SoldiersRow => (
            "#{id} {kind} L{level}: {filled}/{beds} soldiers",
            "#{id} {kind} L{level}: солдат {filled}/{beds}",
        ),
        Key::JobsFilledRow => (
            "#{id} {kind} L{level}: {filled}/{slots} jobs filled",
            "#{id} {kind} L{level}: занято мест {filled}/{slots}",
        ),
        Key::FreeSlots => (
            "Free job slots: {free}; unemployed residents: {people}",
            "Свободных мест: {free}; безработных жителей: {people}",
        ),
        Key::NoJob => ("no job", "без работы"),
        Key::WorksAt => ("works at #{id} {kind}", "работает в #{id} {kind}"),
        Key::SoldierDuty => ("; soldier {order}", "; солдат {order}"),
        Key::TitleResidents => ("Residents & jobs - {name}", "Жители и работа — {name}"),

        Key::JobCancelled => ("cancelled", "отменён"),
        Key::JobDone => ("done", "готово"),
        Key::JobPaused => ("paused", "пауза"),
        Key::JobWaiting => ("waiting: {reason}", "ждёт: {reason}"),
        Key::JobWorking => ("working", "в работе"),
        Key::OrderRow => (
            "{id}: {recipe} {done}/{count} by {worker} - {status}",
            "{id}: {recipe} {done}/{count}, {worker} — {status}",
        ),
        Key::Resume => ("Resume {id}", "Продолжить {id}"),
        Key::Pause => ("Pause {id}", "Пауза {id}"),
        Key::Cancel => ("Cancel {id}", "Отменить {id}"),
        Key::NoOrders => (
            "No production orders yet.",
            "Заказов производства пока нет.",
        ),
        Key::NewOrderHelp => (
            "A new order sends one resident to a crafting table inside a certified workshop; the resident crafts from the ingredients in their own inventory.",
            "Новый заказ отправляет жителя к верстаку в сертифицированной мастерской; житель крафтит из ингредиентов своего инвентаря.",
        ),
        Key::NewOrder => ("New order", "Новый заказ"),
        Key::TitleOrders => ("Production orders - {name}", "Заказы производства — {name}"),
        Key::NoStation => (
            "No crafting table inside a certified workshop. Place a crafting table in a workshop and try again.",
            "В сертифицированной мастерской нет верстака. Поставьте верстак и повторите.",
        ),
        Key::StationRow => (
            "#{id} {kind}: {block} at {at}",
            "#{id} {kind}: {block} в {at}",
        ),
        Key::ChooseStation => ("Choose a workstation", "Выбор рабочего места"),
        Key::WhereWork => (
            "Where should the resident work?",
            "Где должен работать житель?",
        ),
        Key::StationGone => ("That workstation is gone.", "Это рабочее место исчезло."),
        Key::StationUnloaded => (
            "That workstation is not loaded.",
            "Это рабочее место не загружено.",
        ),
        Key::NoFreeResident => (
            "No free resident: bind villagers with /settlement bind, or wait for current orders to finish.",
            "Нет свободных жителей: привяжите жителей через /settlement bind или дождитесь окончания текущих заказов.",
        ),
        Key::TitleNewOrder => ("New production order", "Новый заказ производства"),
        Key::PickProduct => (
            "Pick the product, a quantity and the resident. Crafting table at {at}.",
            "Выберите продукт, количество и жителя. Верстак в {at}.",
        ),
        Key::Quantity => ("Quantity", "Количество"),
        Key::Resident => ("Resident", "Житель"),
        Key::FailureBusy => (
            "That resident already has an active order.",
            "У этого жителя уже есть активный заказ.",
        ),
        Key::FailureNotFound => (
            "Resident, workshop or recipe not found.",
            "Житель, мастерская или рецепт не найдены.",
        ),
        Key::FailureForbidden => (
            "That resident does not belong to this settlement.",
            "Этот житель не принадлежит этому поселению.",
        ),
        Key::FailureBlocked => (
            "The workstation is missing or blocked.",
            "Рабочее место отсутствует или заблокировано.",
        ),
        Key::FailureUnloaded => ("The workshop is not loaded.", "Мастерская не загружена."),
        Key::FailureOther => ("Refused: {reason}.", "Отказано: {reason}."),
        Key::OrderIncomplete => (
            "The order form was incomplete.",
            "Форма заказа заполнена не полностью.",
        ),
        Key::WorkshopGone => (
            "That workshop no longer exists.",
            "Эта мастерская больше не существует.",
        ),
        Key::WrongStation => (
            "That recipe needs a different workstation than the one chosen.",
            "Этому рецепту нужно другое рабочее место, не выбранное.",
        ),
        Key::NoStand => (
            "No free standing cell next to that workstation.",
            "Рядом с этим рабочим местом нет свободной клетки.",
        ),
        Key::OrderStarted => (
            "Order {job} started: {item} x{count}.",
            "Заказ {job} начат: {item} ×{count}.",
        ),
        Key::OrderGone => (
            "That order no longer exists.",
            "Этот заказ больше не существует.",
        ),
        Key::OrderUpdated => ("Order {job} updated.", "Заказ {job} обновлён."),

        Key::NoSoldiers => (
            "No soldiers. Recruit an idle adult; equipment comes from settlement storage.",
            "Солдат нет. Наймите свободного взрослого: снаряжение берётся со склада поселения.",
        ),
        Key::Recruit => ("Recruit soldier", "Нанять солдата"),
        Key::FollowMe => ("Follow me", "Следовать за мной"),
        Key::GuardHere => ("Guard here", "Охранять здесь"),
        Key::Patrol => ("Patrol", "Патруль"),
        Key::Return => ("Return", "Вернуться"),
        Key::Dismiss => ("Dismiss", "Уволить"),
        Key::FollowMeAll => ("Follow me (all)", "Следовать за мной (все)"),
        Key::GuardHereAll => ("Guard here (all)", "Охранять здесь (все)"),
        Key::PatrolAll => ("Patrol (all)", "Патруль (все)"),
        Key::ReturnAll => ("Return (all)", "Вернуться (все)"),
        Key::TitleArmy => ("Army - {name}", "Армия — {name}"),
        Key::TitleSoldier => ("Soldier {id} - {name}", "Солдат {id} — {name}"),
        Key::SoldierButton => ("Soldier {id}", "Солдат {id}"),
        Key::Recruited => ("Recruited {uuid}.", "Нанят {uuid}."),
        Key::Dismissed => ("Dismissed {uuid}.", "Уволен {uuid}."),
        Key::SoldierGone => (
            "That soldier no longer exists.",
            "Этот солдат больше не существует.",
        ),
        Key::ChooseOrder => ("Choose an order.", "Выберите приказ."),
        Key::Ordered => (
            "Ordered {count} soldier(s).",
            "Приказ отдан: солдат {count}.",
        ),

        Key::OrderBlueprint => ("Order: {blueprint}", "Заказ: {blueprint}"),
        Key::UpgradeRow => (
            "Upgrade #{id} ({kind} L{level}) to L{next}",
            "Улучшить #{id} ({kind} L{level}) до L{next}",
        ),
        Key::TitleProjects => ("Projects - {name}", "Стройка — {name}"),
        Key::ChooseBlueprint => (
            "Choose a blueprint to order.",
            "Выберите чертёж для заказа.",
        ),
        Key::PendingOrder => (
            "Pending: {blueprint} L{level} — {reason}",
            "В очереди: {blueprint} ур. {level} — {reason}",
        ),
        Key::PendingBusy => ("busy", "идёт другая стройка"),
        Key::PendingMaterials => ("waiting for materials", "не хватает материалов"),
        Key::PendingSite => ("waiting for a site", "ожидание места для стройки"),
        Key::CancelPending => ("Cancel pending order", "Отменить заказ в очереди"),
        Key::PendingCancelled => ("Pending order cancelled.", "Заказ в очереди отменён."),
        Key::Status => ("Status", "Статус"),
        Key::TitleProjectStatus => ("Project status - {name}", "Статус стройки — {name}"),

        Key::SettingsBody => (
            "Name: {name}\nRadius: {radius} blocks (grows with population)\nRevision: {revision}",
            "Название: {name}\nРадиус: {radius} блоков (растёт с населением)\nРевизия: {revision}",
        ),
        Key::Rename => ("Rename", "Переименовать"),
        Key::TitleSettings => ("Settings - {name}", "Настройки — {name}"),
        Key::NewName => (
            "New name (letters, digits, - _)",
            "Новое имя (буквы, цифры, - _)",
        ),
        Key::EnterName => ("Enter a name.", "Введите имя."),
        Key::Renamed => ("Renamed to {name}.", "Переименовано в {name}."),
    }
}

/// Text for `key` in `lang`.
#[must_use]
pub(crate) fn text(lang: Lang, key: Key) -> &'static str {
    let (english, russian) = pair(key);
    match lang {
        Lang::En => english,
        Lang::Ru => russian,
    }
}

/// Substitute `{name}` placeholders; unknown placeholders stay verbatim.
#[must_use]
pub(crate) fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let key = &tail[..close];
        match args.iter().find(|(name, _)| *name == key) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(key);
                out.push('}');
            }
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_prefix_selects_russian_and_everything_else_is_english() {
        assert_eq!(Lang::of(Some("ru_ru")), Lang::Ru);
        assert_eq!(Lang::of(Some("ru")), Lang::Ru);
        assert_eq!(Lang::of(Some("ru_RU")), Lang::Ru);
        assert_eq!(Lang::of(Some("en_us")), Lang::En);
        assert_eq!(Lang::of(Some("de_de")), Lang::En);
        assert_eq!(Lang::of(Some("")), Lang::En);
        assert_eq!(Lang::of(None), Lang::En);
    }

    #[test]
    fn fill_replaces_known_placeholders_only() {
        assert_eq!(
            fill("Owner: {owner} ({n})", &[("owner", "Mayor")]),
            "Owner: Mayor ({n})"
        );
        assert_eq!(fill("no holes", &[]), "no holes");
        assert_eq!(fill("{a}{a}", &[("a", "x")]), "xx");
    }

    #[test]
    fn known_navigation_strings_are_translated() {
        assert_eq!(text(Lang::En, Key::Back), "Back");
        assert_eq!(text(Lang::Ru, Key::Back), "Назад");
        assert_eq!(text(Lang::Ru, Key::Army), "Армия");
        assert_eq!(text(Lang::Ru, Key::Claim), "Присвоить поселение");
    }
}
