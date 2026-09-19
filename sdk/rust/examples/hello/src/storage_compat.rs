//! Cross-runtime storage fixture, driven by one outstanding request at a time.
//!
//! `migrate` reads Luau records and receipts before committing a WASM operation.
//! `restart` verifies persisted values, replay and conflicting operation content.
//! `foreign` verifies that another plugin sees neither records nor receipts.
//! Each phase starts from init and logs success only after all typed answers
//! match their request IDs, operation IDs, values, revisions and outcomes.

use solaris_plugin_sdk::events::{OperationFailure, OperationOutcome, OperationPayload};
use solaris_plugin_sdk::{
    commands, log, storage, storage_get, Command, Event, Failure, LogLevel, StorageGetOutcome,
};

/// The key the legacy standalone compare-and-swap wrote. This fixture only reads
/// it: nothing after the legacy package may move it.
const CAS_KEY: &str = "legacy-cas";
/// The value that compare-and-swap left there.
const CAS_VALUE: &str = "from-luau-cas";
/// The revision it landed at, which is also the owner's first commit.
const CAS_REVISION: u64 = 1;

/// The key the legacy batch operation wrote: the record the migration moves and
/// both later phases re-read.
const LEDGER_KEY: &str = "ledger";
/// The value that operation wrote.
const LEDGER_VALUE: &str = "from-luau";
/// The revision that operation committed at.
const LEDGER_REVISION: u64 = 2;

/// The value the migration's own batch writes.
const MIGRATED_VALUE: &str = "from-wasm";
/// The revision the migration commits at, the next one after the legacy record
/// it swaps.
const MIGRATED_REVISION: u64 = 3;

/// A value neither the legacy operation nor the migration wrote. Substituting it
/// in a reused operation id is what the server refuses as a conflict, so both
/// phases that reuse an id with different content send this one.
const SUBSTITUTED_VALUE: &str = "from-wasm-substituted";

/// The operation id the legacy batch committed under.
const LEGACY_OPERATION: &str = "legacy-seed";
/// The operation id the migration commits under: the package's own choice, which
/// is why the server records it beside the legacy one instead of overwriting it.
const MIGRATED_OPERATION: &str = "wasm-write";

/// The keys one committed batch of this fixture changed, exactly as the server
/// reports them back: the ledger, written rather than deleted.
const CHANGES: &[(&str, bool)] = &[(LEDGER_KEY, false)];

/// What one step asks the server to do, in the contract's own records.
#[derive(Clone, Copy)]
enum Ask {
    /// Read one key of this plugin's own storage.
    Read(&'static str),
    /// Commit one batch under a durable operation id: the ledger swapped to
    /// `value`, expected at `expected_version`. This is the shape of the legacy
    /// package's own mutation, so a replay of it is the same operation content.
    Batch {
        operation: &'static str,
        expected_version: Option<u64>,
        value: &'static str,
    },
    /// Look up the outcome the server recorded under a durable operation id.
    Probe(&'static str),
}

/// The one reason a step accepts when it expects a refusal. Another reason is a
/// failure of the step, never a refusal to read past: the two an operation can
/// answer here say different things about the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The operation named something the server never recorded for this owner.
    NotFound,
    /// The operation id is already recorded for different content.
    OperationConflict,
}

impl Refusal {
    /// The refusal a typed failure is, when it is one of the two reasons a step
    /// of this fixture accepts.
    fn of(failure: &OperationFailure) -> Option<Self> {
        match failure {
            OperationFailure::NotFound => Some(Self::NotFound),
            OperationFailure::OperationConflict => Some(Self::OperationConflict),
            _ => None,
        }
    }
}

/// What one step expects its answer to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The key holds nothing: no value and no revision, which is what any owner
    /// but the one that wrote it must see.
    Absent,
    /// The key holds this value at this revision.
    Record { value: &'static str, revision: u64 },
    /// The operation committed at this revision, having changed exactly these
    /// keys.
    Committed {
        revision: u64,
        changes: &'static [(&'static str, bool)],
    },
    /// The server refused with this reason.
    Refused(Refusal),
}

/// One request of a phase, with the answer it must get.
///
/// The request id is this fixture's own and every step of every phase names a
/// different one: a replay is therefore a new request that reuses the durable
/// operation id and the original mutation content, which is what makes the
/// outcome the server recorded the thing that answers it.
struct Step {
    request: &'static str,
    ask: Ask,
    expect: Expect,
}

/// The migration phase: the legacy records, the legacy operation replayed and
/// refused a substitution, the migration committed and replayed, and both keys
/// verified once more before the phase concludes.
const MIGRATE: &[Step] = &[
    Step {
        request: "migrate-read-cas",
        ask: Ask::Read(CAS_KEY),
        expect: Expect::Record {
            value: CAS_VALUE,
            revision: CAS_REVISION,
        },
    },
    Step {
        request: "migrate-read-ledger",
        ask: Ask::Read(LEDGER_KEY),
        expect: Expect::Record {
            value: LEDGER_VALUE,
            revision: LEDGER_REVISION,
        },
    },
    // The legacy operation, replayable because this step sends its original
    // mutation content under a new request id: the server answers the outcome it
    // recorded and applies nothing.
    Step {
        request: "migrate-replay-legacy-seed",
        ask: Ask::Batch {
            operation: LEGACY_OPERATION,
            expected_version: None,
            value: LEDGER_VALUE,
        },
        expect: Expect::Committed {
            revision: LEDGER_REVISION,
            changes: CHANGES,
        },
    },
    // The same operation id with substituted content, under another new request
    // id: refused as a conflict, so the legacy receipt stays the legacy receipt.
    Step {
        request: "migrate-conflict-legacy-seed",
        ask: Ask::Batch {
            operation: LEGACY_OPERATION,
            expected_version: None,
            value: SUBSTITUTED_VALUE,
        },
        expect: Expect::Refused(Refusal::OperationConflict),
    },
    // The migration itself: the ledger the legacy operation left at revision 2,
    // swapped to this package's value at the next revision.
    Step {
        request: "migrate-commit-wasm-write",
        ask: Ask::Batch {
            operation: MIGRATED_OPERATION,
            expected_version: Some(LEDGER_REVISION),
            value: MIGRATED_VALUE,
        },
        expect: Expect::Committed {
            revision: MIGRATED_REVISION,
            changes: CHANGES,
        },
    },
    // The migration replayed with its own original content: the same operation
    // id, the same mutation, another new request id, and the recorded outcome.
    Step {
        request: "migrate-replay-wasm-write",
        ask: Ask::Batch {
            operation: MIGRATED_OPERATION,
            expected_version: Some(LEDGER_REVISION),
            value: MIGRATED_VALUE,
        },
        expect: Expect::Committed {
            revision: MIGRATED_REVISION,
            changes: CHANGES,
        },
    },
    Step {
        request: "migrate-read-ledger-migrated",
        ask: Ask::Read(LEDGER_KEY),
        expect: Expect::Record {
            value: MIGRATED_VALUE,
            revision: MIGRATED_REVISION,
        },
    },
    Step {
        request: "migrate-read-cas-unchanged",
        ask: Ask::Read(CAS_KEY),
        expect: Expect::Record {
            value: CAS_VALUE,
            revision: CAS_REVISION,
        },
    },
];

/// The restart phase: the same world read from a reopened storage, both receipts
/// replayed and probed, and both keys verified unchanged. Nothing here writes a
/// value the first phase did not write.
const RESTART: &[Step] = &[
    Step {
        request: "restart-read-ledger",
        ask: Ask::Read(LEDGER_KEY),
        expect: Expect::Record {
            value: MIGRATED_VALUE,
            revision: MIGRATED_REVISION,
        },
    },
    Step {
        request: "restart-read-cas",
        ask: Ask::Read(CAS_KEY),
        expect: Expect::Record {
            value: CAS_VALUE,
            revision: CAS_REVISION,
        },
    },
    Step {
        request: "restart-replay-wasm-write",
        ask: Ask::Batch {
            operation: MIGRATED_OPERATION,
            expected_version: Some(LEDGER_REVISION),
            value: MIGRATED_VALUE,
        },
        expect: Expect::Committed {
            revision: MIGRATED_REVISION,
            changes: CHANGES,
        },
    },
    Step {
        request: "restart-conflict-wasm-write",
        ask: Ask::Batch {
            operation: MIGRATED_OPERATION,
            expected_version: Some(LEDGER_REVISION),
            value: SUBSTITUTED_VALUE,
        },
        expect: Expect::Refused(Refusal::OperationConflict),
    },
    // Both receipts read back by their durable ids, across the restart that
    // reopened the storage: the migration's own, and the legacy one the migration
    // replayed but never moved.
    Step {
        request: "restart-probe-wasm-write",
        ask: Ask::Probe(MIGRATED_OPERATION),
        expect: Expect::Committed {
            revision: MIGRATED_REVISION,
            changes: CHANGES,
        },
    },
    Step {
        request: "restart-probe-legacy-seed",
        ask: Ask::Probe(LEGACY_OPERATION),
        expect: Expect::Committed {
            revision: LEDGER_REVISION,
            changes: CHANGES,
        },
    },
    Step {
        request: "restart-read-ledger-unchanged",
        ask: Ask::Read(LEDGER_KEY),
        expect: Expect::Record {
            value: MIGRATED_VALUE,
            revision: MIGRATED_REVISION,
        },
    },
    Step {
        request: "restart-read-cas-unchanged",
        ask: Ask::Read(CAS_KEY),
        expect: Expect::Record {
            value: CAS_VALUE,
            revision: CAS_REVISION,
        },
    },
];

/// The foreign phase: the same world, another owner. Neither key may be visible
/// and neither durable id may answer, and this phase sends no mutation at all -
/// a write is what the other two phases exist to verify.
const FOREIGN: &[Step] = &[
    Step {
        request: "foreign-read-ledger",
        ask: Ask::Read(LEDGER_KEY),
        expect: Expect::Absent,
    },
    Step {
        request: "foreign-read-cas",
        ask: Ask::Read(CAS_KEY),
        expect: Expect::Absent,
    },
    Step {
        request: "foreign-probe-wasm-write",
        ask: Ask::Probe(MIGRATED_OPERATION),
        expect: Expect::Refused(Refusal::NotFound),
    },
    Step {
        request: "foreign-probe-legacy-seed",
        ask: Ask::Probe(LEGACY_OPERATION),
        expect: Expect::Refused(Refusal::NotFound),
    },
];

/// One phase of the compatibility fixture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Migrate,
    Restart,
    Foreign,
    /// The configuration named no phase this fixture runs. It is a state of its
    /// own rather than a default: every phase reads or writes the one world, so
    /// choosing one for an absent or misspelled name would decide for the
    /// operator.
    Unconfigured,
}

impl Phase {
    /// The phase `storage_phase` names.
    fn from_name(name: &str) -> Self {
        match name {
            "migrate" => Self::Migrate,
            "restart" => Self::Restart,
            "foreign" => Self::Foreign,
            _ => Self::Unconfigured,
        }
    }

    /// The steps this phase runs, one outstanding request at a time.
    fn plan(self) -> &'static [Step] {
        match self {
            Self::Migrate => MIGRATE,
            Self::Restart => RESTART,
            Self::Foreign => FOREIGN,
            Self::Unconfigured => &[],
        }
    }

    /// The exact line this phase logs once every step of its plan verified. A
    /// test reads it as the phase's own conclusion, so nothing else is on it.
    fn marker(self) -> &'static str {
        match self {
            Self::Migrate => "P3_STORAGE_MIGRATED",
            Self::Restart => "P3_STORAGE_REPLAYED",
            Self::Foreign => "P3_STORAGE_ISOLATED",
            Self::Unconfigured => "",
        }
    }

    /// How this phase is named in a diagnostic line.
    fn name(self) -> &'static str {
        match self {
            Self::Migrate => "migrate",
            Self::Restart => "restart",
            Self::Foreign => "foreign",
            Self::Unconfigured => "unconfigured",
        }
    }
}

/// The fixture itself: the phase it runs and the one step it waits on.
pub struct StorageCompat {
    phase: Phase,
    /// The index of the step whose answer this instance is waiting for. Equal to
    /// the plan's length once the phase concluded, so an answer that arrives
    /// after that is an answer to nothing this instance asked.
    step: usize,
}

impl StorageCompat {
    /// The fixture `storage_phase` in `config.toml` names.
    #[must_use]
    pub fn new(phase: &str) -> Self {
        Self {
            phase: Phase::from_name(phase),
            step: 0,
        }
    }

    /// The phase's first request, answered from `init` so the reads start with no
    /// player and no subscription.
    pub fn init(&mut self) -> Result<Vec<Command>, Failure> {
        self.ask_next()
    }

    /// The commands one delivered batch answers with. The phase holds one request
    /// outstanding, so at most one event of a batch can be the answer it waits
    /// for; every other event is the world going on and is none of this fixture's
    /// business.
    pub fn on_events(&mut self, events: &[Event]) -> Result<Vec<Command>, Failure> {
        let mut answers = events.iter().filter(|event| is_answer(event));
        match (answers.next(), answers.next()) {
            (Some(_), Some(_)) => {
                Err(self.reject("multiple answers arrived for one outstanding request"))
            }
            (Some(event), None) => self.verify(event),
            (None, _) => Ok(Vec::new()),
        }
    }

    /// Ask for the step this instance waits on: the phase's first request, or the
    /// next one after an answer verified.
    fn ask_next(&mut self) -> Result<Vec<Command>, Failure> {
        let Some(step) = self.phase.plan().get(self.step) else {
            return Err(self.reject("no phase of migrate, restart, foreign is configured"));
        };
        Ok(vec![request(step)])
    }

    /// Check one answer against the step that asked for it, then ask for the next
    /// step - or, when it was the phase's last, log the phase's success line and
    /// leave nothing outstanding.
    fn verify(&mut self, event: &Event) -> Result<Vec<Command>, Failure> {
        let Some(step) = self.phase.plan().get(self.step) else {
            return Err(self.reject("an answer arrived with no request outstanding"));
        };
        let (sent, ask, expect) = (step.request, step.ask, step.expect);
        if let Err(detail) = check(sent, ask, expect, event) {
            return Err(self.reject(&detail));
        }
        self.step += 1;
        if self.step == self.phase.plan().len() {
            log(LogLevel::Info, self.phase.marker());
            return Ok(Vec::new());
        }
        self.ask_next()
    }

    /// The one failure this fixture reports: a line naming the phase and the step
    /// an operator has to look at, and the plugin's own failure, so the
    /// deployment sees a callback that did not answer. A phase that ends here
    /// logs no success line.
    fn reject(&self, detail: &str) -> Failure {
        log(
            LogLevel::Error,
            &format!(
                "P3_STORAGE_UNEXPECTED {} step {}: {detail}",
                self.phase.name(),
                self.step
            ),
        );
        Failure::Failed
    }
}

/// The one command one step issues, in the contract's own records.
fn request(step: &Step) -> Command {
    match step.ask {
        Ask::Read(key) => storage_get(step.request, key),
        Ask::Batch {
            operation,
            expected_version,
            value,
        } => commands::Command::StorageBatchCas(commands::StorageBatchCas {
            request: step.request.to_owned(),
            operation_id: operation.to_owned(),
            mutations: vec![commands::StorageMutation::Cas(
                storage::StorageCasMutation {
                    key: LEDGER_KEY.to_owned(),
                    expected_version,
                    value: value.to_owned(),
                },
            )],
        }),
        Ask::Probe(operation) => commands::Command::OperationStatus(commands::OperationStatus {
            request: step.request.to_owned(),
            operation_id: operation.to_owned(),
        }),
    }
}

/// Check one answer against the step that asked for it, and name what was wrong
/// when it does not match.
///
/// Every value the contract carries is compared: the request id the answer echoes
/// against the one this step sent, the durable operation id it names against the
/// one this step addressed, and then the key's value and revision, the commit's
/// revision and its changed keys, or the refusal's reason. An answer of another
/// shape is a failure rather than an outcome to interpret.
fn check(sent: &str, ask: Ask, expect: Expect, event: &Event) -> Result<(), String> {
    match (ask, event) {
        (Ask::Read(key), Event::StorageGetAnswered(answered)) => {
            if answered.request != sent {
                return Err(format!(
                    "the read of {key} was answered under {}, not {sent}",
                    answered.request
                ));
            }
            match &answered.outcome {
                StorageGetOutcome::Read(record) => match expect {
                    Expect::Absent if record.value.is_none() && record.version.is_none() => Ok(()),
                    Expect::Absent => Err(format!(
                        "{key} holds {:?} at {:?}, not nothing",
                        record.value, record.version
                    )),
                    Expect::Record { value, revision }
                        if record.value.as_deref() == Some(value)
                            && record.version == Some(revision) =>
                    {
                        Ok(())
                    }
                    Expect::Record { value, revision } => Err(format!(
                        "{key} reads {:?} at {:?}, not {value} at {revision}",
                        record.value, record.version
                    )),
                    Expect::Committed { .. } | Expect::Refused(_) => {
                        unreachable!("a read is never asked for an operation's outcome")
                    }
                },
                StorageGetOutcome::Failed(failure) => Err(format!(
                    "{key} answered storage {}",
                    super::failure_name(failure)
                )),
            }
        }
        (
            Ask::Batch { operation, .. } | Ask::Probe(operation),
            Event::OperationAnswered(answered),
        ) => {
            if answered.request != sent || answered.operation_id.as_deref() != Some(operation) {
                return Err(format!(
                    "the answer names {} for {:?}, not {sent} for {operation}",
                    answered.request, answered.operation_id
                ));
            }
            match (&answered.outcome, expect) {
                (
                    OperationOutcome::Committed(committed),
                    Expect::Committed { revision, changes },
                ) => {
                    if committed.revision != revision {
                        return Err(format!(
                            "{operation} committed at {}, not {revision}",
                            committed.revision
                        ));
                    }
                    let OperationPayload::StorageBatch(changed) = &committed.payload else {
                        return Err(format!("{operation} returned a non-storage payload"));
                    };
                    if !changed
                        .iter()
                        .map(|change| (change.key.as_str(), change.deleted))
                        .eq(changes.iter().copied())
                    {
                        return Err(format!("{operation} changed {changed:?}, not {changes:?}"));
                    }
                    Ok(())
                }
                (OperationOutcome::Refused(failure), Expect::Refused(wanted)) => {
                    if Refusal::of(&failure.reason) != Some(wanted) {
                        return Err(format!(
                            "{operation} was refused as {}, not {:?}",
                            super::operation_failure_name(&failure.reason),
                            wanted
                        ));
                    }
                    Ok(())
                }
                (outcome, expect) => {
                    Err(format!("{operation} answered {outcome:?}, not {expect:?}"))
                }
            }
        }
        (_, event) => Err(format!("answered by {}", kind(event))),
    }
}

/// Whether one event is the host answering a request this plugin made. Every
/// other event - a join, a chat line, a command - is the world going on and is
/// left alone: the phases are driven from `init`.
fn is_answer(event: &Event) -> bool {
    matches!(
        event,
        Event::StorageGetAnswered(_)
            | Event::StorageCasAnswered(_)
            | Event::OnlinePlayersAnswered(_)
            | Event::PlayerTeleportAnswered(_)
            | Event::OperationAnswered(_)
            | Event::ZoneCommandAnswered(_)
    )
}

/// How one answer event is named in a diagnostic line, in the contract's own
/// vocabulary.
fn kind(event: &Event) -> &'static str {
    match event {
        Event::StorageGetAnswered(_) => "storage-get-answered",
        Event::StorageCasAnswered(_) => "storage-cas-answered",
        Event::OnlinePlayersAnswered(_) => "online-players-answered",
        Event::PlayerTeleportAnswered(_) => "player-teleport-answered",
        Event::OperationAnswered(_) => "operation-answered",
        Event::ZoneCommandAnswered(_) => "zone-command-answered",
        _ => "event",
    }
}
