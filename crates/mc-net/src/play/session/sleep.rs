use super::*;
use crate::play::beds::next_morning_time;
use std::sync::atomic::Ordering;

pub(in crate::play::session) const DEFAULT_PLAYERS_SLEEPING_PERCENTAGE: u32 = 100;
pub(in crate::play::session) const DEEP_SLEEP_TICKS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) enum SleepOutcome {
    Daytime,
    Occupied,
    Waiting {
        sleeping: usize,
        required: usize,
    },
    Skipped {
        new_time: u64,
        sleepers: Vec<SleepingPlayer>,
    },
    Inactive,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::play::session) enum SleepTransition {
    Woke {
        sleepers: Vec<SleepingPlayer>,
    },
    Skipped {
        new_time: u64,
        sleepers: Vec<SleepingPlayer>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play::session) struct SleepingState {
    started_tick: u64,
    bed: mc_world::BlockPos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) struct SleepingPlayer {
    pub(in crate::play) id: SessionId,
    pub(in crate::play) bed: mc_world::BlockPos,
}

fn active_sleeping_players(inner: &SessionRegistryInner) -> usize {
    inner
        .sessions
        .keys()
        .filter(|id| !inner.spectator_sessions.contains(id))
        .count()
}

pub(in crate::play::session) fn sleepers_needed(active_players: usize, percentage: u32) -> usize {
    let numerator = (active_players as u128).saturating_mul(u128::from(percentage));
    let required = numerator.saturating_add(99) / 100;
    usize::try_from(required).unwrap_or(usize::MAX).max(1)
}

fn drain_sleepers(inner: &mut SessionRegistryInner) -> Vec<SleepingPlayer> {
    let mut sleepers = inner
        .sleeping_sessions
        .drain()
        .map(|(id, state)| SleepingPlayer { id, bed: state.bed })
        .collect::<Vec<_>>();
    sleepers.sort_unstable_by_key(|sleeper| sleeper.id);
    sleepers
}

impl SessionRegistry {
    #[cfg(test)]
    pub(crate) fn set_world_time(&self, world_time: u64) {
        self.restore_world_time_core(world_time);
    }

    pub(in crate::play) fn restore_world_time_owned(
        &self,
        _authority: &SimulationAuthority,
        world_time: u64,
    ) {
        self.restore_world_time_core(world_time);
    }

    pub(in crate::play::session) fn restore_world_time_core(&self, world_time: u64) {
        self.world_time.store(world_time, Ordering::Release);
    }

    pub(in crate::play) fn set_world_time_owned(
        &self,
        _authority: &SimulationAuthority,
        world_time: u64,
    ) -> HerdSpawnOutcome {
        self.set_world_time_core(world_time)
    }

    pub(in crate::play::session) fn set_world_time_core(
        &self,
        world_time: u64,
    ) -> HerdSpawnOutcome {
        let (pending, sleep_transition) = {
            let mut inner = self.lock_inner("set world time and claim pending hostiles");
            self.world_time.store(world_time, Ordering::Release);
            let pending = if super::super::world_time_is_night(world_time) {
                claim_loaded_pending_hostiles_locked(&mut inner)
            } else {
                ClaimedPendingHostiles::default()
            };
            let sleep_transition = self.resolve_sleep_transition_locked(&mut inner);
            (pending, sleep_transition)
        };

        let mut outcome = self.commit_claimed_pending_hostiles(pending);
        outcome
            .dispatches
            .extend(self.sleep_transition_dispatches(sleep_transition));
        outcome
            .dispatches
            .extend(self.broadcast_world_time(self.world_time()));
        outcome
    }

    #[cfg(test)]
    pub(crate) fn set_world_time_and_update_sleep(&self, world_time: u64) {
        dispatch_visibility_commands(self.set_world_time_core(world_time).into_dispatches());
    }

    pub(crate) fn players_sleeping_percentage(&self) -> u32 {
        self.players_sleeping_percentage.load(Ordering::Acquire)
    }

    pub(crate) fn set_players_sleeping_percentage(&self, percentage: u32) {
        self.players_sleeping_percentage
            .store(percentage, Ordering::Release);
        dispatch_visibility_commands(
            self.recompute_sleep_transition("update sleep after gamerule change"),
        );
    }

    pub(in crate::play) fn begin_sleep_at(
        &self,
        id: SessionId,
        bed: mc_world::BlockPos,
    ) -> SleepOutcome {
        let mut inner = self.lock_inner("begin player sleep");
        if !inner.sessions.contains_key(&id) || inner.spectator_sessions.contains(&id) {
            return SleepOutcome::Inactive;
        }
        if inner
            .sleeping_sessions
            .iter()
            .any(|(sleeper, state)| *sleeper != id && state.bed == bed)
        {
            return SleepOutcome::Occupied;
        }
        let world_time = self.world_time();
        if !super::super::world_time_is_night(world_time) {
            inner.sleeping_sessions.remove(&id);
            return SleepOutcome::Daytime;
        }

        inner
            .sleeping_sessions
            .entry(id)
            .or_insert_with(|| SleepingState {
                started_tick: self.simulation_tick(),
                bed,
            });
        let sleeping = inner.sleeping_sessions.len();
        let required = sleepers_needed(
            active_sleeping_players(&inner),
            self.players_sleeping_percentage(),
        );
        match self.resolve_sleep_transition_locked(&mut inner) {
            Some(SleepTransition::Skipped { new_time, sleepers }) => {
                SleepOutcome::Skipped { new_time, sleepers }
            }
            Some(SleepTransition::Woke { .. }) => SleepOutcome::Daytime,
            None => SleepOutcome::Waiting { sleeping, required },
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn begin_sleep(&self, id: SessionId) -> SleepOutcome {
        self.begin_sleep_at(
            id,
            mc_world::BlockPos {
                x: i32::try_from(id).unwrap_or(i32::MAX),
                y: 64,
                z: 0,
            },
        )
    }

    pub(in crate::play) fn tick_sleep_owned(
        &self,
        _authority: &SimulationAuthority,
    ) -> Vec<VisibilityDispatch> {
        self.recompute_sleep_transition("tick player sleep")
    }

    pub(in crate::play) fn stop_sleeping(&self, id: SessionId) -> Option<mc_world::BlockPos> {
        self.lock_inner("stop player sleep")
            .sleeping_sessions
            .remove(&id)
            .map(|state| state.bed)
    }

    pub(in crate::play) fn sleeping_bed(&self, id: SessionId) -> Option<mc_world::BlockPos> {
        self.lock_inner("read player sleeping bed")
            .sleeping_sessions
            .get(&id)
            .map(|state| state.bed)
    }

    #[cfg(test)]
    pub(in crate::play::session) fn sleeping_session_count_for_test(&self) -> usize {
        self.lock_inner("read sleeping session count")
            .sleeping_sessions
            .len()
    }

    fn recompute_sleep_transition(&self, operation: &'static str) -> Vec<VisibilityDispatch> {
        let transition = {
            let mut inner = self.lock_inner(operation);
            self.resolve_sleep_transition_locked(&mut inner)
        };
        self.sleep_transition_dispatches(transition)
    }

    pub(in crate::play::session) fn resolve_sleep_transition_locked(
        &self,
        inner: &mut SessionRegistryInner,
    ) -> Option<SleepTransition> {
        inner.sleeping_sessions.retain(|id, _| {
            inner.sessions.contains_key(id) && !inner.spectator_sessions.contains(id)
        });
        if inner.sleeping_sessions.is_empty() {
            return None;
        }

        let world_time = self.world_time();
        if !super::super::world_time_is_night(world_time) {
            return Some(SleepTransition::Woke {
                sleepers: drain_sleepers(inner),
            });
        }

        let required = sleepers_needed(
            active_sleeping_players(inner),
            self.players_sleeping_percentage(),
        );
        let simulation_tick = self.simulation_tick();
        let deep_sleepers = inner
            .sleeping_sessions
            .values()
            .filter(|state| simulation_tick.saturating_sub(state.started_tick) >= DEEP_SLEEP_TICKS)
            .count();
        if deep_sleepers < required {
            return None;
        }

        let sleepers = drain_sleepers(inner);
        let new_time = next_morning_time(world_time);
        self.world_time.store(new_time, Ordering::Release);
        Some(SleepTransition::Skipped { new_time, sleepers })
    }

    pub(in crate::play::session) fn sleep_transition_dispatches(
        &self,
        transition: Option<SleepTransition>,
    ) -> Vec<VisibilityDispatch> {
        let Some(transition) = transition else {
            return Vec::new();
        };
        let (sleepers, new_time) = match transition {
            SleepTransition::Woke { sleepers } => (sleepers, None),
            SleepTransition::Skipped { new_time, sleepers } => (sleepers, Some(new_time)),
        };
        self.completed_sleep_dispatches(sleepers, new_time)
    }

    pub(in crate::play) fn completed_sleep_dispatches(
        &self,
        sleepers: Vec<SleepingPlayer>,
        new_time: Option<u64>,
    ) -> Vec<VisibilityDispatch> {
        let mut dispatches = Vec::new();
        for sleeper in sleepers {
            let wake_recipient = {
                let inner = self.lock_inner("route completed sleep wake");
                session_recipients(&inner, [sleeper.id])
            };
            dispatches.extend(visibility_dispatches(wake_recipient, || {
                OutboundCommand::WakeFromBed { bed: sleeper.bed }
            }));
            dispatches.extend(self.broadcast_player_entity_data_including_self(
                sleeper.id,
                vec![EntityDataValue::Pose {
                    index: ENTITY_DATA_POSE_INDEX,
                    pose: EntityPose::Standing,
                }],
            ));
        }
        if let Some(new_time) = new_time {
            dispatches.extend(self.broadcast_world_time(new_time));
        }
        dispatches
    }

    pub(in crate::play) fn advance_world_time_owned(
        &self,
        _authority: &SimulationAuthority,
        ticks: u64,
    ) -> (u64, HerdSpawnOutcome) {
        let (world_time, pending) = {
            let mut inner = self.lock_inner("advance world time and claim pending hostiles");
            let previous_world_time = self.world_time();
            let world_time = self.advance_world_time_core(ticks);
            let pending =
                if super::super::world_time_advance_crosses_night_start(previous_world_time, ticks)
                {
                    claim_loaded_pending_hostiles_locked(&mut inner)
                } else {
                    ClaimedPendingHostiles::default()
                };
            (world_time, pending)
        };
        (world_time, self.commit_claimed_pending_hostiles(pending))
    }

    #[cfg(test)]
    pub(crate) fn advance_world_time(&self, ticks: u64) -> u64 {
        self.advance_world_time_core(ticks)
    }

    pub(in crate::play::session) fn advance_world_time_core(&self, ticks: u64) -> u64 {
        let world_time = self
            .world_time
            .fetch_add(ticks, Ordering::AcqRel)
            .wrapping_add(ticks);
        let previous_tick = self
            .entity_lifecycle_tick
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |tick| {
                Some(tick.saturating_add(ticks))
            })
            .expect("simulation tick update always returns a value");
        let simulation_tick = previous_tick.saturating_add(ticks);
        self.simulation_tick_sender.send_replace(simulation_tick);
        world_time
    }

    pub(crate) fn world_time(&self) -> u64 {
        self.world_time.load(Ordering::Acquire)
    }

    pub(in crate::play) fn broadcast_world_time(&self, world_time: u64) -> Vec<VisibilityDispatch> {
        let recipients = {
            let inner = self.lock_inner("broadcast world time");
            session_recipients(&inner, inner.sessions.keys().copied().collect::<Vec<_>>())
        };
        visibility_dispatches(recipients, || OutboundCommand::WorldTime { world_time })
    }
}
