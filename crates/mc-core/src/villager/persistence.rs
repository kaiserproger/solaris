//! 26.1.2 entity save schema, verified from the local Mojang client jar's
//! Villager, ZombieVillager, InventoryCarrier, MerchantOffer and Brain codecs.
//! Canonical item codecs retain every mc-data component. Decoding is transactional:
//! malformed present fields return an error, never a partially cleared villager.

use super::{
    Activity, Gossip, VillagerData, VillagerKind, VillagerProfession, VillagerState,
    VillagerTradeOffer,
};
use crate::boundary::{BlockPos, EntityId};
use mc_data::{
    ItemStack,
    item_nbt::{stack_from_fields, stack_to_fields},
    items::ItemRegistry,
};
use mc_nbt::{ListTag, Tag, tag_type};

#[derive(Debug)]
pub(crate) enum VillagerNbtError {
    Invalid(&'static str),
    Item(mc_data::item_nbt::ItemNbtError),
}
impl std::fmt::Display for VillagerNbtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(field) => write!(f, "invalid villager NBT: {field}"),
            Self::Item(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for VillagerNbtError {}
impl From<mc_data::item_nbt::ItemNbtError> for VillagerNbtError {
    fn from(error: mc_data::item_nbt::ItemNbtError) -> Self {
        Self::Item(error)
    }
}
type Result<T, E = VillagerNbtError> = std::result::Result<T, E>;
type Fields = [(String, Tag)];

fn field<'a>(fields: &'a Fields, name: &'static str) -> Result<Option<&'a Tag>> {
    let mut matches = fields.iter().filter(|(key, _)| key == name);
    let value = matches.next().map(|(_, value)| value);
    if matches.next().is_some() {
        return Err(VillagerNbtError::Invalid(name));
    }
    Ok(value)
}
fn compound<'a>(tag: &'a Tag, name: &'static str) -> Result<&'a Fields> {
    match tag {
        Tag::Compound(fields) => Ok(fields),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn integer(fields: &Fields, name: &'static str, default: i32) -> Result<i32> {
    match field(fields, name)? {
        None => Ok(default),
        Some(Tag::Int(value)) => Ok(*value),
        Some(Tag::Byte(value)) => Ok(i32::from(*value)),
        Some(Tag::Short(value)) => Ok(i32::from(*value)),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn long(fields: &Fields, name: &'static str, default: i64) -> Result<i64> {
    match field(fields, name)? {
        None => Ok(default),
        Some(Tag::Long(value)) => Ok(*value),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn boolean(fields: &Fields, name: &'static str, default: bool) -> Result<bool> {
    match field(fields, name)? {
        None => Ok(default),
        Some(Tag::Byte(0)) => Ok(false),
        Some(Tag::Byte(1)) => Ok(true),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn string<'a>(fields: &'a Fields, name: &'static str, default: &'a str) -> Result<&'a str> {
    match field(fields, name)? {
        None => Ok(default),
        Some(Tag::String(value)) => Ok(value),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn list<'a>(fields: &'a Fields, name: &'static str) -> Result<&'a [Tag]> {
    match field(fields, name)? {
        None => Ok(&[]),
        Some(Tag::List(list))
            if list.element_type == tag_type::COMPOUND
                || (list.element_type == tag_type::END && list.elements.is_empty()) =>
        {
            Ok(&list.elements)
        }
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn compound_list(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    })
}

/// Solaris worker cursors ride the runtime compound as compact JSON strings;
/// the NBT path is cold, so the serde allocation stays off the hot tick.
/// Absent means default; any present-but-undecodable value fails closed.
fn progress_to_tag<T: Default + PartialEq + serde::Serialize>(
    progress: &T,
    name: &'static str,
) -> Result<Option<Tag>> {
    if *progress == T::default() {
        return Ok(None);
    }
    serde_json::to_string(progress)
        .map(|json| Some(Tag::String(json)))
        .map_err(|_| VillagerNbtError::Invalid(name))
}

fn progress_from_tag<T: Default + for<'de> serde::Deserialize<'de>>(
    runtime: &Fields,
    name: &'static str,
) -> Result<T> {
    match field(runtime, name)? {
        None => Ok(T::default()),
        Some(Tag::String(json)) => {
            serde_json::from_str(json).map_err(|_| VillagerNbtError::Invalid(name))
        }
        Some(_) => Err(VillagerNbtError::Invalid(name)),
    }
}
fn tick_value(value: i64, name: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| VillagerNbtError::Invalid(name))
}
fn tick_tag(value: u64, name: &'static str) -> Result<Tag> {
    i64::try_from(value)
        .map(Tag::Long)
        .map_err(|_| VillagerNbtError::Invalid(name))
}
fn uuid_tag(uuid: uuid::Uuid) -> Tag {
    Tag::IntArray(
        uuid.as_bytes()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| i32::from_be_bytes(*chunk))
            .collect(),
    )
}
fn uuid_from_tag(tag: &Tag, name: &'static str) -> Result<uuid::Uuid> {
    let Tag::IntArray(values) = tag else {
        return Err(VillagerNbtError::Invalid(name));
    };
    if values.len() != 4 {
        return Err(VillagerNbtError::Invalid(name));
    }
    let mut bytes = [0; 16];
    for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    Ok(uuid::Uuid::from_bytes(bytes))
}
fn position(tag: &Tag, name: &'static str) -> Result<BlockPos> {
    match tag {
        Tag::IntArray(values) if values.len() == 3 => Ok(BlockPos {
            x: values[0],
            y: values[1],
            z: values[2],
        }),
        _ => Err(VillagerNbtError::Invalid(name)),
    }
}
fn position_tag(pos: BlockPos) -> Tag {
    Tag::IntArray(vec![pos.x, pos.y, pos.z])
}
fn dimension_valid(value: &str) -> bool {
    mc_data::Identifier::parse(value.to_owned()).is_ok()
}
fn gossip_valid(gossip: &Gossip) -> bool {
    gossip.value > 0
        && matches!(
            gossip.kind.as_str(),
            "major_negative" | "minor_negative" | "minor_positive" | "major_positive" | "trading"
        )
}
fn stack_valid(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && stack.count > 0
        && stack.count <= 64
        && stack.damage.is_none_or(|damage| damage >= 0)
        && stack
            .enchantments
            .iter()
            .all(|enchantment| (1..=255).contains(&enchantment.level))
}

/// Shared validation for NBT and journal after-images; never repair/drop custody.
pub(crate) fn validate_state(state: &VillagerState) -> Result<()> {
    if state.settlement.is_some_and(|id| id.0 == 0) {
        return Err(VillagerNbtError::Invalid("settlement"));
    }
    if state.data.level == 0 {
        return Err(VillagerNbtError::Invalid("VillagerData.level"));
    }
    if state.xp < 0 || !(0..=127).contains(&state.food_level) || state.restocks_today < 0 {
        return Err(VillagerNbtError::Invalid("villager counters"));
    }
    if state.mining_progress.layer > 2 {
        return Err(VillagerNbtError::Invalid("SolarisMiningProgress.layer"));
    }
    if (state.data.profession == VillagerProfession::Soldier) != state.soldier.is_some()
        || state.soldier.as_ref().is_some_and(|soldier| {
            soldier.barracks.0 & u64::from(u32::MAX) == 0
                || state
                    .settlement
                    .is_none_or(|id| soldier.barracks.0 >> 32 != id.0)
        })
    {
        return Err(VillagerNbtError::Invalid("SolarisSoldier"));
    }
    if ![
        &state.job_site_dimension,
        &state.home_dimension,
        &state.meeting_point_dimension,
    ]
    .into_iter()
    .all(|dimension| dimension_valid(dimension))
    {
        return Err(VillagerNbtError::Invalid("Brain.dimension"));
    }
    if state.gossips.iter().any(|gossip| !gossip_valid(gossip)) {
        return Err(VillagerNbtError::Invalid("Gossips"));
    }
    if state
        .inventory
        .iter()
        .flatten()
        .any(|stack| !stack_valid(stack))
    {
        return Err(VillagerNbtError::Invalid("Inventory"));
    }
    for offer in &state.offers {
        if !stack_valid(&offer.cost_a)
            || !stack_valid(&offer.result)
            || offer
                .cost_b
                .as_ref()
                .is_some_and(|stack| !stack_valid(stack))
            || offer.uses < 0
            || offer.max_uses <= 0
            || offer.xp < 0
            || !offer.price_multiplier.is_finite()
            || offer.price_multiplier < 0.0
        {
            return Err(VillagerNbtError::Invalid("Offers.Recipes"));
        }
    }
    if state.conversion_time.is_some_and(|time| time < -1)
        || (state.conversion_time.is_none() && state.conversion_player.is_some())
    {
        return Err(VillagerNbtError::Invalid("ConversionTime"));
    }
    if state
        .household
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 64)
        || (state.household.is_some() && state.settlement.is_none())
    {
        return Err(VillagerNbtError::Invalid("household"));
    }
    match (&state.birth_origin, state.birth_sequence) {
        (Some(origin), Some(sequence)) => {
            origin
                .validate()
                .map_err(|_| VillagerNbtError::Invalid("birth_origin"))?;
            if sequence / 65_536 != origin.birth_tick || sequence % 65_536 == 0 {
                return Err(VillagerNbtError::Invalid("birth_sequence"));
            }
        }
        (None, None) => {}
        _ => return Err(VillagerNbtError::Invalid("birth_origin/birth_sequence")),
    }
    Ok(())
}

pub(crate) fn deserialize_optional_state<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> std::result::Result<Option<VillagerState>, D::Error> {
    use serde::Deserialize;
    let state = Option::<VillagerState>::deserialize(decoder)?;
    if let Some(state) = &state {
        validate_state(state).map_err(serde::de::Error::custom)?;
    }
    Ok(state)
}

pub(crate) mod entity_id_option {
    use crate::boundary::EntityId;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        value: &Option<EntityId>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        value.map(|id| id.0).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        decoder: D,
    ) -> std::result::Result<Option<EntityId>, D::Error> {
        Ok(Option::<i32>::deserialize(decoder)?.map(EntityId))
    }
}

pub(crate) mod block_pos_option {
    use crate::boundary::BlockPos;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        value: &Option<BlockPos>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        value.map(|pos| [pos.x, pos.y, pos.z]).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        decoder: D,
    ) -> std::result::Result<Option<BlockPos>, D::Error> {
        Ok(Option::<[i32; 3]>::deserialize(decoder)?.map(|[x, y, z]| BlockPos { x, y, z }))
    }
}

/// Project deterministic countdowns and continuously refreshed memories from
/// their tick anchor. Events, resets and clock admission changes still differ
/// from this projection and therefore retain an absolute image.
pub(crate) fn advance_clocks(state: &mut VillagerState, from: u64, to: u64) {
    let elapsed = to.saturating_sub(from);
    if state.brain_clock && !state.wandering && state.conversion_time.is_none() {
        let delta = elapsed.min(u64::from(u32::MAX)) as i64;
        state.age = if state.age < 0 {
            (i64::from(state.age) + delta).min(0) as i32
        } else {
            (i64::from(state.age) - delta).max(0) as i32
        };
        let previous = i64::try_from(from.saturating_sub(1)).unwrap_or(i64::MAX);
        if state.activity == Activity::Work && state.last_work == previous {
            state.last_work = i64::try_from(to.saturating_sub(1)).unwrap_or(i64::MAX);
        }
        if state.sleeping && state.last_slept == from.checked_sub(1) {
            state.last_slept = to.checked_sub(1);
        }
        if state.activity == Activity::Panic && state.panic_until == from.saturating_add(99) {
            state.panic_until = to.saturating_add(99);
        }
    }
    if state.brain_clock
        && let Some(remaining) = &mut state.conversion_time
        && *remaining >= 0
    {
        *remaining = remaining
            .saturating_sub(elapsed.min(i32::MAX as u64) as i32)
            .max(0);
    }
}

/// Compare durable gameplay state without allocating or examining path caches.
pub(crate) fn journal_eq(left: &VillagerState, right: &VillagerState) -> bool {
    macro_rules! compare {
        ($($field:ident),+ $(,)?) => { $(left.$field == right.$field)&&+ };
    }
    compare!(
        settlement,
        household,
        birth_origin,
        birth_sequence,
        courtship_started_tick,
        wandering,
        despawn_at,
        data,
        data_finalized,
        xp,
        offers,
        gossips,
        job_site,
        home,
        meeting_point,
        job_site_dimension,
        home_dimension,
        meeting_point_dimension,
        inventory,
        food_level,
        age,
        last_restock,
        last_restock_day,
        restocks_today,
        last_work,
        last_slept,
        golem_detected_until,
        last_gossip,
        activity,
        sleeping,
        courtship_until,
        partner,
        panic_until,
        brain_clock,
        last_gossip_decay,
        conversion_time,
        conversion_player,
        mining_progress,
        lumber_progress,
        soldier,
    )
}

/// Clone only durable fields on a change, never allocate/copy navigation caches.
pub(crate) fn journal_image(state: &VillagerState) -> VillagerState {
    VillagerState {
        settlement: state.settlement,
        household: state.household.clone(),
        birth_origin: state.birth_origin.clone(),
        birth_sequence: state.birth_sequence,
        courtship_started_tick: state.courtship_started_tick,
        wandering: state.wandering,
        despawn_at: state.despawn_at,
        data: state.data,
        data_finalized: state.data_finalized,
        xp: state.xp,
        offers: state.offers.clone(),
        gossips: state.gossips.clone(),
        job_site: state.job_site,
        home: state.home,
        meeting_point: state.meeting_point,
        job_site_dimension: state.job_site_dimension.clone(),
        home_dimension: state.home_dimension.clone(),
        meeting_point_dimension: state.meeting_point_dimension.clone(),
        inventory: state.inventory.clone(),
        food_level: state.food_level,
        age: state.age,
        last_restock: state.last_restock,
        last_restock_day: state.last_restock_day,
        restocks_today: state.restocks_today,
        last_work: state.last_work,
        last_slept: state.last_slept,
        golem_detected_until: state.golem_detected_until,
        last_gossip: state.last_gossip,
        activity: state.activity,
        sleeping: state.sleeping,
        seat: None,
        courtship_until: state.courtship_until,
        partner: state.partner,
        navigation_path: Vec::new(),
        navigation_target: None,
        navigation_chunks: Vec::new(),
        opened_doors: Vec::new(),
        harvest_at: 0,
        harvest_progress: super::harvest::Progress::default(),
        motion_geometry: super::brain::MotionGeometryCache::default(),
        navigation_repath_at: 0,
        navigation_can_open_doors: false,
        navigation_progress_len: 0,
        navigation_progress_next: None,
        navigation_progress_dist_sq: 0.0,
        poi_validation: crate::settlements::allocator::PoiValidation::default(),
        trade_work: None,
        panic_until: state.panic_until,
        brain_clock: state.brain_clock,
        last_gossip_decay: state.last_gossip_decay,
        conversion_time: state.conversion_time,
        conversion_player: state.conversion_player,
        mining_progress: state.mining_progress.clone(),
        lumber_progress: state.lumber_progress.clone(),
        soldier: state.soldier.clone(),
    }
}

fn decode_stack(items: &ItemRegistry, tag: &Tag, name: &'static str) -> Result<ItemStack> {
    let fields = compound(tag, name)?;
    // The canonical stack decoder treats absent/wrong id as empty and defaults
    // malformed counts. Here a present merchant/inventory stack must be real.
    if !matches!(field(fields, "id")?, Some(Tag::String(_))) {
        return Err(VillagerNbtError::Invalid(name));
    }
    if integer(fields, "count", 1)? <= 0 {
        return Err(VillagerNbtError::Invalid(name));
    }
    if let Some(tag) = field(fields, "components")? {
        let components = compound(tag, name)?;
        let mut seen = std::collections::BTreeSet::new();
        for (key, value) in components {
            if !seen.insert(key.as_str()) {
                return Err(VillagerNbtError::Invalid("duplicate item component"));
            }
            let valid = match key.as_str() {
                "minecraft:damage" => matches!(value, Tag::Int(value) if *value >= 0),
                "minecraft:enchantments" => matches!(value, Tag::Compound(_)),
                "minecraft:item_model" => matches!(value, Tag::String(_)),
                "minecraft:custom_name" => {
                    matches!(value, Tag::String(_) | Tag::Compound(_) | Tag::List(_))
                }
                _ => true,
            };
            if !valid {
                return Err(VillagerNbtError::Invalid("item component shape"));
            }
        }
    }
    stack_from_fields(items, fields)?
        .filter(stack_valid)
        .ok_or(VillagerNbtError::Invalid(name))
}
fn offer_to_tag(items: &ItemRegistry, offer: &VillagerTradeOffer) -> Result<Tag> {
    let mut fields = vec![
        (
            "buy".into(),
            Tag::Compound(stack_to_fields(items, &offer.cost_a)?),
        ),
        (
            "sell".into(),
            Tag::Compound(stack_to_fields(items, &offer.result)?),
        ),
        ("uses".into(), Tag::Int(offer.uses)),
        ("maxUses".into(), Tag::Int(offer.max_uses)),
        ("rewardExp".into(), Tag::Byte(i8::from(offer.reward_exp))),
        ("xp".into(), Tag::Int(offer.xp)),
        ("specialPrice".into(), Tag::Int(offer.special_price)),
        ("demand".into(), Tag::Int(offer.demand)),
        ("priceMultiplier".into(), Tag::Float(offer.price_multiplier)),
    ];
    if let Some(cost) = &offer.cost_b {
        fields.push(("buyB".into(), Tag::Compound(stack_to_fields(items, cost)?)));
    }
    Ok(Tag::Compound(fields))
}
fn offer_from_tag(items: &ItemRegistry, tag: &Tag) -> Result<VillagerTradeOffer> {
    let fields = compound(tag, "Offers.Recipes")?;
    let required = |name| field(fields, name)?.ok_or(VillagerNbtError::Invalid(name));
    Ok(VillagerTradeOffer {
        cost_a: decode_stack(items, required("buy")?, "buy")?,
        cost_b: field(fields, "buyB")?
            .map(|tag| decode_stack(items, tag, "buyB"))
            .transpose()?,
        result: decode_stack(items, required("sell")?, "sell")?,
        uses: integer(fields, "uses", 0)?,
        max_uses: integer(fields, "maxUses", 4)?,
        reward_exp: boolean(fields, "rewardExp", true)?,
        xp: integer(fields, "xp", 1)?,
        special_price: integer(fields, "specialPrice", 0)?,
        demand: integer(fields, "demand", 0)?,
        price_multiplier: match field(fields, "priceMultiplier")? {
            None => 0.0,
            Some(Tag::Float(value)) => *value,
            _ => return Err(VillagerNbtError::Invalid("priceMultiplier")),
        },
    })
}
fn memory(value: Tag) -> Tag {
    Tag::Compound(vec![("value".into(), value)])
}
fn poi_memory(pos: BlockPos, dimension: &str) -> Tag {
    memory(Tag::Compound(vec![
        ("dimension".into(), Tag::String(dimension.into())),
        ("pos".into(), position_tag(pos)),
    ]))
}
fn memory_fields<'a>(memories: &'a Fields, name: &'static str) -> Result<Option<&'a Fields>> {
    field(memories, name)?
        .map(|tag| compound(tag, name))
        .transpose()
}
fn poi_from_memory(memories: &Fields, name: &'static str) -> Result<Option<(BlockPos, String)>> {
    let Some(wrapper) = memory_fields(memories, name)? else {
        return Ok(None);
    };
    if field(wrapper, "ttl")?.is_some() {
        return Err(VillagerNbtError::Invalid(name));
    }
    let value = compound(
        field(wrapper, "value")?.ok_or(VillagerNbtError::Invalid(name))?,
        name,
    )?;
    let dimension = string(value, "dimension", "")?;
    if !dimension_valid(dimension) {
        return Err(VillagerNbtError::Invalid(name));
    }
    let pos = position(
        field(value, "pos")?.ok_or(VillagerNbtError::Invalid(name))?,
        name,
    )?;
    Ok(Some((pos, dimension.into())))
}

/// Encode complete species fields for summon/native entity/Anvil checkpoints.
/// Brain expiry is a relative vanilla ttl; the journal keeps the absolute deadline.
pub(crate) fn villager_to_fields(
    items: &ItemRegistry,
    state: &VillagerState,
    tick: u64,
) -> Result<Vec<(String, Tag)>> {
    validate_state(state)?;
    let mut memories = Vec::new();
    for (name, pos, dimension) in [
        (
            "minecraft:job_site",
            state.job_site,
            &state.job_site_dimension,
        ),
        ("minecraft:home", state.home, &state.home_dimension),
        (
            "minecraft:meeting_point",
            state.meeting_point,
            &state.meeting_point_dimension,
        ),
    ] {
        if let Some(pos) = pos {
            memories.push((name.into(), poi_memory(pos, dimension)));
        }
    }
    if let Some(slept) = state.last_slept {
        memories.push((
            "minecraft:last_slept".into(),
            memory(tick_tag(slept, "last_slept")?),
        ));
    }
    if state.last_work != 0 {
        memories.push((
            "minecraft:last_worked_at_poi".into(),
            memory(Tag::Long(state.last_work)),
        ));
    }
    if let Some(until) = state.golem_detected_until {
        if until == u64::MAX {
            memories.push((
                "minecraft:golem_detected_recently".into(),
                memory(Tag::Byte(1)),
            ));
        } else if until >= tick {
            // The brain stores an inclusive deadline (detection at t lasts to
            // t+599); vanilla's ttl counts the current tick as well.
            let ttl = until - tick + 1;
            memories.push((
                "minecraft:golem_detected_recently".into(),
                Tag::Compound(vec![
                    ("value".into(), Tag::Byte(1)),
                    ("ttl".into(), tick_tag(ttl, "golem ttl")?),
                ]),
            ));
        }
    }
    let inventory: Vec<Tag> = state
        .inventory
        .iter()
        .flatten()
        .map(|stack| stack_to_fields(items, stack).map(Tag::Compound))
        .collect::<std::result::Result<_, _>>()?;
    let mut runtime = vec![
        ("last_restock_day".into(), Tag::Long(state.last_restock_day)),
        ("brain_clock".into(), Tag::Byte(i8::from(state.brain_clock))),
        (
            "last_gossip".into(),
            tick_tag(state.last_gossip, "last_gossip")?,
        ),
        (
            "panic_until".into(),
            tick_tag(state.panic_until, "panic_until")?,
        ),
        (
            "activity".into(),
            Tag::String(
                match state.activity {
                    Activity::Idle => "idle",
                    Activity::Work => "work",
                    Activity::Meet => "meet",
                    Activity::Rest => "rest",
                    Activity::Play => "play",
                    Activity::Panic => "panic",
                }
                .into(),
            ),
        ),
        // Vanilla compact Inventory omits holes, but Solaris retains exact custody slots.
        (
            "inventory_slots".into(),
            Tag::IntArray(
                state
                    .inventory
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, item)| item.as_ref().map(|_| slot as i32))
                    .collect(),
            ),
        ),
    ];
    if let Some(id) = state.settlement {
        // NBT has no unsigned long; retain all identity bits, including bit 63.
        runtime.push(("settlement".into(), Tag::Long(id.0 as i64)));
    }
    if state.wandering {
        runtime.push(("wandering_trader".into(), Tag::Byte(1)));
    }
    if let Some(deadline) = state.despawn_at {
        runtime.push(("despawn_at".into(), tick_tag(deadline, "despawn_at")?));
    }
    if let Some(until) = state.golem_detected_until.filter(|until| *until < tick) {
        runtime.push((
            "expired_golem_deadline".into(),
            tick_tag(until, "golem deadline")?,
        ));
    }
    if let Some(until) = state.courtship_until {
        runtime.push((
            "courtship_until".into(),
            tick_tag(until, "courtship_until")?,
        ));
    }
    if let Some(started) = state.courtship_started_tick {
        runtime.push((
            "courtship_started".into(),
            tick_tag(started, "courtship_started")?,
        ));
    }
    if let Some(household) = state.household.as_deref() {
        runtime.push(("household".into(), Tag::String(household.into())));
    }
    if let Some(origin) = state.birth_origin.as_ref() {
        let mut parents = origin.parents.clone();
        parents.sort_unstable();
        runtime.push((
            "birth_parents".into(),
            Tag::List(ListTag {
                element_type: tag_type::STRING,
                elements: parents.into_iter().map(Tag::String).collect(),
            }),
        ));
        runtime.push((
            "birth_started".into(),
            tick_tag(origin.started_tick, "birth_started")?,
        ));
        runtime.push((
            "birth_tick".into(),
            tick_tag(origin.birth_tick, "birth_tick")?,
        ));
    }
    if let Some(sequence) = state.birth_sequence {
        // NBT Long is a signed carrier; preserve all u64 identity bits.
        runtime.push(("birth_sequence".into(), Tag::Long(sequence as i64)));
    }
    if let Some(tag) = progress_to_tag(&state.mining_progress, "SolarisMiningProgress")? {
        runtime.push(("SolarisMiningProgress".into(), tag));
    }
    if let Some(tag) = progress_to_tag(&state.lumber_progress, "SolarisLumberProgress")? {
        runtime.push(("SolarisLumberProgress".into(), tag));
    }
    if let Some(tag) = progress_to_tag(&state.soldier, "SolarisSoldier")? {
        runtime.push(("SolarisSoldier".into(), tag));
    }
    if matches!(
        state.data.profession,
        VillagerProfession::Miner
            | VillagerProfession::Lumberjack
            | VillagerProfession::Builder
            | VillagerProfession::Soldier
    ) {
        runtime.push((
            "profession".into(),
            Tag::String(state.data.profession.name().into()),
        ));
    }
    if let Some(partner) = state.partner {
        runtime.push(("partner".into(), Tag::Int(partner.0)));
    }
    let mut fields = vec![
        (
            "VillagerData".into(),
            Tag::Compound(vec![
                (
                    "type".into(),
                    Tag::String(format!("minecraft:{}", state.data.kind.name())),
                ),
                (
                    "profession".into(),
                    Tag::String(format!("minecraft:{}", state.data.profession.title_name())),
                ),
                ("level".into(), Tag::Int(i32::from(state.data.level))),
            ]),
        ),
        (
            "VillagerDataFinalized".into(),
            Tag::Byte(i8::from(state.data_finalized)),
        ),
        (
            "Offers".into(),
            Tag::Compound(vec![(
                "Recipes".into(),
                compound_list(
                    state
                        .offers
                        .iter()
                        .map(|offer| offer_to_tag(items, offer))
                        .collect::<Result<_>>()?,
                ),
            )]),
        ),
        (
            "Gossips".into(),
            compound_list(
                state
                    .gossips
                    .iter()
                    .map(|gossip| {
                        Tag::Compound(vec![
                            ("Target".into(), uuid_tag(gossip.target)),
                            ("Type".into(), Tag::String(gossip.kind.clone())),
                            ("Value".into(), Tag::Int(gossip.value)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("Xp".into(), Tag::Int(state.xp)),
        ("FoodLevel".into(), Tag::Byte(state.food_level as i8)),
        ("Age".into(), Tag::Int(state.age)),
        ("LastRestock".into(), Tag::Long(state.last_restock)),
        ("RestocksToday".into(), Tag::Int(state.restocks_today)),
        (
            "LastGossipDecay".into(),
            tick_tag(state.last_gossip_decay, "LastGossipDecay")?,
        ),
        ("Inventory".into(), compound_list(inventory)),
        (
            "Brain".into(),
            Tag::Compound(vec![("memories".into(), Tag::Compound(memories))]),
        ),
        ("solaris:VillagerRuntime".into(), Tag::Compound(runtime)),
    ];
    if state.wandering {
        fields.push((
            "DespawnDelay".into(),
            Tag::Int(
                state
                    .despawn_at
                    .map_or(0, |at| at.saturating_sub(tick).min(i32::MAX as u64) as i32),
            ),
        ));
    }
    if state.sleeping {
        let home = state
            .home
            .ok_or(VillagerNbtError::Invalid("sleeping_pos"))?;
        fields.push(("sleeping_pos".into(), position_tag(home)));
    }
    if let Some(time) = state.conversion_time {
        fields.push(("ConversionTime".into(), Tag::Int(time)));
        fields.push(("IsBaby".into(), Tag::Byte(i8::from(state.age < 0))));
    }
    if let Some(player) = state.conversion_player {
        fields.push(("ConversionPlayer".into(), uuid_tag(player)));
    }
    Ok(fields)
}

/// Fields owned by the villager projection, including cleared optional values.
pub(crate) fn owns_field(key: &str) -> bool {
    matches!(
        key,
        "VillagerData"
            | "VillagerDataFinalized"
            | "Offers"
            | "Gossips"
            | "Xp"
            | "FoodLevel"
            | "Age"
            | "LastRestock"
            | "RestocksToday"
            | "LastGossipDecay"
            | "Inventory"
            | "Brain"
            | "solaris:VillagerRuntime"
            | "DespawnDelay"
            | "sleeping_pos"
            | "ConversionTime"
            | "IsBaby"
            | "ConversionPlayer"
    )
}

/// Missing vanilla fields use vanilla defaults; levels use vanilla's minimum of one.
/// Other malformed present data fails closed.
/// No caller-owned state is mutated on error.
pub(crate) fn villager_from_fields(
    items: &ItemRegistry,
    fields: &Fields,
    tick: u64,
) -> Result<VillagerState> {
    let mut state = VillagerState::default();
    if let Some(tag) = field(fields, "VillagerData")? {
        let data = compound(tag, "VillagerData")?;
        let profession_name = string(data, "profession", "minecraft:none")?;
        let profession = VillagerProfession::from_name(profession_name)
            .ok_or(VillagerNbtError::Invalid("VillagerData.profession"))?;
        if matches!(
            profession,
            VillagerProfession::Miner
                | VillagerProfession::Lumberjack
                | VillagerProfession::Builder
                | VillagerProfession::Soldier
        ) {
            return Err(VillagerNbtError::Invalid("VillagerData.profession"));
        }
        state.data = VillagerData {
            kind: VillagerKind::from_name(string(data, "type", "minecraft:plains")?)
                .ok_or(VillagerNbtError::Invalid("VillagerData.type"))?,
            profession,
            level: u8::try_from(integer(data, "level", 1)?.max(1))
                .map_err(|_| VillagerNbtError::Invalid("VillagerData.level"))?,
        };
    }
    state.data_finalized = boolean(fields, "VillagerDataFinalized", false)?;
    state.xp = integer(fields, "Xp", 0)?;
    state.food_level = integer(fields, "FoodLevel", 0)?;
    state.age = integer(fields, "Age", 0)?;
    let baby = boolean(fields, "IsBaby", false)?;
    if field(fields, "Age")?.is_none() && baby {
        state.age = -24_000;
    }
    state.last_restock = long(fields, "LastRestock", 0)?;
    state.restocks_today = integer(fields, "RestocksToday", 0)?;
    state.last_gossip_decay = tick_value(long(fields, "LastGossipDecay", 0)?, "LastGossipDecay")?;
    if let Some(tag) = field(fields, "Offers")? {
        state.offers = list(compound(tag, "Offers")?, "Recipes")?
            .iter()
            .map(|tag| offer_from_tag(items, tag))
            .collect::<Result<_>>()?;
    }
    state.gossips = list(fields, "Gossips")?
        .iter()
        .map(|tag| {
            let gossip = compound(tag, "Gossips")?;
            Ok(Gossip {
                target: uuid_from_tag(
                    field(gossip, "Target")?.ok_or(VillagerNbtError::Invalid("Target"))?,
                    "Target",
                )?,
                kind: string(gossip, "Type", "")?.into(),
                value: integer(gossip, "Value", 0)?,
            })
        })
        .collect::<Result<_>>()?;
    let stacks = list(fields, "Inventory")?;
    if stacks.len() > 8 {
        return Err(VillagerNbtError::Invalid("Inventory"));
    }
    let runtime = field(fields, "solaris:VillagerRuntime")?
        .map(|tag| compound(tag, "solaris:VillagerRuntime"))
        .transpose()?
        .unwrap_or(&[]);
    if let Some(tag) = field(runtime, "profession")? {
        state.data.profession = match tag {
            Tag::String(role) if role == "miner" => VillagerProfession::Miner,
            Tag::String(role) if role == "lumberjack" => VillagerProfession::Lumberjack,
            Tag::String(role) if role == "builder" => VillagerProfession::Builder,
            Tag::String(role) if role == "soldier" => VillagerProfession::Soldier,
            _ => {
                return Err(VillagerNbtError::Invalid(
                    "solaris:VillagerRuntime.profession",
                ));
            }
        };
    }
    state.wandering = boolean(runtime, "wandering_trader", false)?;
    state.despawn_at = match field(runtime, "despawn_at")? {
        Some(Tag::Long(at)) => Some(tick_value(*at, "despawn_at")?),
        Some(_) => return Err(VillagerNbtError::Invalid("despawn_at")),
        None => {
            let delay = integer(fields, "DespawnDelay", 0)?;
            (delay > 0).then_some(tick.saturating_add(delay as u64))
        }
    };
    let slots = match field(runtime, "inventory_slots")? {
        None => None,
        Some(Tag::IntArray(slots)) if slots.len() == stacks.len() => Some(slots),
        _ => return Err(VillagerNbtError::Invalid("inventory_slots")),
    };
    for (index, tag) in stacks.iter().enumerate() {
        let slot = match slots {
            Some(slots) => usize::try_from(slots[index])
                .map_err(|_| VillagerNbtError::Invalid("inventory_slots"))?,
            None => index,
        };
        if slot >= 8 || state.inventory[slot].is_some() {
            return Err(VillagerNbtError::Invalid("inventory_slots"));
        }
        state.inventory[slot] = Some(decode_stack(items, tag, "Inventory")?);
    }
    if let Some(tag) = field(fields, "Brain")? {
        let brain = compound(tag, "Brain")?;
        let memories = field(brain, "memories")?
            .map(|tag| compound(tag, "Brain.memories"))
            .transpose()?
            .unwrap_or(&[]);
        if let Some((pos, dimension)) = poi_from_memory(memories, "minecraft:job_site")? {
            state.job_site = Some(pos);
            state.job_site_dimension = dimension;
        }
        if let Some((pos, dimension)) = poi_from_memory(memories, "minecraft:home")? {
            state.home = Some(pos);
            state.home_dimension = dimension;
        }
        if let Some((pos, dimension)) = poi_from_memory(memories, "minecraft:meeting_point")? {
            state.meeting_point = Some(pos);
            state.meeting_point_dimension = dimension;
        }
        if let Some(wrapper) = memory_fields(memories, "minecraft:last_slept")? {
            if field(wrapper, "value")?.is_none() || field(wrapper, "ttl")?.is_some() {
                return Err(VillagerNbtError::Invalid("last_slept"));
            }
            state.last_slept = Some(tick_value(long(wrapper, "value", 0)?, "last_slept")?);
        }
        if let Some(wrapper) = memory_fields(memories, "minecraft:last_worked_at_poi")? {
            if field(wrapper, "value")?.is_none() || field(wrapper, "ttl")?.is_some() {
                return Err(VillagerNbtError::Invalid("last_worked_at_poi"));
            }
            state.last_work = long(wrapper, "value", 0)?;
        }
        if let Some(wrapper) = memory_fields(memories, "minecraft:golem_detected_recently")? {
            if !boolean(wrapper, "value", false)? {
                return Err(VillagerNbtError::Invalid("golem_detected_recently"));
            }
            state.golem_detected_until = if field(wrapper, "ttl")?.is_none() {
                Some(u64::MAX)
            } else {
                let ttl = tick_value(long(wrapper, "ttl", 0)?, "golem ttl")?;
                if ttl == 0 {
                    None
                } else {
                    Some(
                        tick.checked_add(ttl - 1)
                            .ok_or(VillagerNbtError::Invalid("golem ttl"))?,
                    )
                }
            };
        }
    }
    if let Some(tag) = field(fields, "sleeping_pos")? {
        let pos = position(tag, "sleeping_pos")?;
        if state.home.is_some_and(|home| home != pos) {
            return Err(VillagerNbtError::Invalid("sleeping_pos"));
        }
        state.home = Some(pos);
        state.sleeping = true;
    }
    state.last_restock_day = long(runtime, "last_restock_day", i64::MIN)?;
    if field(runtime, "settlement")?.is_some() {
        state.settlement = Some(crate::settlements::SettlementId(
            long(runtime, "settlement", 0)? as u64,
        ));
    }
    state.last_gossip = tick_value(long(runtime, "last_gossip", 0)?, "last_gossip")?;
    state.panic_until = tick_value(long(runtime, "panic_until", 0)?, "panic_until")?;
    state.brain_clock = boolean(runtime, "brain_clock", false)?;
    if field(runtime, "expired_golem_deadline")?.is_some() {
        let until = tick_value(
            long(runtime, "expired_golem_deadline", 0)?,
            "golem deadline",
        )?;
        if until >= tick || state.golem_detected_until.is_some() {
            return Err(VillagerNbtError::Invalid("golem deadline"));
        }
        state.golem_detected_until = Some(until);
    }
    state.activity = match string(
        runtime,
        "activity",
        if state.sleeping { "rest" } else { "idle" },
    )? {
        "idle" => Activity::Idle,
        "work" => Activity::Work,
        "meet" => Activity::Meet,
        "rest" => Activity::Rest,
        "play" => Activity::Play,
        "panic" => Activity::Panic,
        _ => return Err(VillagerNbtError::Invalid("activity")),
    };
    if field(runtime, "courtship_until")?.is_some() {
        state.courtship_until = Some(tick_value(
            long(runtime, "courtship_until", 0)?,
            "courtship_until",
        )?);
    }
    if field(runtime, "courtship_started")?.is_some() {
        state.courtship_started_tick = Some(tick_value(
            long(runtime, "courtship_started", 0)?,
            "courtship_started",
        )?);
    }
    if field(runtime, "household")?.is_some() {
        let household = string(runtime, "household", "")?;
        if household.is_empty() || household.len() > 64 {
            return Err(VillagerNbtError::Invalid("household"));
        }
        state.household = Some(household.into());
    }
    if field(runtime, "birth_parents")?.is_some()
        || field(runtime, "birth_started")?.is_some()
        || field(runtime, "birth_tick")?.is_some()
    {
        let Some(Tag::List(parents)) = field(runtime, "birth_parents")? else {
            return Err(VillagerNbtError::Invalid("birth_parents"));
        };
        if parents.element_type != tag_type::STRING || parents.elements.len() != 2 {
            return Err(VillagerNbtError::Invalid("birth_parents"));
        }
        let mut sorted = Vec::with_capacity(2);
        for parent in &parents.elements {
            let Tag::String(parent) = parent else {
                return Err(VillagerNbtError::Invalid("birth_parents"));
            };
            sorted.push(parent.clone());
        }
        sorted.sort_unstable();
        let origin = mc_script::ScriptNativeBirthOrigin::new(
            [sorted[0].clone(), sorted[1].clone()],
            tick_value(long(runtime, "birth_started", 0)?, "birth_started")?,
            tick_value(long(runtime, "birth_tick", 0)?, "birth_tick")?,
        );
        origin
            .validate()
            .map_err(|_| VillagerNbtError::Invalid("birth_origin"))?;
        state.birth_origin = Some(origin);
    }
    if field(runtime, "birth_sequence")?.is_some() {
        state.birth_sequence = Some(long(runtime, "birth_sequence", 0)? as u64);
    }
    if field(runtime, "partner")?.is_some() {
        state.partner = Some(EntityId(integer(runtime, "partner", 0)?));
    }
    state.mining_progress = progress_from_tag(runtime, "SolarisMiningProgress")?;
    state.lumber_progress = progress_from_tag(runtime, "SolarisLumberProgress")?;
    state.soldier = progress_from_tag(runtime, "SolarisSoldier")?;
    if field(fields, "ConversionTime")?.is_some() {
        state.conversion_time = Some(integer(fields, "ConversionTime", -1)?);
    }
    if let Some(tag) = field(fields, "ConversionPlayer")? {
        state.conversion_player = Some(uuid_from_tag(tag, "ConversionPlayer")?);
    }
    validate_state(&state)?;
    Ok(state)
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
