use std::sync::atomic::Ordering;

use super::{OutboundCommand, SessionRegistry, session_recipients, visibility_dispatches};

const WEATHER_LEVEL_STEP_PER_TICK: f32 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum WeatherKind {
    Clear = 0,
    Rain = 1,
    Thunder = 2,
}

impl WeatherKind {
    pub(crate) const fn raining(self) -> bool {
        !matches!(self, Self::Clear)
    }

    pub(crate) const fn thundering(self) -> bool {
        matches!(self, Self::Thunder)
    }

    pub(crate) const fn as_u8(self) -> u8 {
        self as u8
    }

    pub(crate) const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Clear),
            1 => Some(Self::Rain),
            2 => Some(Self::Thunder),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WeatherState {
    kind: WeatherKind,
    rain_level_bits: u32,
    thunder_level_bits: u32,
}

impl WeatherState {
    #[cfg(test)]
    pub(crate) const CLEAR: Self = Self {
        kind: WeatherKind::Clear,
        rain_level_bits: 0.0_f32.to_bits(),
        thunder_level_bits: 0.0_f32.to_bits(),
    };

    pub(crate) fn new(kind: WeatherKind, rain_level: f32, thunder_level: f32) -> Option<Self> {
        if !valid_level(rain_level) || !valid_level(thunder_level) {
            return None;
        }
        Some(Self {
            kind,
            rain_level_bits: rain_level.to_bits(),
            thunder_level_bits: thunder_level.to_bits(),
        })
    }

    pub(crate) const fn kind(self) -> WeatherKind {
        self.kind
    }

    pub(crate) const fn rain_level(self) -> f32 {
        f32::from_bits(self.rain_level_bits)
    }

    pub(crate) const fn thunder_level(self) -> f32 {
        f32::from_bits(self.thunder_level_bits)
    }
}

const WEATHER_RAINING_SYNC_THRESHOLD: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct WeatherStateSync {
    pub(in crate::play) raining: bool,
    pub(in crate::play) rain_level: f32,
    pub(in crate::play) thunder_level: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct WeatherProjection {
    pub(in crate::play) rain_level: Option<f32>,
    pub(in crate::play) thunder_level: Option<f32>,
    pub(in crate::play) state_sync: Option<WeatherStateSync>,
}

impl WeatherProjection {
    fn between(previous: WeatherState, next: WeatherState) -> Option<Self> {
        let rain_level =
            (previous.rain_level_bits != next.rain_level_bits).then(|| next.rain_level());
        let thunder_level =
            (previous.thunder_level_bits != next.thunder_level_bits).then(|| next.thunder_level());
        let crossed_start = next.kind().raining()
            && previous.rain_level() < WEATHER_RAINING_SYNC_THRESHOLD
            && next.rain_level() >= WEATHER_RAINING_SYNC_THRESHOLD;
        let crossed_stop = !next.kind().raining()
            && previous.rain_level() >= WEATHER_RAINING_SYNC_THRESHOLD
            && next.rain_level() < WEATHER_RAINING_SYNC_THRESHOLD;
        let state_sync = (crossed_start || crossed_stop).then_some(WeatherStateSync {
            raining: crossed_start,
            rain_level: next.rain_level(),
            thunder_level: next.thunder_level(),
        });
        (rain_level.is_some() || thunder_level.is_some() || state_sync.is_some()).then_some(Self {
            rain_level,
            thunder_level,
            state_sync,
        })
    }

    pub(in crate::play) fn snapshot(state: WeatherState) -> Option<Self> {
        (state.rain_level() > 0.0 || state.thunder_level() > 0.0 || state.kind().raining())
            .then_some(Self {
                rain_level: None,
                thunder_level: None,
                state_sync: Some(WeatherStateSync {
                    raining: state.kind().raining(),
                    rain_level: state.rain_level(),
                    thunder_level: state.thunder_level(),
                }),
            })
    }
}

impl SessionRegistry {
    pub(crate) fn weather(&self) -> WeatherState {
        let kind = WeatherKind::from_u8(self.weather_kind.load(Ordering::Acquire))
            .expect("session weather atomic only stores validated kinds");
        let rain_level = f32::from_bits(self.rain_level_bits.load(Ordering::Acquire));
        let thunder_level = f32::from_bits(self.thunder_level_bits.load(Ordering::Acquire));
        WeatherState::new(kind, rain_level, thunder_level)
            .expect("session weather atomics only store validated levels")
    }

    pub(crate) fn restore_weather(&self, weather: WeatherState) {
        self.weather_kind
            .store(weather.kind().as_u8(), Ordering::Release);
        self.rain_level_bits
            .store(weather.rain_level().to_bits(), Ordering::Release);
        self.thunder_level_bits
            .store(weather.thunder_level().to_bits(), Ordering::Release);
    }

    pub(crate) fn set_weather(&self, kind: WeatherKind) {
        self.weather_kind.store(kind.as_u8(), Ordering::Release);
    }

    pub(crate) fn tick_weather(&self, ticks: u64) {
        if ticks == 0 {
            return;
        }
        let previous = self.weather();
        let mut rain_level = previous.rain_level();
        let mut thunder_level = previous.thunder_level();
        for _ in 0..ticks.min(101) {
            rain_level = step_level(
                rain_level,
                previous.kind().raining(),
                WEATHER_LEVEL_STEP_PER_TICK,
            );
            thunder_level = step_level(
                thunder_level,
                previous.kind().thundering(),
                WEATHER_LEVEL_STEP_PER_TICK,
            );
        }
        let next = WeatherState::new(previous.kind(), rain_level, thunder_level)
            .expect("clamped weather levels are valid");
        self.restore_weather(next);
        if let Some(projection) = WeatherProjection::between(previous, next) {
            super::super::dispatch_visibility_commands(self.broadcast_weather(projection));
        }
    }

    pub(in crate::play) fn broadcast_weather(
        &self,
        projection: WeatherProjection,
    ) -> Vec<super::VisibilityDispatch> {
        let recipients = {
            let inner = self.lock_inner("broadcast weather");
            let ids = inner.sessions.keys().copied().collect::<Vec<_>>();
            session_recipients(&inner, ids)
        };
        visibility_dispatches(recipients, || OutboundCommand::Weather(projection))
    }
}

fn valid_level(level: f32) -> bool {
    level.is_finite() && (0.0..=1.0).contains(&level)
}

fn step_level(level: f32, increasing: bool, step: f32) -> f32 {
    if increasing {
        (level + step).min(1.0)
    } else {
        (level - step).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tokio::sync::mpsc;

    use super::*;
    use crate::login::LoggedInProfile;
    use crate::play::PlayerPose;

    #[test]
    fn weather_state_levels_are_validated_and_canonical() {
        assert_eq!(WeatherState::CLEAR.kind(), WeatherKind::Clear);
        assert_eq!(WeatherState::CLEAR.rain_level(), 0.0);
        assert_eq!(WeatherState::CLEAR.thunder_level(), 0.0);
        assert!(WeatherState::new(WeatherKind::Rain, -0.01, 0.0).is_none());
        assert!(WeatherState::new(WeatherKind::Thunder, 1.0, 1.01).is_none());
        for kind in [WeatherKind::Clear, WeatherKind::Rain, WeatherKind::Thunder] {
            assert_eq!(WeatherKind::from_u8(kind.as_u8()), Some(kind));
        }
        assert_eq!(WeatherKind::from_u8(3), None);
    }

    #[test]
    fn weather_levels_ramp_by_one_hundredth_per_tick() {
        let registry = SessionRegistry::new();
        registry.set_weather(WeatherKind::Rain);
        registry.tick_weather(1);
        assert!((registry.weather().rain_level() - 0.01).abs() < f32::EPSILON);
        assert_eq!(registry.weather().thunder_level(), 0.0);
        registry.tick_weather(99);
        assert!(registry.weather().rain_level() >= 0.99);

        registry.set_weather(WeatherKind::Thunder);
        registry.tick_weather(1);
        assert!((registry.weather().thunder_level() - 0.01).abs() < f32::EPSILON);
        registry.tick_weather(99);
        assert!(registry.weather().thunder_level() >= 0.99);

        registry.set_weather(WeatherKind::Clear);
        registry.tick_weather(101);
        assert_eq!(registry.weather(), WeatherState::CLEAR);
    }

    #[test]
    fn weather_tick_reliably_broadcasts_changed_levels_to_every_active_session() {
        let registry = SessionRegistry::new();
        let register = |name: &str| {
            let profile = LoggedInProfile {
                uuid: crate::login::offline_uuid(name),
                name: name.to_owned(),
            };
            let (tx, rx) = mpsc::channel(8);
            registry.register(
                &profile,
                (0, 0),
                2,
                HashSet::new(),
                tx,
                PlayerPose::new(0.5, 64.0, 0.5),
            );
            rx
        };
        let mut first = register("WeatherA");
        let mut second = register("WeatherB");

        registry.set_weather(WeatherKind::Rain);
        assert!(first.try_recv().is_err());
        assert!(second.try_recv().is_err());
        registry.tick_weather(1);
        for receiver in [&mut first, &mut second] {
            assert!(matches!(
                receiver.try_recv(),
                Ok(OutboundCommand::Weather(WeatherProjection {
                    rain_level: Some(level),
                    thunder_level: None,
                    state_sync: None,
                })) if (level - 0.01).abs() < f32::EPSILON
            ));
            assert!(receiver.try_recv().is_err());
        }
    }
}
