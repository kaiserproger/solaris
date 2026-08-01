use super::*;
use crate::lock_policy::lock_authoritative_mutex;
use crate::play::beds::next_morning_time;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
#[path = "sleep_tests.rs"]
mod tests;

pub(in crate::play::session) const DEFAULT_PLAYERS_SLEEPING_PERCENTAGE: u32 = 100;
pub(in crate::play::session) const DEEP_SLEEP_TICKS: u64 = 100;

static NEXT_WAKE_TOKEN: AtomicU64 = AtomicU64::new(1);

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

#[derive(Debug)]
pub(in crate::play::session) struct SleepingState {
    started_tick: u64,
    bed: mc_world::BlockPos,
    wake: Option<StagedWake>,
    deferred_dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) struct SleepingPlayer {
    pub(in crate::play) id: SessionId,
    pub(in crate::play) bed: mc_world::BlockPos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play::session) enum SleepWakeReason {
    Normal,
    Damage,
    Spectator { previous: GameMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StagedWake {
    token: u64,
    reason: SleepWakeReason,
    claimed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) struct SleepWakeToken {
    session_id: SessionId,
    bed: mc_world::BlockPos,
    token: u64,
}

#[derive(Debug)]
pub(in crate::play) struct CompletedSleepWake {
    pub(in crate::play) dispatches: Vec<VisibilityDispatch>,
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

fn stage_sleepers(inner: &mut SessionRegistryInner) -> Vec<SleepingPlayer> {
    let mut sleepers = inner
        .sleeping_sessions
        .iter_mut()
        .filter_map(|(&id, state)| {
            if state.wake.is_some() {
                return None;
            }
            state.wake = Some(StagedWake {
                token: NEXT_WAKE_TOKEN.fetch_add(1, Ordering::Relaxed),
                reason: SleepWakeReason::Normal,
                claimed: false,
            });
            Some(SleepingPlayer { id, bed: state.bed })
        })
        .collect::<Vec<_>>();
    sleepers.sort_unstable_by_key(|sleeper| sleeper.id);
    sleepers
}

pub(in crate::play::session) fn stage_sleep_wake_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    reason: SleepWakeReason,
) -> Option<SleepingPlayer> {
    let state = inner.sleeping_sessions.get_mut(&id)?;
    match state.wake.as_mut() {
        Some(wake) => wake.reason = reason,
        None => {
            state.wake = Some(StagedWake {
                token: NEXT_WAKE_TOKEN.fetch_add(1, Ordering::Relaxed),
                reason,
                claimed: false,
            });
        }
    }
    Some(SleepingPlayer { id, bed: state.bed })
}

pub(in crate::play::session) fn defer_staged_sleep_dispatches_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    dispatches: &mut Vec<VisibilityDispatch>,
) {
    let Some(state) = inner.sleeping_sessions.get_mut(&id) else {
        return;
    };
    if state
        .wake
        .is_some_and(|wake| matches!(wake.reason, SleepWakeReason::Damage))
    {
        state.deferred_dispatches.append(dispatches);
    }
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
        #[cfg(test)]
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
        #[cfg(not(test))]
        let sleep_transition = {
            let mut inner = self.lock_inner("set world time");
            self.world_time.store(world_time, Ordering::Release);
            self.resolve_sleep_transition_locked(&mut inner)
        };
        #[cfg(test)]
        let mut outcome = self.commit_claimed_pending_hostiles(pending);
        #[cfg(not(test))]
        let mut outcome = HerdSpawnOutcome::committed(Vec::new());
        let sleep_published_time =
            matches!(sleep_transition, Some(SleepTransition::Skipped { .. }));
        outcome
            .dispatches
            .extend(self.sleep_transition_dispatches(sleep_transition));
        if !sleep_published_time {
            outcome
                .dispatches
                .extend(self.broadcast_world_time(self.world_time()));
        }
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

    pub(crate) fn daylight_cycle_enabled(&self) -> bool {
        self.daylight_cycle_enabled.load(Ordering::Acquire)
    }

    pub(crate) fn daylight_cycle_rate(&self) -> f32 {
        if self.daylight_cycle_enabled() {
            1.0
        } else {
            0.0
        }
    }

    pub(crate) fn set_daylight_cycle_enabled(&self, enabled: bool) {
        let previous = self.daylight_cycle_enabled.swap(enabled, Ordering::AcqRel);
        if previous != enabled {
            dispatch_visibility_commands(self.broadcast_world_time(self.world_time()));
        }
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
                wake: None,
                deferred_dispatches: Vec::new(),
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

    pub(in crate::play) fn request_sleep_wake(&self, id: SessionId) -> Option<mc_world::BlockPos> {
        let mut inner = self.lock_inner("request player wake");
        let state = inner.sleeping_sessions.get_mut(&id)?;
        if state.wake.is_none() {
            state.wake = Some(StagedWake {
                token: NEXT_WAKE_TOKEN.fetch_add(1, Ordering::Relaxed),
                reason: SleepWakeReason::Normal,
                claimed: false,
            });
        }
        Some(state.bed)
    }

    pub(in crate::play) fn cancel_sleep_reservation(&self, id: SessionId) {
        let mut inner = self.lock_inner("cancel uncommitted sleep reservation");
        if inner
            .sleeping_sessions
            .get(&id)
            .is_some_and(|state| state.wake.is_none())
        {
            inner.sleeping_sessions.remove(&id);
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn stop_sleeping(&self, id: SessionId) -> Option<mc_world::BlockPos> {
        self.lock_inner("stop player sleep in test")
            .sleeping_sessions
            .remove(&id)
            .map(|state| state.bed)
    }

    pub(in crate::play::session) fn stage_sleep_wake_locked(
        &self,
        inner: &mut SessionRegistryInner,
        id: SessionId,
        reason: SleepWakeReason,
    ) -> Option<SleepingPlayer> {
        stage_sleep_wake_locked(inner, id, reason)
    }

    pub(in crate::play) fn claim_sleep_wake(
        &self,
        id: SessionId,
        bed: mc_world::BlockPos,
    ) -> Option<SleepWakeToken> {
        let mut inner = self.lock_inner("claim staged player wake");
        let state = inner.sleeping_sessions.get_mut(&id)?;
        if state.bed != bed {
            return None;
        }
        let wake = state.wake.as_mut()?;
        if wake.claimed {
            return None;
        }
        wake.claimed = true;
        Some(SleepWakeToken {
            session_id: id,
            bed,
            token: wake.token,
        })
    }

    pub(in crate::play::session) fn defer_staged_sleep_dispatches(
        &self,
        id: SessionId,
        dispatches: &mut Vec<VisibilityDispatch>,
    ) {
        let mut inner = self.lock_inner("defer sleep damage publications");
        defer_staged_sleep_dispatches_locked(&mut inner, id, dispatches);
    }

    pub(in crate::play) fn reject_sleep_wake(&self, token: SleepWakeToken) -> Option<GameMode> {
        let mut inner = self.lock_inner("reject staged player wake");
        let player_state = inner.player_persistence.get(&token.session_id).cloned();
        let state = inner.sleeping_sessions.get_mut(&token.session_id)?;
        if state.bed != token.bed {
            return None;
        }
        let wake = state.wake?;
        if wake.token != token.token || !wake.claimed {
            return None;
        }
        match wake.reason {
            SleepWakeReason::Spectator { previous } => {
                let Some(player_state) = player_state else {
                    state.wake.as_mut().expect("validated staged wake").claimed = false;
                    return None;
                };
                let mut player_state =
                    lock_authoritative_mutex(&player_state, "play.player_persistence");
                state.wake = None;
                if player_state.game_mode == GameMode::Spectator {
                    player_state.game_mode = previous;
                    Some(previous)
                } else {
                    None
                }
            }
            SleepWakeReason::Normal | SleepWakeReason::Damage => {
                state.wake.as_mut().expect("validated staged wake").claimed = false;
                None
            }
        }
    }

    pub(in crate::play) fn complete_sleep_wake(
        &self,
        token: SleepWakeToken,
    ) -> Option<CompletedSleepWake> {
        let (transition, mut deferred_dispatches) = {
            let mut inner = self.lock_inner("complete staged player wake");
            let state = inner.sleeping_sessions.get(&token.session_id)?;
            let wake = state.wake?;
            if state.bed != token.bed || wake.token != token.token || !wake.claimed {
                return None;
            }
            let reason = wake.reason;
            let mut state = inner
                .sleeping_sessions
                .remove(&token.session_id)
                .expect("validated sleeping state remains present");
            let spectator_commit_is_current = matches!(reason, SleepWakeReason::Spectator { .. })
                && inner
                    .player_persistence
                    .get(&token.session_id)
                    .is_some_and(|player_state| {
                        lock_authoritative_mutex(player_state, "play.player_persistence").game_mode
                            == GameMode::Spectator
                    });
            if spectator_commit_is_current {
                inner.spectator_sessions.insert(token.session_id);
                inner.publish_combat_target(token.session_id);
            }
            let transition = self.resolve_sleep_transition_locked(&mut inner);
            (transition, std::mem::take(&mut state.deferred_dispatches))
        };

        let mut dispatches = self.broadcast_player_entity_data_including_self(
            token.session_id,
            vec![EntityDataValue::Pose {
                index: ENTITY_DATA_POSE_INDEX,
                pose: EntityPose::Standing,
            }],
        );
        dispatches.append(&mut deferred_dispatches);
        dispatches.extend(self.sleep_transition_dispatches(transition));
        Some(CompletedSleepWake { dispatches })
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
            .values()
            .filter(|state| state.wake.is_none())
            .count()
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
        if inner.sleeping_sessions.is_empty() {
            return None;
        }

        let world_time = self.world_time();
        if !super::super::world_time_is_night(world_time) {
            return Some(SleepTransition::Woke {
                sleepers: stage_sleepers(inner),
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
            .filter(|state| {
                state.wake.is_none()
                    && simulation_tick.saturating_sub(state.started_tick) >= DEEP_SLEEP_TICKS
            })
            .count();
        if deep_sleepers < required {
            return None;
        }

        let sleepers = stage_sleepers(inner);
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
        #[cfg(test)]
        let (world_time, pending) = {
            let mut inner = self.lock_inner("advance world time and claim pending hostiles");
            let previous_world_time = self.world_time();
            let world_time = self.advance_world_time_core(ticks);
            let pending = if self.daylight_cycle_enabled()
                && super::super::world_time_advance_crosses_night_start(previous_world_time, ticks)
            {
                claim_loaded_pending_hostiles_locked(&mut inner)
            } else {
                ClaimedPendingHostiles::default()
            };
            (world_time, pending)
        };
        #[cfg(test)]
        {
            (world_time, self.commit_claimed_pending_hostiles(pending))
        }
        #[cfg(not(test))]
        {
            (
                self.advance_world_time_core(ticks),
                HerdSpawnOutcome::committed(Vec::new()),
            )
        }
    }

    #[cfg(test)]
    pub(crate) fn advance_world_time(&self, ticks: u64) -> u64 {
        self.advance_world_time_core(ticks)
    }

    pub(in crate::play::session) fn advance_world_time_core(&self, ticks: u64) -> u64 {
        let world_time = if self.daylight_cycle_enabled() {
            self.world_time
                .fetch_add(ticks, Ordering::AcqRel)
                .wrapping_add(ticks)
        } else {
            self.world_time()
        };
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
        let daylight_cycle_enabled = self.daylight_cycle_enabled();
        let rate = if daylight_cycle_enabled { 1.0 } else { 0.0 };
        let recipients = {
            let mut inner = self.lock_inner("broadcast world time");
            let recipients = inner
                .sessions
                .iter_mut()
                .filter_map(|(&id, session)| {
                    if session.last_broadcast_world_time
                        == Some((world_time, daylight_cycle_enabled))
                    {
                        return None;
                    }
                    session.last_broadcast_world_time = Some((world_time, daylight_cycle_enabled));
                    Some(id)
                })
                .collect::<Vec<_>>();
            session_recipients(&inner, recipients)
        };
        visibility_dispatches(recipients, || OutboundCommand::WorldTime {
            world_time,
            rate,
        })
    }
}
