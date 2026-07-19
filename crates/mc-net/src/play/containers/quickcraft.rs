pub(in crate::play) const QUICKCRAFT_TYPE_CHARITABLE: i8 = 0;
pub(in crate::play) const QUICKCRAFT_TYPE_GREEDY: i8 = 1;
pub(in crate::play) const QUICKCRAFT_HEADER_START: i8 = 0;
pub(in crate::play) const QUICKCRAFT_HEADER_CONTINUE: i8 = 1;
pub(in crate::play) const QUICKCRAFT_HEADER_END: i8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) struct QuickCraftClick {
    pub(in crate::play) header: i8,
    pub(in crate::play) kind: i8,
    pub(in crate::play) slot: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum QuickCraftOutcome {
    Pending,
    Changed,
    Rejected,
}

#[derive(Debug, Clone)]
pub(in crate::play) struct QuickCraftSelection {
    pub(in crate::play) kind: i8,
    pub(in crate::play) slots: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum QuickCraftStep {
    Started,
    Continued { slot: Option<usize> },
    Finished,
    Rejected,
}

#[derive(Debug, Clone)]
pub(in crate::play) struct QuickCraftState {
    status: i8,
    kind: i8,
    slots: Vec<usize>,
}

impl Default for QuickCraftState {
    fn default() -> Self {
        Self {
            status: QUICKCRAFT_HEADER_START,
            kind: -1,
            slots: Vec::new(),
        }
    }
}

impl QuickCraftState {
    pub(in crate::play) fn reset(&mut self) {
        self.status = QUICKCRAFT_HEADER_START;
        self.kind = -1;
        self.slots.clear();
    }

    pub(in crate::play) fn advance(
        &mut self,
        carried_item_is_empty: bool,
        click: QuickCraftClick,
    ) -> QuickCraftStep {
        if (self.status != QUICKCRAFT_HEADER_CONTINUE || click.header != QUICKCRAFT_HEADER_END)
            && self.status != click.header
        {
            self.reset();
            return QuickCraftStep::Rejected;
        }
        if carried_item_is_empty {
            self.reset();
            return QuickCraftStep::Rejected;
        }

        match click.header {
            QUICKCRAFT_HEADER_START if quickcraft_kind_is_supported(click.kind) => {
                self.status = QUICKCRAFT_HEADER_CONTINUE;
                self.kind = click.kind;
                self.slots.clear();
                QuickCraftStep::Started
            }
            QUICKCRAFT_HEADER_START => {
                self.reset();
                QuickCraftStep::Rejected
            }
            QUICKCRAFT_HEADER_CONTINUE => QuickCraftStep::Continued { slot: click.slot },
            QUICKCRAFT_HEADER_END => QuickCraftStep::Finished,
            _ => {
                self.reset();
                QuickCraftStep::Rejected
            }
        }
    }

    pub(in crate::play) fn finish(&mut self) -> QuickCraftSelection {
        let selection = QuickCraftSelection {
            kind: self.kind,
            slots: self.slots.clone(),
        };
        self.reset();
        selection
    }

    pub(in crate::play) fn selected_slot_count(&self) -> usize {
        self.slots.len()
    }

    pub(in crate::play) fn add_slot(&mut self, slot: usize) {
        if !self.slots.contains(&slot) {
            self.slots.push(slot);
        }
    }
}

pub(in crate::play) fn quickcraft_distribution_count(
    source_count: i32,
    selected_slots: usize,
    kind: i8,
) -> i32 {
    if selected_slots == 0 {
        return 0;
    }

    match kind {
        QUICKCRAFT_TYPE_CHARITABLE => source_count / selected_slots as i32,
        QUICKCRAFT_TYPE_GREEDY => 1,
        _ => 0,
    }
}

fn quickcraft_kind_is_supported(kind: i8) -> bool {
    matches!(kind, QUICKCRAFT_TYPE_CHARITABLE | QUICKCRAFT_TYPE_GREEDY)
}

#[cfg(test)]
#[path = "quickcraft_tests.rs"]
mod tests;
