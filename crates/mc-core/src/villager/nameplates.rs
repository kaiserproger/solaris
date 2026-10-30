//! Outgoing-only nameplates. Native CustomName remains player-owned save data.
use super::{Activity, VillagerProfession};
use crate::boundary::EntityAppearance;
use crate::entities::ColdRow;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Named,
    Generated {
        uuid: uuid::Uuid,
        profession: VillagerProfession,
        baby: bool,
        activity: Activity,
        sleeping: bool,
    },
}

impl Key {
    fn of(row: &ColdRow) -> Self {
        if row.appearance.custom_name.is_some() {
            return Self::Named;
        }
        let v = row.villager.as_ref().expect("villager nameplate");
        Self::Generated {
            uuid: row.uuid,
            profession: v.data.profession,
            baby: v.age < 0,
            activity: v.activity,
            sleeping: v.sleeping,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Nameplate {
    // Retaining the source Arc makes pointer equality immune to allocator reuse.
    seat: Option<crate::boundary::BlockPos>,
    source: Arc<EntityAppearance>,
    key: Key,
    pub(crate) appearance: Arc<EntityAppearance>,
}

impl Nameplate {
    pub(crate) fn new(row: &ColdRow) -> Self {
        let key = Key::of(row);
        let seat = row.villager.as_ref().and_then(|v| v.seat);
        let appearance = if key == Key::Named && row.appearance.custom_name_visible && seat.is_none() {
            Arc::clone(&row.appearance)
        } else {
            let mut appearance = (*row.appearance).clone();
            appearance.villager_seat = seat;
            if let Key::Generated {
                uuid,
                profession,
                baby,
                activity,
                sleeping,
            } = key
            {
                appearance.custom_name = Some(format!(
                    "{} · {} · {}",
                    crate::settlements::presentation::villager_name(uuid),
                    if baby {
                        "ребёнок"
                    } else {
                        crate::settlements::presentation::profession_name(profession)
                    },
                    activity_name(activity, sleeping),
                ));
            }
            appearance.custom_name_visible = true;
            Arc::new(appearance)
        };
        Self {
            source: Arc::clone(&row.appearance),
            key,
            seat,
            appearance,
        }
    }

    pub(crate) fn refresh(&mut self, row: &ColdRow) {
        if self.key != Key::of(row)
            || self.seat != row.villager.as_ref().and_then(|v| v.seat)
            || !Arc::ptr_eq(&self.source, &row.appearance)
        {
            *self = Self::new(row);
        }
    }
}

fn activity_name(activity: Activity, sleeping: bool) -> &'static str {
    if sleeping {
        return "сон";
    }
    match activity {
        Activity::Work => "работа",
        Activity::Rest => "отдых",
        Activity::Meet => "встреча",
        Activity::Idle => "прогулка",
        Activity::Play => "игра",
        Activity::Panic => "паника",
    }
}
