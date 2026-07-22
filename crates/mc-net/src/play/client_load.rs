const CLIENT_LOAD_TIMEOUT_TICKS: u8 = 60;

#[derive(Debug, Default)]
pub(super) struct ClientLoadGate {
    ticks_remaining: u8,
}

impl ClientLoadGate {
    pub(super) fn has_loaded(&self) -> bool {
        self.ticks_remaining == 0
    }

    pub(super) fn restart_after_respawn(&mut self) {
        self.ticks_remaining = CLIENT_LOAD_TIMEOUT_TICKS;
    }

    pub(super) fn acknowledge(&mut self) {
        self.ticks_remaining = 0;
    }

    pub(super) fn tick(&mut self) -> bool {
        if self.ticks_remaining == 0 {
            return false;
        }
        self.ticks_remaining -= 1;
        self.ticks_remaining == 0
    }
}
