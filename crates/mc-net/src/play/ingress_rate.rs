use std::time::{Duration, Instant};

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ServerboundAttack, ServerboundChat, ServerboundChatCommand, ServerboundCommandSuggestion,
    ServerboundContainerButtonClick, ServerboundContainerClick, ServerboundCustomPayload,
    ServerboundInteract, ServerboundMovePlayerPos, ServerboundMovePlayerPosRot,
    ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundPlayerCommand, ServerboundPlayerInput,
    ServerboundSignUpdate, ServerboundUseItem, ServerboundUseItemOn,
};

use crate::error::ConnectionError;

const TOKEN_SCALE: u128 = 1_000_000_000;
const VIOLATION_RESET_AFTER: Duration = Duration::from_secs(10);
const DISCONNECT_AFTER_VIOLATIONS: u32 = 3;

const PACKET_BURST: u64 = 240;
const PACKET_REFILL_PER_SECOND: u64 = 120;
const BYTE_BURST: u64 = 1024 * 1024;
const BYTE_REFILL_PER_SECOND: u64 = 512 * 1024;
const WORK_BURST: u64 = 1024;
const WORK_REFILL_PER_SECOND: u64 = 512;
const CHAT_BURST: u64 = 16;
const CHAT_REFILL_PER_SECOND: u64 = 4;
const COMMAND_BURST: u64 = 8;
const COMMAND_REFILL_PER_SECOND: u64 = 2;
const SUGGESTION_BURST: u64 = 8;
const SUGGESTION_REFILL_PER_SECOND: u64 = 2;
const CUSTOM_PAYLOAD_BURST_BYTES: u64 = 128 * 1024;
const CUSTOM_PAYLOAD_REFILL_BYTES_PER_SECOND: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IngressDecision {
    Allow,
    Drop {
        class: &'static str,
        violations: u32,
        class_violations: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitClass {
    Packets,
    Bytes,
    Work,
    Chat,
    Command,
    Suggestion,
    CustomPayload,
}

impl LimitClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Packets => "packets",
            Self::Bytes => "bytes",
            Self::Work => "weighted_work",
            Self::Chat => "chat",
            Self::Command => "command",
            Self::Suggestion => "suggestion",
            Self::CustomPayload => "custom_payload",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ViolationCounters {
    total: u64,
    packets: u64,
    bytes: u64,
    work: u64,
    chat: u64,
    command: u64,
    suggestion: u64,
    custom_payload: u64,
}

impl ViolationCounters {
    fn record(&mut self, class: LimitClass) -> u64 {
        self.total = self.total.saturating_add(1);
        let counter = match class {
            LimitClass::Packets => &mut self.packets,
            LimitClass::Bytes => &mut self.bytes,
            LimitClass::Work => &mut self.work,
            LimitClass::Chat => &mut self.chat,
            LimitClass::Command => &mut self.command,
            LimitClass::Suggestion => &mut self.suggestion,
            LimitClass::CustomPayload => &mut self.custom_payload,
        };
        *counter = counter.saturating_add(1);
        *counter
    }
}

#[derive(Debug, Clone)]
struct TokenBucket {
    capacity: u128,
    tokens: u128,
    refill_per_second: u128,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: u64, refill_per_second: u64, now: Instant) -> Self {
        let capacity = u128::from(capacity).saturating_mul(TOKEN_SCALE);
        Self {
            capacity,
            tokens: capacity,
            refill_per_second: u128::from(refill_per_second),
            last_refill: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_refill);
        self.last_refill = now;
        let refill = elapsed.as_nanos().saturating_mul(self.refill_per_second);
        self.tokens = self.capacity.min(self.tokens.saturating_add(refill));
    }

    fn can_take(&self, amount: u64) -> bool {
        self.tokens >= u128::from(amount).saturating_mul(TOKEN_SCALE)
    }

    fn take(&mut self, amount: u64) {
        self.tokens = self
            .tokens
            .saturating_sub(u128::from(amount).saturating_mul(TOKEN_SCALE));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketKind {
    Movement,
    Chat,
    Command,
    Suggestion,
    CustomPayload,
    Action,
    Other,
}

impl PacketKind {
    fn for_id(id: i32) -> Self {
        if matches!(
            id,
            ServerboundMovePlayerPos::ID
                | ServerboundMovePlayerPosRot::ID
                | ServerboundMovePlayerRot::ID
                | ServerboundMovePlayerStatusOnly::ID
                | ServerboundPlayerInput::ID
        ) {
            Self::Movement
        } else if id == ServerboundChat::ID {
            Self::Chat
        } else if id == ServerboundChatCommand::ID {
            Self::Command
        } else if id == ServerboundCommandSuggestion::ID {
            Self::Suggestion
        } else if id == ServerboundCustomPayload::ID {
            Self::CustomPayload
        } else if matches!(
            id,
            ServerboundAttack::ID
                | ServerboundInteract::ID
                | ServerboundUseItem::ID
                | ServerboundUseItemOn::ID
                | ServerboundPlayerAction::ID
                | ServerboundPlayerCommand::ID
                | ServerboundContainerClick::ID
                | ServerboundContainerButtonClick::ID
                | ServerboundPlaceRecipe::ID
                | ServerboundSignUpdate::ID
        ) {
            Self::Action
        } else {
            Self::Other
        }
    }

    fn work_cost(self, body_bytes: usize) -> u64 {
        match self {
            Self::Movement => 1,
            Self::Chat => 8,
            Self::Command => 24,
            Self::Suggestion => 32,
            Self::CustomPayload => {
                let kib = body_bytes.div_ceil(1024);
                8_u64.saturating_add(u64::try_from(kib).unwrap_or(u64::MAX).saturating_mul(4))
            }
            Self::Action => 4,
            Self::Other => 2,
        }
    }
}

pub(super) struct PlayIngressLimiter {
    packets: TokenBucket,
    bytes: TokenBucket,
    work: TokenBucket,
    chat: TokenBucket,
    commands: TokenBucket,
    suggestions: TokenBucket,
    custom_payload_bytes: TokenBucket,
    violation_streak: u32,
    last_violation: Option<Instant>,
    counters: ViolationCounters,
}

impl PlayIngressLimiter {
    #[must_use]
    pub(super) fn new(now: Instant) -> Self {
        Self {
            packets: TokenBucket::new(PACKET_BURST, PACKET_REFILL_PER_SECOND, now),
            bytes: TokenBucket::new(BYTE_BURST, BYTE_REFILL_PER_SECOND, now),
            work: TokenBucket::new(WORK_BURST, WORK_REFILL_PER_SECOND, now),
            chat: TokenBucket::new(CHAT_BURST, CHAT_REFILL_PER_SECOND, now),
            commands: TokenBucket::new(COMMAND_BURST, COMMAND_REFILL_PER_SECOND, now),
            suggestions: TokenBucket::new(SUGGESTION_BURST, SUGGESTION_REFILL_PER_SECOND, now),
            custom_payload_bytes: TokenBucket::new(
                CUSTOM_PAYLOAD_BURST_BYTES,
                CUSTOM_PAYLOAD_REFILL_BYTES_PER_SECOND,
                now,
            ),
            violation_streak: 0,
            last_violation: None,
            counters: ViolationCounters::default(),
        }
    }

    pub(super) fn admit(
        &mut self,
        id: i32,
        body_bytes: usize,
        now: Instant,
    ) -> Result<IngressDecision, ConnectionError> {
        self.refill(now);
        let kind = PacketKind::for_id(id);
        let body_bytes = u64::try_from(body_bytes).unwrap_or(u64::MAX);
        let work_cost = kind.work_cost(usize::try_from(body_bytes).unwrap_or(usize::MAX));

        let rejected = if !self.packets.can_take(1) {
            Some(LimitClass::Packets)
        } else if !self.bytes.can_take(body_bytes) {
            Some(LimitClass::Bytes)
        } else if !self.work.can_take(work_cost) {
            Some(LimitClass::Work)
        } else {
            match kind {
                PacketKind::Chat if !self.chat.can_take(1) => Some(LimitClass::Chat),
                PacketKind::Command if !self.commands.can_take(1) => Some(LimitClass::Command),
                PacketKind::Suggestion if !self.suggestions.can_take(1) => {
                    Some(LimitClass::Suggestion)
                }
                PacketKind::CustomPayload if !self.custom_payload_bytes.can_take(body_bytes) => {
                    Some(LimitClass::CustomPayload)
                }
                _ => None,
            }
        };

        if let Some(class) = rejected {
            return self.reject(class, now);
        }

        self.packets.take(1);
        self.bytes.take(body_bytes);
        self.work.take(work_cost);
        match kind {
            PacketKind::Chat => self.chat.take(1),
            PacketKind::Command => self.commands.take(1),
            PacketKind::Suggestion => self.suggestions.take(1),
            PacketKind::CustomPayload => self.custom_payload_bytes.take(body_bytes),
            PacketKind::Movement | PacketKind::Action | PacketKind::Other => {}
        }
        Ok(IngressDecision::Allow)
    }

    fn refill(&mut self, now: Instant) {
        self.packets.refill(now);
        self.bytes.refill(now);
        self.work.refill(now);
        self.chat.refill(now);
        self.commands.refill(now);
        self.suggestions.refill(now);
        self.custom_payload_bytes.refill(now);
        if self
            .last_violation
            .is_some_and(|last| now.saturating_duration_since(last) >= VIOLATION_RESET_AFTER)
        {
            self.violation_streak = 0;
            self.last_violation = None;
        }
    }

    fn reject(
        &mut self,
        class: LimitClass,
        now: Instant,
    ) -> Result<IngressDecision, ConnectionError> {
        self.last_violation = Some(now);
        self.violation_streak = self.violation_streak.saturating_add(1);
        let class_violations = self.counters.record(class);
        if self.violation_streak >= DISCONNECT_AFTER_VIOLATIONS {
            return Err(ConnectionError::PlayRateLimitExceeded {
                class: class.as_str(),
                violations: self.violation_streak,
            });
        }
        Ok(IngressDecision::Drop {
            class: class.as_str(),
            violations: self.violation_streak,
            class_violations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_protocol::packets::play::{ConfirmTeleportation, ServerboundKeepAlive};

    #[test]
    fn normal_movement_and_burst_under_allowance_are_unaffected() {
        let start = Instant::now();
        let mut limiter = PlayIngressLimiter::new(start);
        for index in 0..200 {
            let now = start + Duration::from_millis(index * 5);
            assert_eq!(
                limiter
                    .admit(ServerboundMovePlayerPos::ID, 25, now)
                    .unwrap(),
                IngressDecision::Allow
            );
        }
    }

    #[test]
    fn sustained_packet_flood_escalates_to_disconnect() {
        let now = Instant::now();
        let mut limiter = PlayIngressLimiter::new(now);
        for _ in 0..PACKET_BURST {
            assert_eq!(
                limiter.admit(ConfirmTeleportation::ID, 1, now).unwrap(),
                IngressDecision::Allow
            );
        }
        for expected in 1..DISCONNECT_AFTER_VIOLATIONS {
            assert!(matches!(
                limiter.admit(ConfirmTeleportation::ID, 1, now).unwrap(),
                IngressDecision::Drop {
                    class: "packets",
                    violations,
                    ..
                } if violations == expected
            ));
        }
        assert!(matches!(
            limiter.admit(ConfirmTeleportation::ID, 1, now),
            Err(ConnectionError::PlayRateLimitExceeded {
                class: "packets",
                violations: DISCONNECT_AFTER_VIOLATIONS,
            })
        ));
    }

    #[test]
    fn expensive_suggestions_hit_their_dedicated_bucket() {
        let now = Instant::now();
        let mut limiter = PlayIngressLimiter::new(now);
        for _ in 0..SUGGESTION_BURST {
            assert_eq!(
                limiter
                    .admit(ServerboundCommandSuggestion::ID, 32, now)
                    .unwrap(),
                IngressDecision::Allow
            );
        }
        assert!(matches!(
            limiter
                .admit(ServerboundCommandSuggestion::ID, 32, now)
                .unwrap(),
            IngressDecision::Drop {
                class: "suggestion",
                class_violations: 1,
                ..
            }
        ));
    }

    #[test]
    fn refill_and_violation_reset_use_injected_time() {
        let start = Instant::now();
        let mut limiter = PlayIngressLimiter::new(start);
        for _ in 0..CHAT_BURST {
            limiter.admit(ServerboundChat::ID, 32, start).unwrap();
        }
        assert!(matches!(
            limiter.admit(ServerboundChat::ID, 32, start).unwrap(),
            IngressDecision::Drop { violations: 1, .. }
        ));

        let refilled = start + Duration::from_secs(1);
        for _ in 0..CHAT_REFILL_PER_SECOND {
            assert_eq!(
                limiter.admit(ServerboundChat::ID, 32, refilled).unwrap(),
                IngressDecision::Allow
            );
        }

        let reset = start + VIOLATION_RESET_AFTER + Duration::from_secs(1);
        for _ in 0..CHAT_BURST {
            let _ = limiter.admit(ServerboundChat::ID, 32, reset);
        }
        assert!(matches!(
            limiter.admit(ServerboundChat::ID, 32, reset).unwrap(),
            IngressDecision::Drop { violations: 1, .. }
        ));
    }

    #[test]
    fn custom_payload_bytes_have_a_separate_budget() {
        let now = Instant::now();
        let mut limiter = PlayIngressLimiter::new(now);
        let half = CUSTOM_PAYLOAD_BURST_BYTES / 2;
        assert_eq!(
            limiter
                .admit(ServerboundCustomPayload::ID, half as usize, now)
                .unwrap(),
            IngressDecision::Allow
        );
        assert_eq!(
            limiter
                .admit(ServerboundCustomPayload::ID, half as usize, now)
                .unwrap(),
            IngressDecision::Allow
        );
        assert!(matches!(
            limiter.admit(ServerboundCustomPayload::ID, 1, now).unwrap(),
            IngressDecision::Drop {
                class: "custom_payload",
                ..
            }
        ));
    }

    #[test]
    fn keepalive_remains_cheap() {
        let now = Instant::now();
        let mut limiter = PlayIngressLimiter::new(now);
        assert_eq!(
            limiter.admit(ServerboundKeepAlive::ID, 8, now).unwrap(),
            IngressDecision::Allow
        );
    }
}
