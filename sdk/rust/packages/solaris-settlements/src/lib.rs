//! Bind authored or generator-authenticated village containers and assign residents to haul into them.

use std::collections::{BTreeMap, VecDeque};

use solaris_plugin_sdk::events::{
    CommandInvoked, Event, OperationAnswered, OperationOutcome, OperationPayload,
    StorageCasAnswered, StorageGetAnswered,
};
use solaris_plugin_sdk::inventories::InventoryEndpoint;
use solaris_plugin_sdk::residents::{HaulWork, ResidentOrderResult, WorkOrder};
use solaris_plugin_sdk::settlements::{SettlementResult, WarehouseBinding, WarehouseSource};
use solaris_plugin_sdk::{
    assign_resident_work, bind_village_warehouse, bind_warehouse, export_plugin, message_session,
    storage_cas, storage_get, Command, Config, EventContext, Failure, InitContext, Plugin,
    StorageCasOutcome, StorageGetOutcome,
};

const WORK_UNITS: u64 = 4_096;
const OPERATION_COUNTER_KEY: &str = "operation-counter";
const MAX_SAFE_REVISION: u64 = (1 << 53) - 1;

#[derive(Clone, Copy)]
enum IssueDestination {
    Carry,
    Equipment,
}

impl IssueDestination {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "carry" => Some(Self::Carry),
            "equipment" => Some(Self::Equipment),
            _ => None,
        }
    }

    fn endpoint(self, resident: String) -> InventoryEndpoint {
        match self {
            Self::Carry => InventoryEndpoint::ResidentCarry(resident),
            Self::Equipment => InventoryEndpoint::ResidentEquipment(resident),
        }
    }
}

#[derive(Clone)]
enum HaulDirection {
    Deposit,
    Issue(IssueDestination),
}

#[derive(Clone)]
enum OperationIntent {
    Bind {
        session: u64,
        structure_id: String,
        container_id: u32,
    },
    BindVillage {
        session: u64,
        site_id: String,
        container_id: u32,
    },
    Haul {
        session: u64,
        resident: String,
        warehouse: String,
        expected_revision: u64,
        direction: HaulDirection,
        item: Option<String>,
    },
}

impl OperationIntent {
    fn session(&self) -> u64 {
        match self {
            Self::Bind { session, .. }
            | Self::BindVillage { session, .. }
            | Self::Haul { session, .. } => *session,
        }
    }
    fn is_bind(&self) -> bool {
        matches!(self, Self::Bind { .. } | Self::BindVillage { .. })
    }
}

#[derive(Clone)]
struct PendingOperation {
    session: u64,
    operation_id: String,
    kind: PendingKind,
}

#[derive(Clone)]
enum PendingKind {
    Bind {
        structure_id: String,
        container_id: u32,
    },
    BindVillage {
        site_id: String,
        container_id: u32,
    },
    Haul {
        resident: String,
    },
}

struct PendingCounter {
    request: String,
    next: u64,
    intent: OperationIntent,
}

#[derive(Default)]
struct Settlements {
    warehouse: Option<WarehouseBinding>,
    resident_revisions: BTreeMap<String, u64>,
    pending: BTreeMap<String, PendingOperation>,
    queued: VecDeque<OperationIntent>,
    pending_counter: Option<PendingCounter>,
    counter: Option<u64>,
    counter_version: Option<u64>,
    counter_read_request: Option<String>,
    counter_read_session: Option<u64>,
    storage_sequence: u64,
}

impl Settlements {
    fn next_storage_request(&mut self, prefix: &str) -> Option<String> {
        self.storage_sequence = self.storage_sequence.checked_add(1)?;
        Some(format!("{prefix}-{}", self.storage_sequence))
    }

    fn binding_pending(&self) -> bool {
        self.pending
            .values()
            .any(|pending| matches!(pending.kind, PendingKind::Bind { .. }))
            || self
                .pending_counter
                .as_ref()
                .is_some_and(|pending| pending.intent.is_bind())
            || self.queued.iter().any(OperationIntent::is_bind)
    }

    fn command(&mut self, invoked: &CommandInvoked) -> Vec<Command> {
        if invoked.name != "settlement" {
            return Vec::new();
        }
        let arguments = invoked
            .arguments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        match arguments.as_slice() {
            ["bind", structure_id, container_id] => self.bind(invoked.session, structure_id, container_id),
            ["bind-village", site_id, container_id] => {
                self.bind_village(invoked.session, site_id, container_id)
            }
            ["haul", resident, expected_revision] => {
                self.haul(invoked.session, resident, expected_revision, None)
            }
            ["haul", resident, expected_revision, item] => {
                self.haul(invoked.session, resident, expected_revision, Some(item))
            }
            ["issue", resident, expected_revision, destination] => {
                self.issue_haul(invoked.session, resident, expected_revision, destination, None)
            }
            ["issue", resident, expected_revision, destination, item] => {
                self.issue_haul(
                    invoked.session,
                    resident,
                    expected_revision,
                    destination,
                    Some(item),
                )
            }
            _ => vec![message_session(
                invoked.session,
                "Usage: /settlement bind <structure-id> <container-id>, /settlement bind-village <village-site-id> <container-id>, /settlement haul <resident-handle> <expected-revision> [item-id], or /settlement issue <resident-handle> <expected-revision> <carry|equipment> [item-id].",
            )],
        }
    }

    fn bind(&mut self, session: u64, structure_id: &str, container_id: &str) -> Vec<Command> {
        if !valid_identity(structure_id) {
            return vec![message_session(
                session,
                "Structure id must be 1..64 bytes.",
            )];
        }
        let Ok(container_id) = container_id.parse::<u32>() else {
            return vec![message_session(
                session,
                "Container id must be an unsigned 32-bit index.",
            )];
        };
        if self.binding_pending() {
            return vec![message_session(
                session,
                "A warehouse binding is already pending.",
            )];
        }
        self.submit(OperationIntent::Bind {
            session,
            structure_id: structure_id.to_owned(),
            container_id,
        })
    }

    fn bind_village(&mut self, session: u64, site_id: &str, container_id: &str) -> Vec<Command> {
        if !valid_identity(site_id) {
            return vec![message_session(
                session,
                "Village site id must be 1..64 bytes.",
            )];
        }
        let Ok(container_id) = container_id.parse::<u32>() else {
            return vec![message_session(
                session,
                "Container id must be an unsigned 32-bit index.",
            )];
        };
        if self.binding_pending() {
            return vec![message_session(
                session,
                "A warehouse binding is already pending.",
            )];
        }
        self.submit(OperationIntent::BindVillage {
            session,
            site_id: site_id.to_owned(),
            container_id,
        })
    }

    fn haul(
        &mut self,
        session: u64,
        resident: &str,
        expected_revision: &str,
        item: Option<&str>,
    ) -> Vec<Command> {
        self.assign_haul(
            session,
            resident,
            expected_revision,
            HaulDirection::Deposit,
            item,
        )
    }

    fn issue_haul(
        &mut self,
        session: u64,
        resident: &str,
        expected_revision: &str,
        destination: &str,
        item: Option<&str>,
    ) -> Vec<Command> {
        let Some(destination) = IssueDestination::parse(destination) else {
            return vec![message_session(
                session,
                "Issue destination must be carry or equipment.",
            )];
        };
        self.assign_haul(
            session,
            resident,
            expected_revision,
            HaulDirection::Issue(destination),
            item,
        )
    }

    fn assign_haul(
        &mut self,
        session: u64,
        resident: &str,
        expected_revision: &str,
        direction: HaulDirection,
        item: Option<&str>,
    ) -> Vec<Command> {
        if !valid_identity(resident) {
            return vec![message_session(
                session,
                "Resident handle must be 1..64 bytes.",
            )];
        }
        let Some(expected_revision) = expected_revision
            .parse::<u64>()
            .ok()
            .filter(|revision| *revision <= MAX_SAFE_REVISION)
        else {
            return vec![message_session(
                session,
                "Expected revision must be an unsigned 53-bit integer.",
            )];
        };
        if item.is_some_and(|item| !valid_resource_id(item)) {
            return vec![message_session(
                session,
                "Item id must be a resource identifier of at most 128 bytes.",
            )];
        }
        let Some(warehouse) = self
            .warehouse
            .as_ref()
            .map(|binding| binding.handle.clone())
        else {
            return vec![message_session(
                session,
                "Bind a warehouse before assigning a haul.",
            )];
        };
        self.submit(OperationIntent::Haul {
            session,
            resident: resident.to_owned(),
            warehouse,
            expected_revision,
            direction,
            item: item.map(str::to_owned),
        })
    }

    fn submit(&mut self, intent: OperationIntent) -> Vec<Command> {
        let session = intent.session();
        self.queued.push_back(intent);
        if self.counter.is_some() {
            self.reserve_next_operation()
        } else {
            self.load_counter(Some(session))
        }
    }

    fn load_counter(&mut self, session: Option<u64>) -> Vec<Command> {
        if self.counter_read_request.is_some() {
            return Vec::new();
        }
        let Some(request) = self.next_storage_request("settlement-counter-read") else {
            return self.fail_queued("No further settlement operations can be issued.");
        };
        self.counter_read_request = Some(request.clone());
        self.counter_read_session = session;
        vec![storage_get(&request, OPERATION_COUNTER_KEY)]
    }

    fn reserve_next_operation(&mut self) -> Vec<Command> {
        if self.counter_read_request.is_some() || self.pending_counter.is_some() {
            return Vec::new();
        }
        let Some(intent) = self.queued.pop_front() else {
            return Vec::new();
        };
        let Some(next) = self.counter.and_then(|counter| counter.checked_add(1)) else {
            return vec![message_session(
                intent.session(),
                "No further settlement operations can be issued.",
            )];
        };
        let Some(request) = self.next_storage_request("settlement-counter-write") else {
            self.queued.push_front(intent);
            return self.fail_queued("No further settlement operations can be issued.");
        };
        self.pending_counter = Some(PendingCounter {
            request: request.clone(),
            next,
            intent,
        });
        vec![storage_cas(
            &request,
            OPERATION_COUNTER_KEY,
            self.counter_version,
            next.to_string(),
        )]
    }

    fn storage_get(&mut self, answered: &StorageGetAnswered) -> Vec<Command> {
        if self.counter_read_request.as_deref() != Some(answered.request.as_str()) {
            return Vec::new();
        }
        self.counter_read_request = None;
        let session = self.counter_read_session.take();
        let StorageGetOutcome::Read(record) = &answered.outcome else {
            return self.counter_load_failed(session);
        };
        let Some(counter) = record.value.as_deref().unwrap_or("0").parse::<u64>().ok() else {
            return self.counter_load_failed(session);
        };
        self.counter = Some(counter);
        self.counter_version = record.version;
        self.reserve_next_operation()
    }

    fn storage_cas(&mut self, answered: &StorageCasAnswered) -> Vec<Command> {
        let Some(pending) = self.pending_counter.take() else {
            return Vec::new();
        };
        if pending.request != answered.request {
            self.pending_counter = Some(pending);
            return Vec::new();
        }
        let StorageCasOutcome::Committed(version) = &answered.outcome else {
            let session = pending.intent.session();
            self.queued.push_front(pending.intent);
            self.counter = None;
            self.counter_version = None;
            return self.load_counter(Some(session));
        };
        self.counter = Some(pending.next);
        self.counter_version = Some(*version);
        let mut commands = self.issue(pending.intent, pending.next);
        commands.extend(self.reserve_next_operation());
        commands
    }

    fn issue(&mut self, intent: OperationIntent, counter: u64) -> Vec<Command> {
        let request = format!("settlement-request-{counter}");
        let operation_id = format!("settlement-operation-{counter}");
        match intent {
            OperationIntent::Bind {
                session,
                structure_id,
                container_id,
            } => {
                self.pending.insert(
                    request.clone(),
                    PendingOperation {
                        session,
                        operation_id: operation_id.clone(),
                        kind: PendingKind::Bind {
                            structure_id: structure_id.clone(),
                            container_id,
                        },
                    },
                );
                vec![bind_warehouse(
                    &request,
                    &operation_id,
                    &structure_id,
                    container_id,
                )]
            }
            OperationIntent::BindVillage {
                session,
                site_id,
                container_id,
            } => {
                self.pending.insert(
                    request.clone(),
                    PendingOperation {
                        session,
                        operation_id: operation_id.clone(),
                        kind: PendingKind::BindVillage {
                            site_id: site_id.clone(),
                            container_id,
                        },
                    },
                );
                vec![bind_village_warehouse(
                    &request,
                    &operation_id,
                    &site_id,
                    container_id,
                )]
            }
            OperationIntent::Haul {
                session,
                resident,
                warehouse,
                expected_revision,
                direction,
                item,
            } => {
                self.pending.insert(
                    request.clone(),
                    PendingOperation {
                        session,
                        operation_id: operation_id.clone(),
                        kind: PendingKind::Haul {
                            resident: resident.clone(),
                        },
                    },
                );
                let (source, destination) = match direction {
                    HaulDirection::Deposit => (
                        InventoryEndpoint::ResidentCarry(resident.clone()),
                        InventoryEndpoint::Warehouse(warehouse),
                    ),
                    HaulDirection::Issue(destination) => (
                        InventoryEndpoint::Warehouse(warehouse),
                        destination.endpoint(resident.clone()),
                    ),
                };
                vec![assign_resident_work(
                    &request,
                    &operation_id,
                    &resident,
                    WorkOrder::Haul(HaulWork {
                        source,
                        destination,
                        item,
                    }),
                    WORK_UNITS,
                    expected_revision,
                )]
            }
        }
    }

    fn answered(&mut self, answered: &OperationAnswered) -> Vec<Command> {
        let Some(pending) = self.pending.get(&answered.request).cloned() else {
            return Vec::new();
        };
        if answered.operation_id.as_deref() != Some(pending.operation_id.as_str()) {
            return Vec::new();
        }
        self.pending.remove(&answered.request);
        match pending.kind {
            PendingKind::Bind {
                structure_id,
                container_id,
            } => self.bind_answer(
                pending.session,
                &structure_id,
                container_id,
                &answered.outcome,
            ),
            PendingKind::BindVillage {
                site_id,
                container_id,
            } => {
                self.bind_village_answer(pending.session, &site_id, container_id, &answered.outcome)
            }
            PendingKind::Haul { resident } => {
                self.haul_answer(pending.session, &resident, &answered.outcome)
            }
        }
    }

    fn bind_answer(
        &mut self,
        session: u64,
        structure_id: &str,
        container_id: u32,
        outcome: &OperationOutcome,
    ) -> Vec<Command> {
        match outcome {
            OperationOutcome::Committed(committed) => {
                let OperationPayload::Settlement(SettlementResult::Warehouse(binding)) =
                    &committed.payload
                else {
                    return vec![message_session(
                        session,
                        "Warehouse binding returned an unexpected result.",
                    )];
                };
                if !matches!(
                    &binding.source,
                    WarehouseSource::Authored(source)
                        if source.structure_id.as_str() == structure_id
                            && source.container_id == container_id
                ) {
                    return vec![message_session(
                        session,
                        "Warehouse binding returned an unexpected result.",
                    )];
                }
                self.warehouse = Some(binding.clone());
                vec![message_session(session, "Warehouse bound.")]
            }
            OperationOutcome::Refused(refused) => vec![message_session(
                session,
                format!("Warehouse binding refused: {:?}.", refused.reason),
            )],
        }
    }

    fn bind_village_answer(
        &mut self,
        session: u64,
        site_id: &str,
        container_id: u32,
        outcome: &OperationOutcome,
    ) -> Vec<Command> {
        match outcome {
            OperationOutcome::Committed(committed) => {
                let OperationPayload::Settlement(SettlementResult::Warehouse(binding)) =
                    &committed.payload
                else {
                    return vec![message_session(
                        session,
                        "Warehouse binding returned an unexpected result.",
                    )];
                };
                if !matches!(
                    &binding.source,
                    WarehouseSource::VanillaVillage(source)
                        if source.site_id.as_str() == site_id
                            && source.container_id == container_id
                ) {
                    return vec![message_session(
                        session,
                        "Warehouse binding returned an unexpected result.",
                    )];
                }
                self.warehouse = Some(binding.clone());
                vec![message_session(session, "Warehouse bound.")]
            }
            OperationOutcome::Refused(refused) => vec![message_session(
                session,
                format!("Warehouse binding refused: {:?}.", refused.reason),
            )],
        }
    }

    fn haul_answer(
        &mut self,
        session: u64,
        resident: &str,
        outcome: &OperationOutcome,
    ) -> Vec<Command> {
        match outcome {
            OperationOutcome::Committed(committed) => {
                let OperationPayload::ResidentOrder(ResidentOrderResult::Work(assignment)) =
                    &committed.payload
                else {
                    return vec![message_session(
                        session,
                        "Haul returned an unexpected result.",
                    )];
                };
                if assignment.handle.as_str() != resident {
                    return vec![message_session(
                        session,
                        "Haul returned an unexpected result.",
                    )];
                }
                self.resident_revisions
                    .insert(resident.to_owned(), assignment.revision);
                vec![message_session(session, "Haul assigned.")]
            }
            OperationOutcome::Refused(refused) => vec![message_session(
                session,
                format!("Haul refused: {:?}.", refused.reason),
            )],
        }
    }

    fn counter_load_failed(&mut self, session: Option<u64>) -> Vec<Command> {
        self.counter = None;
        self.counter_version = None;
        let mut commands = self.fail_queued("Settlement operation counter is unavailable.");
        if commands.is_empty() {
            if let Some(session) = session {
                commands.push(message_session(
                    session,
                    "Settlement operation counter is unavailable.",
                ));
            }
        }
        commands
    }

    fn fail_queued(&mut self, message: &str) -> Vec<Command> {
        self.queued
            .drain(..)
            .map(|intent| message_session(intent.session(), message))
            .collect()
    }

    fn clear_unapplied(&mut self) {
        self.pending.clear();
        self.queued.clear();
        self.pending_counter = None;
        self.counter_read_request = None;
        self.counter_read_session = None;
    }
}

impl Plugin for Settlements {
    fn configure(
        &mut self,
        _config: &Config,
    ) -> Result<Option<solaris_plugin_sdk::StartupContribution>, Failure> {
        Ok(None)
    }

    fn init(&mut self, _config: &Config, _context: &InitContext) -> Result<Vec<Command>, Failure> {
        self.warehouse = None;
        self.resident_revisions.clear();
        self.pending.clear();
        self.queued.clear();
        self.pending_counter = None;
        self.counter = None;
        self.counter_version = None;
        self.counter_read_request = None;
        self.counter_read_session = None;
        self.storage_sequence = 0;
        Ok(self.load_counter(None))
    }

    fn on_events(
        &mut self,
        _context: &EventContext,
        events: &[Event],
    ) -> Result<Vec<Command>, Failure> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::CommandBatchRejected => self.clear_unapplied(),
                Event::CommandInvoked(invoked) => commands.extend(self.command(invoked)),
                Event::StorageGetAnswered(answered) => commands.extend(self.storage_get(answered)),
                Event::StorageCasAnswered(answered) => commands.extend(self.storage_cas(answered)),
                Event::OperationAnswered(answered) => commands.extend(self.answered(answered)),
                _ => {}
            }
        }
        Ok(commands)
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64
}

fn valid_resource_id(value: &str) -> bool {
    let Some((namespace, path)) = value.split_once(':') else {
        return false;
    };
    value.len() <= 128
        && !namespace.is_empty()
        && !path.is_empty()
        && namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
        })
        && path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'.' | b'/' | b'-')
        })
}

export_plugin!(Settlements);
#[cfg(test)]
mod tests {
    use super::*;
    use solaris_plugin_sdk::domain_operations::DomainOperation;
    use solaris_plugin_sdk::settlements::SettlementOperation;

    #[test]
    fn bind_village_command_issues_a_site_and_ordinal_only() {
        let commands = Settlements::default().issue(
            OperationIntent::BindVillage {
                session: 7,
                site_id: "village_3_5_0123456789abcdef".to_owned(),
                container_id: 2,
            },
            1,
        );
        assert!(matches!(
            commands.as_slice(),
            [Command::Operation(operation)]
                if operation.request == "settlement-request-1"
                    && matches!(
                        &operation.operation,
                        DomainOperation::Settlement(
                            SettlementOperation::BindVillageWarehouse(bind)
                        ) if bind.operation_id == "settlement-operation-1"
                            && bind.site_id == "village_3_5_0123456789abcdef"
                            && bind.container_id == 2
                    )
        ));
    }
}
