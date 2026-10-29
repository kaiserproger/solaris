//! Target selector matching and resolution for command execution.
//! Moved verbatim from `command_execution.rs`.

use super::*;

/// Java target-selector option families beyond type/name/position: scores,
/// tag, team, level, gamemode, pitch/yaw, nbt, advancements, predicates.
pub(super) fn options_match(
    state: &TickState,
    executor: EntityId,
    slot: crate::entities::EntitySlot,
    selector: &Selector,
) -> bool {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let holder = state
        .connections
        .name_of(body.id)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("entity_{}", body.id.0));
    for (objective, (min, max)) in &selector.scores {
        let value = state
            .scoreboard
            .scores
            .get(&holder)
            .and_then(|scores| scores.get(objective))
            .map(|score| score.value);
        let inside = match value {
            Some(value) => min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max),
            None => min.is_none() && max.is_none(),
        };
        if !inside {
            return false;
        }
    }
    let entity_data = if !selector.tags.is_empty() || !selector.nbt.is_empty() {
        match crate::command_data::entity(state, body.id) {
            Ok(tag) => Some(tag),
            Err(_) => return false,
        }
    } else {
        None
    };
    if let Some(Tag::Compound(fields)) = &entity_data {
        let tags = fields
            .iter()
            .find_map(|(name, value)| match (name.as_str(), value) {
                ("Tags", Tag::List(list)) => Some(&list.elements),
                _ => None,
            });
        for (tag, inverted) in &selector.tags {
            let matched = if tag.is_empty() {
                tags.is_none_or(Vec::is_empty)
            } else {
                tags.is_some_and(|tags| {
                    tags.iter()
                        .any(|value| matches!(value, Tag::String(value) if value == tag))
                })
            };
            if matched == *inverted {
                return false;
            }
        }
    }
    if let Some((team, inverted)) = &selector.team {
        let id_label = format!("entity_{}", body.id.0);
        let member = match team.as_str() {
            "" => state.scoreboard.teams.values().all(|entry| {
                !entry.members.contains(&holder) && !entry.members.contains(&id_label)
            }),
            name => state.scoreboard.teams.get(name).is_some_and(|entry| {
                entry.members.contains(&holder) || entry.members.contains(&id_label)
            }),
        };
        if member == *inverted {
            return false;
        }
    }
    if let Some((min, max)) = selector.level {
        let level = state
            .players
            .get(&body.id)
            .map(|player| player.vitals.experience.level);
        match level {
            Some(level) => {
                if min.is_some_and(|min| level < min) || max.is_some_and(|max| level > max) {
                    return false;
                }
            }
            None => return false,
        }
    }
    if let Some((mode, inverted)) = &selector.gamemode {
        let matched = state
            .players
            .get(&body.id)
            .is_some_and(|player| &player.game_mode == mode);
        if matched == *inverted {
            return false;
        }
    }
    if selector.pitch.is_some() || selector.yaw.is_some() {
        let [yaw, pitch, _] = state.entities.rot(slot);
        if let Some((min, max)) = selector.pitch {
            let pitch = f64::from(pitch);
            let inside = match (min, max) {
                (Some(min), Some(max)) if min <= max => {
                    pitch >= f64::from(min) && pitch <= f64::from(max)
                }
                _ => {
                    let lo = min.map(f64::from);
                    let hi = max.map(f64::from);
                    lo.is_none_or(|lo| pitch >= lo) && hi.is_none_or(|hi| pitch <= hi)
                }
            };
            if !inside {
                return false;
            }
        }
        if let Some((min, max)) = selector.yaw {
            let yaw = f64::from(yaw).rem_euclid(360.0);
            let inside = match (min, max) {
                (Some(min), Some(max)) if f64::from(min) <= f64::from(max) => {
                    yaw >= f64::from(min) && yaw <= f64::from(max)
                }
                (Some(min), Some(max)) => yaw >= f64::from(min) || yaw <= f64::from(max),
                (Some(min), None) => yaw >= f64::from(min),
                (None, Some(max)) => yaw <= f64::from(max),
                (None, None) => true,
            };
            if !inside {
                return false;
            }
        }
    }
    if let Some(actual) = &entity_data {
        for (expected, inverted) in &selector.nbt {
            if crate::command_nbt_path::matches(expected, actual) == *inverted {
                return false;
            }
        }
    }
    for (id, granted) in &selector.advancements {
        let done = state
            .players
            .get(&body.id)
            .and_then(|player| player.advancement_progress.as_object())
            .and_then(|progress| progress.get(id))
            .and_then(|entry| entry.get("done"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if done != *granted {
            return false;
        }
    }
    for (id, inverted) in &selector.predicates {
        // No loot-predicate registry is wired to selectors yet; a positive
        // predicate matches nothing while a negative one admits everything.
        if !inverted {
            return false;
        }
        let _ = (id, executor);
    }
    let _ = row;
    true
}

pub(super) fn selector(
    state: &TickState,
    executor: EntityId,
    selector: &Selector,
) -> Vec<EntityId> {
    let origin = crate::command_console::pose(state, executor)
        .map(|(position, _)| position)
        .unwrap_or_else(|| state.world.world_spawn_position());
    let origin: [f64; 3] =
        std::array::from_fn(|axis| selector.position[axis].unwrap_or(origin[axis]));
    let players_only = matches!(
        selector.kind,
        SelectorKind::Nearest | SelectorKind::AllPlayers | SelectorKind::Random
    );
    let candidates: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            // Java @a includes dead players and @s may select a nonliving
            // executor; @e and @n stay alive-only.
            let dead_player = !body.alive && matches!(body.kind, EntityKind::Player { .. });
            let admitted = body.alive
                || (dead_player
                    && matches!(
                        selector.kind,
                        SelectorKind::AllPlayers | SelectorKind::Executor
                    ));
            if !admitted
                || (players_only && !matches!(body.kind, EntityKind::Player { .. }))
                || (selector.kind == SelectorKind::Executor && body.id != executor)
            {
                return None;
            }
            for (kind, is_tag, inverted) in &selector.entity_types {
                let matched = if *is_tag {
                    entity_type_tags(state, body.id)
                        .iter()
                        .any(|tag| *tag == kind.as_str())
                } else {
                    entity_type_name(state, body.id) == Some(kind.as_str())
                };
                if matched == *inverted {
                    return None;
                }
            }
            if !selector.names.is_empty() {
                let actual = state
                    .connections
                    .name_of(body.id)
                    .or(row.appearance.custom_name.as_deref())
                    .map(str::to_owned)
                    .unwrap_or_else(|| plain_entity_name(state, body.id));
                for (name, inverted) in &selector.names {
                    if (actual == *name) == *inverted {
                        return None;
                    }
                }
            }
            if !options_match(state, executor, slot, selector) {
                return None;
            }
            let pos = state.entities.pos(slot);
            if selector.extent.iter().any(Option::is_some)
                && (0..3).any(|axis| {
                    let extent = selector.extent[axis].unwrap_or(0.0);
                    let min = pos[axis] - if axis == 1 { 0.0 } else { body.half_width };
                    let max = pos[axis]
                        + if axis == 1 {
                            body.height
                        } else {
                            body.half_width
                        };
                    max <= origin[axis] + extent.min(0.0)
                        || min >= origin[axis] + extent.max(0.0) + 1.0
                })
            {
                return None;
            }
            let distance = (pos[0] - origin[0]).powi(2)
                + (pos[1] - origin[1]).powi(2)
                + (pos[2] - origin[2]).powi(2);
            if let Some((min, max)) = selector.distance
                && (min.is_some_and(|min| distance < min * min)
                    || max.is_some_and(|max| distance > max * max))
            {
                return None;
            }
            Some((body.id, distance))
        })
        .collect();
    order_and_limit(state, executor, selector, candidates)
}

/// The single owner of selector `sort=`/`limit=` semantics, shared by
/// per-world and cross-dimension resolution. `candidates` carry the squared
/// distance from the selector origin.
pub(crate) fn order_and_limit(
    state: &TickState,
    executor: EntityId,
    selector: &crate::commands::Selector,
    mut candidates: Vec<(EntityId, f64)>,
) -> Vec<EntityId> {
    let sort = selector.sort.unwrap_or(match selector.kind {
        SelectorKind::Nearest => SelectorSort::Nearest,
        SelectorKind::Random => SelectorSort::Random,
        _ => SelectorSort::Arbitrary,
    });
    match sort {
        SelectorSort::Nearest => candidates.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0))),
        SelectorSort::Furthest => {
            candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)))
        }
        // Java shuffles on every evaluation (`ORDER_RANDOM` calls
        // `Collections.shuffle` per selection), so repeated same-tick
        // evaluations draw again instead of reusing one deterministic
        // permutation. A per-call SplitMix64 stream keyed by a shared
        // counter gives fresh bounded randomness without vanilla bit-exact
        // RNG claims.
        SelectorSort::Random => {
            let count = state.command_context.random_draws.get();
            state
                .command_context
                .random_draws
                .set(count.wrapping_add(candidates.len() as u64));
            let mut stream = count
                .wrapping_add(state.tick)
                .wrapping_add((executor.0 as u32 as u64) << 32);
            let mut keys = Vec::with_capacity(candidates.len());
            for _ in 0..candidates.len() {
                stream = stream.wrapping_add(0x9e3779b97f4a7c15);
                let mut value = stream;
                value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
                keys.push(value ^ (value >> 31));
            }
            let mut keyed: Vec<_> = candidates.into_iter().zip(keys).collect();
            keyed.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.0.cmp(&b.0.0)));
            candidates = keyed.into_iter().map(|(entry, _)| entry).collect();
        }
        SelectorSort::Arbitrary => candidates.sort_by_key(|(id, _)| *id),
    }
    let default_limit = if matches!(
        selector.kind,
        SelectorKind::Nearest | SelectorKind::Random | SelectorKind::Executor
    ) {
        1
    } else {
        usize::MAX
    };
    candidates
        .into_iter()
        .take(selector.limit.unwrap_or(default_limit))
        .map(|(id, _)| id)
        .collect()
}

pub(crate) fn resolve_target(
    state: &TickState,
    executor: EntityId,
    target: &Target,
) -> Vec<EntityId> {
    match target {
        Target::Entity(entity) => state
            .entities
            .slot_of(*entity)
            .map(|_| vec![*entity])
            .unwrap_or_default(),
        Target::Executor => state
            .entities
            .slot_of_any(executor)
            .map(|_| vec![executor])
            .unwrap_or_default(),
        Target::Nearest | Target::AllPlayers => selector(
            state,
            executor,
            &Selector {
                kind: if matches!(target, Target::Nearest) {
                    SelectorKind::Nearest
                } else {
                    SelectorKind::AllPlayers
                },
                entity_types: Vec::new(),
                names: Vec::new(),
                scores: Vec::new(),
                tags: Vec::new(),
                team: None,
                level: None,
                gamemode: None,
                pitch: None,
                yaw: None,
                nbt: Vec::new(),
                advancements: Vec::new(),
                predicates: Vec::new(),
                distance: None,
                position: [None; 3],
                extent: [None; 3],
                limit: None,
                sort: None,
            },
        ),
        Target::Selector(value) => selector(state, executor, value),
        Target::Player(name) => {
            let uuid = uuid::Uuid::parse_str(name).ok();
            state
                .entities
                .slots()
                .filter_map(|slot| {
                    let body = state.entities.body(slot);
                    let row = state.entities.row(slot);
                    let is_player = matches!(body.kind, EntityKind::Player { .. });
                    let actual = state
                        .connections
                        .name_of(body.id)
                        .or(row.appearance.custom_name.as_deref())
                        .unwrap_or_else(|| entity_type_name(state, body.id).unwrap_or(""));
                    ((body.alive || is_player) && (actual == name || uuid == Some(row.uuid)))
                        .then_some(body.id)
                })
                .collect()
        }
    }
}

/// Whether a selector's declared shape admits at most one entity: an
/// explicit limit, or a single-type variable without options that widen it.
/// Used before resolution so `/tp @s @a` rejects on declared maximum rather
/// than on the resolved count.
pub(super) fn single_target(target: &Target) -> bool {
    match target {
        Target::Entity(_) | Target::Executor | Target::Nearest => true,
        Target::Selector(selector) => {
            if selector.limit.is_some_and(|limit| limit <= 1) {
                return true;
            }
            matches!(
                selector.kind,
                SelectorKind::Nearest | SelectorKind::Executor | SelectorKind::Random
            ) && selector.limit.is_none()
        }
        _ => false,
    }
}

/// Players addressed by a player-only command. Java rejects a non-player
/// selector shape (`@e`, entity tags) at admission — before state mutation —
/// instead of filtering to the player subset afterwards. `/give @e stone`
/// errors rather than granting to online players.
pub(crate) fn player_targets(
    state: &TickState,
    executor: EntityId,
    target: &Target,
) -> Vec<EntityId> {
    if let Target::Selector(selector) = target
        && !matches!(
            selector.kind,
            SelectorKind::Nearest
                | SelectorKind::AllPlayers
                | SelectorKind::Random
                | SelectorKind::Executor
        )
    {
        return Vec::new();
    }
    let mut targets = resolve_target(state, executor, target);
    targets.retain(|id| state.players.contains_key(id));
    targets
}
