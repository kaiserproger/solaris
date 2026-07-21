use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    AdmittedScriptCommand, ScriptCommand, ScriptDtoError, ScriptPlayerInventoryFailure,
};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InventoryAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    PublicationClosed,
}

pub(crate) struct PluginInventoryAdapter {
    scripts: ScriptEventSink,
}

impl PluginInventoryAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self { scripts }
    }

    pub(crate) async fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        runtime_available: bool,
    ) -> Result<(), InventoryAdapterError> {
        let ScriptCommand::PlayerInventoryTransaction { transaction } = admitted.request() else {
            return Err(InventoryAdapterError::WrongCommand);
        };
        let failure = if runtime_available {
            sessions
                .commit_script_player_inventory_transaction(transaction, items, item_facts)
                .err()
        } else {
            Some(ScriptPlayerInventoryFailure::RuntimeUnavailable)
        };
        let event = admitted
            .player_inventory_transaction_result(failure)
            .map_err(InventoryAdapterError::InvalidResult)?;
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(()),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(InventoryAdapterError::PublicationClosed)
            }
        }
    }
}
