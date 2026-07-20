use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{ContainerInput, ItemStack};
use mc_script::{
    ScriptEvent, ScriptInventoryClick, ScriptInventoryMenu, ScriptPlayerContext, ScriptPlayerId,
    ScriptPluginTarget,
};

use crate::play::inventory::PlayerInventory;

const MENU_COLUMNS: usize = 9;
const MAX_MENU_ROWS: usize = 6;

/// A server-owned generic 9xN menu. The script-provided slots are fixed
/// buttons; the appended player inventory is never script-addressable.
#[derive(Debug, Clone)]
pub(in crate::play) struct ScriptMenuWindow {
    pub(in crate::play) container_id: i32,
    pub(in crate::play) state_id: i32,
    owner: ScriptPluginTarget,
    player_id: ScriptPlayerId,
    menu: ScriptInventoryMenu,
    layout: ScriptMenuLayout,
}

impl ScriptMenuWindow {
    pub(in crate::play) fn open(
        container_id: i32,
        owner: ScriptPluginTarget,
        player_id: ScriptPlayerId,
        menu: ScriptInventoryMenu,
        items: &ItemRegistry,
    ) -> Result<Self, ScriptMenuOpenError> {
        let layout = ScriptMenuLayout::open(menu.clone(), items)?;
        Ok(Self {
            container_id,
            state_id: 1,
            owner,
            player_id,
            menu,
            layout,
        })
    }

    pub(in crate::play) fn rows(&self) -> usize {
        self.layout.rows()
    }

    pub(in crate::play) fn menu_type(&self) -> i32 {
        i32::try_from(self.rows() - 1).expect("script menu rows are bounded")
    }

    pub(in crate::play) fn title(&self) -> &str {
        self.menu.title()
    }

    /// Generic containers append precisely the 27 main-inventory and nine
    /// hotbar slots. Crafting, armor, offhand, and cursor state are never
    /// script-menu slots.
    pub(in crate::play) fn wire_items(&self, inventory: &PlayerInventory) -> Vec<ItemStack> {
        self.layout.wire_items(inventory)
    }

    pub(in crate::play) fn click(
        &self,
        click: ScriptMenuClick,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
    ) -> Result<ScriptEvent, ScriptMenuClickDisposition> {
        if player_id != self.player_id {
            return Err(ScriptMenuClickDisposition::Resync);
        }
        let ScriptMenuClickDisposition::Clicked { slot, click } = self.layout.classify_click(click)
        else {
            return Err(ScriptMenuClickDisposition::Resync);
        };
        self.owner
            .inventory_menu_clicked(player_id, context, &self.menu, slot, click)
            .map_err(|_| ScriptMenuClickDisposition::Resync)
    }

    pub(in crate::play) fn matches_close(
        &self,
        plugin_id: &str,
        player_id: ScriptPlayerId,
        menu_id: &str,
    ) -> bool {
        close_identity_matches(
            self.owner.plugin_id(),
            self.player_id,
            self.menu.id(),
            plugin_id,
            player_id,
            menu_id,
        )
    }
}

pub(in crate::play) fn close_identity_matches(
    owner_plugin_id: &str,
    owner_player_id: ScriptPlayerId,
    owner_menu_id: &str,
    requested_plugin_id: &str,
    requested_player_id: ScriptPlayerId,
    requested_menu_id: &str,
) -> bool {
    owner_plugin_id == requested_plugin_id
        && owner_player_id == requested_player_id
        && owner_menu_id == requested_menu_id
}

pub(in crate::play) fn client_close_matches(
    active_container_id: i32,
    received_container_id: i32,
) -> bool {
    active_container_id == received_container_id
}

#[derive(Debug, Clone)]
pub(in crate::play) struct ScriptMenuLayout {
    rows: usize,
    slots: Vec<ItemStack>,
}

impl ScriptMenuLayout {
    pub(in crate::play) fn open(
        menu: ScriptInventoryMenu,
        items: &ItemRegistry,
    ) -> Result<Self, ScriptMenuOpenError> {
        let highest_slot = menu
            .slots()
            .iter()
            .map(|slot| usize::from(slot.index()))
            .max();
        let rows = highest_slot.map_or(1, |slot| slot / MENU_COLUMNS + 1);
        if rows > MAX_MENU_ROWS {
            return Err(ScriptMenuOpenError::InvalidRows);
        }
        let mut slots = vec![ItemStack::EMPTY; rows * MENU_COLUMNS];
        for slot in menu.slots() {
            let id = Identifier::parse(slot.item().resource_id().to_owned()).map_err(|_| {
                ScriptMenuOpenError::UnknownItem(slot.item().resource_id().to_owned())
            })?;
            let item_id = items.id_of(&id).ok_or_else(|| {
                ScriptMenuOpenError::UnknownItem(slot.item().resource_id().to_owned())
            })?;
            let mut stack = ItemStack::new(item_id, i32::from(slot.item().count()));
            if let Some(label) = slot.item().label() {
                stack = stack.with_custom_name(label);
            }
            slots[usize::from(slot.index())] = stack;
        }
        Ok(Self { rows, slots })
    }

    pub(in crate::play) fn rows(&self) -> usize {
        self.rows
    }

    #[cfg(test)]
    pub(in crate::play) fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    pub(in crate::play) fn wire_items(&self, inventory: &PlayerInventory) -> Vec<ItemStack> {
        self.slots
            .iter()
            .cloned()
            .chain(inventory.slots[9..=44].iter().cloned())
            .collect()
    }

    pub(in crate::play) fn classify_click(
        &self,
        click: ScriptMenuClick,
    ) -> ScriptMenuClickDisposition {
        if click.container_id != click.expected_container_id
            || click.state_id != click.expected_state_id
            || click.slot < 0
        {
            return ScriptMenuClickDisposition::Resync;
        }
        let Ok(slot) = usize::try_from(click.slot) else {
            return ScriptMenuClickDisposition::Resync;
        };
        if slot >= self.slots.len() || self.slots[slot].is_empty() {
            return ScriptMenuClickDisposition::Resync;
        }
        let click = match (click.input, click.button) {
            (ContainerInput::Pickup, 0) => ScriptInventoryClick::Primary,
            (ContainerInput::Pickup, 1) => ScriptInventoryClick::Secondary,
            (ContainerInput::QuickMove, 0) => ScriptInventoryClick::ShiftPrimary,
            (ContainerInput::QuickMove, 1) => ScriptInventoryClick::ShiftSecondary,
            _ => return ScriptMenuClickDisposition::Resync,
        };
        ScriptMenuClickDisposition::Clicked {
            slot: u8::try_from(slot).expect("script slot is bounded"),
            click,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::play) struct ScriptMenuClick {
    expected_container_id: i32,
    expected_state_id: i32,
    container_id: i32,
    state_id: i32,
    slot: i16,
    input: ContainerInput,
    button: i8,
}

impl ScriptMenuClick {
    #[cfg(test)]
    pub(in crate::play) fn primary(container_id: i32, state_id: i32, slot: i16) -> Self {
        Self {
            expected_container_id: 4,
            expected_state_id: 7,
            container_id,
            state_id,
            slot,
            input: ContainerInput::Pickup,
            button: 0,
        }
    }

    pub(in crate::play) fn from_packet(
        expected_container_id: i32,
        expected_state_id: i32,
        container_id: i32,
        state_id: i32,
        slot: i16,
        input: ContainerInput,
        button: i8,
    ) -> Self {
        Self {
            expected_container_id,
            expected_state_id,
            container_id,
            state_id,
            slot,
            input,
            button,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum ScriptMenuClickDisposition {
    Clicked {
        slot: u8,
        click: ScriptInventoryClick,
    },
    Resync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) enum ScriptMenuOpenError {
    UnknownItem(String),
    InvalidRows,
}
