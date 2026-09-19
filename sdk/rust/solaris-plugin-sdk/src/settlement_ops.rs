//! Settlement commands: the site, survey and structure operations the used
//! settlement flows make against the server's own settlement owner.
//!
//! Each helper builds exactly the record `settlements.wit` declares and nothing
//! more: the ids, coordinates, revisions and cursors are the plugin's own, and the
//! server's own DTO validator is what refuses a value outside the contract's
//! bounds when the host converts the batch. No helper waits for an answer - every
//! call answers later as
//! [`Event::OperationAnswered`](crate::events::Event::OperationAnswered) with the
//! same `request`.

use crate::{domain_operations, settlements, Command};

/// List one bounded page of settlement sites.
///
/// The sites are the ones core describes: the authored catalog of a deployed
/// settlement package and the vanilla villages the world's own generator places.
/// `cursor` is `None` for the first page and the cursor a previous page answered
/// with otherwise; a page always answers the cursor the walk would continue from,
/// and a cursor core cannot parse is answered `cursor-expired` rather than
/// silently restarting the walk.
///
/// A query carries no durable operation id: it changes nothing and creates no
/// receipt, so the answer's `operation-id` is absent rather than invented and a
/// plugin must not read it as one.
#[must_use]
pub fn list_settlement_sites(request: &str, cursor: Option<&str>, limit: u8) -> Command {
    operation(
        request,
        settlements::SettlementOperation::ListSites(settlements::ListSites {
            cursor: cursor.map(str::to_owned),
            limit,
        }),
    )
}

/// Read one page of one site's own points of interest.
///
/// The page is a contiguous slice of the site's canonical point-of-interest order
/// and `cursor` names the last id the plugin received, so a whole site is read
/// without re-reading the ids before it. A site core does not describe is
/// `not-found`, and a cursor the site does not hold is `cursor-expired`.
///
/// A query carries no durable operation id, exactly as [`list_settlement_sites`]
/// does not.
#[must_use]
pub fn query_settlement_site(
    request: &str,
    site_id: &str,
    cursor: Option<&str>,
    limit: u8,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::QuerySite(settlements::QuerySite {
            site_id: site_id.to_owned(),
            cursor: cursor.map(str::to_owned),
            limit,
        }),
    )
}

/// Reserve one free home point of interest of one site.
///
/// The reservation is durable and names the point of interest, not a position: the
/// answer carries core's own opaque `spawn-site-token`, which a resident spawn
/// consumes and [`release_resident_site`] hands back. `expected_site_revision` is
/// the revision the plugin read with [`list_settlement_sites`] or
/// [`query_settlement_site`] - zero when the site holds no durable reservation state
/// yet - and a site that has moved past it is refused `stale-revision` rather than
/// reserved against the wrong image.
///
/// `operation_id` is the durable name the server records the reservation under: a
/// reservation the plugin repeats with byte-identical content replays the recorded
/// receipt and mints nothing, which is what makes retrying after a lost answer
/// safe, while reusing the id for different content is refused
/// `operation-conflict`.
#[must_use]
pub fn reserve_resident_site(
    request: &str,
    operation_id: &str,
    site_id: &str,
    poi_id: &str,
    expected_site_revision: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::ReserveResidentSite(settlements::ReserveResidentSite {
            operation_id: operation_id.to_owned(),
            site_id: site_id.to_owned(),
            poi_id: poi_id.to_owned(),
            expected_site_revision,
        }),
    )
}

/// Hand one reservation back to core.
///
/// The token is core's own, read from the reservation's answer. A reservation a
/// spawn consumed can never be handed back - releasing it would make an occupied
/// home reservable again - so that answer is `blocked`; a token the owner does not
/// hold, including one already released, is `not-found`, and one another plugin
/// holds is `forbidden`. The answer names the site, the point of interest and the
/// token it released.
#[must_use]
pub fn release_resident_site(request: &str, operation_id: &str, spawn_site_token: &str) -> Command {
    operation(
        request,
        settlements::SettlementOperation::ReleaseResidentSite(settlements::ReleaseResidentSite {
            operation_id: operation_id.to_owned(),
            spawn_site_token: spawn_site_token.to_owned(),
        }),
    )
}

/// Survey one bounded region of one dimension.
///
/// The answer is a bounded aggregate snapshot - the usable and water columns, the
/// biome and resource tags, whether another plugin's zone already covers the
/// region - plus core's own opaque `survey-token`, which is what a later
/// [`prepare_structure`] names. A region whose chunks are not loaded is answered
/// `unloaded` rather than with an empty survey, and the token is core's to mint: a
/// plugin never invents one.
///
/// A survey carries no durable operation id: it decides no durable structure, and
/// the receipt a later prepare records is the one that names the work.
#[must_use]
pub fn survey_site(
    request: &str,
    dimension: &str,
    bounds: settlements::SurveyBounds,
    purpose: settlements::SurveyPurpose,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::Survey(settlements::Survey {
            dimension: dimension.to_owned(),
            bounds,
            purpose,
        }),
    )
}

/// Plan one structure of one authored blueprint at one anchor.
///
/// The anchor is the structure's own base row, and the owner refuses a placement it
/// cannot honestly make: a footprint whose chunks are not all loaded is `unloaded`,
/// and one where terrain rises above the base row is `blocked`, so a structure is
/// never planned where it would overwrite what is already there. `survey_token` is
/// the token [`survey_site`] answered with: an expired token, one whose region
/// changed since it was read, or one whose site revision moved is refused
/// `stale-revision`, a foreign one is `forbidden` and one the owner does not hold is
/// `not-found`. The answer is the planned `structure-snapshot`.
///
/// `operation_id` is the durable name the plan is recorded under, with the same
/// replay and `operation-conflict` rules as [`reserve_resident_site`].
#[must_use]
pub fn prepare_structure(
    request: &str,
    operation_id: &str,
    blueprint_id: &str,
    anchor: settlements::BlockPosition,
    rotation: u16,
    survey_token: &str,
    expected_site_revision: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::PrepareStructure(settlements::PrepareStructure {
            operation_id: operation_id.to_owned(),
            blueprint_id: blueprint_id.to_owned(),
            anchor,
            rotation,
            survey_token: survey_token.to_owned(),
            expected_site_revision,
        }),
    )
}

/// Commit one bounded work portion of one stage of a structure.
///
/// `reservation_ref` is the inventory reservation the portion spends from: one the
/// owner does not hold is `not-found`, a released one, one bound to another
/// structure, or one whose resource plan does not match the structure's is
/// `blocked`, and a portion the reservation cannot cover is `insufficient-items`.
/// `work_units` is what the caller authorizes for this portion and the owner commits
/// at most `mc_script::MAX_WORLD_COMMIT_PORTION` of it. The answer is the portion's
/// own `structure-receipt` - the stage, the sequence, the blocks and work units it
/// covered, what it consumed and the revision it left - so a plugin advances by
/// reading receipts rather than by assuming a stage finished. A structure the site
/// changed under is paused by the owner instead, and that `structure-snapshot` is
/// the answer.
#[must_use]
pub fn advance_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    stage: &str,
    reservation_ref: &str,
    expected_revision: u64,
    work_units: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::AdvanceStructure(settlements::AdvanceStructure {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            stage: stage.to_owned(),
            reservation_ref: reservation_ref.to_owned(),
            expected_revision,
            work_units,
        }),
    )
}

/// Pause one active structure.
///
/// The owner records the paused state and answers the resulting
/// `structure-snapshot`; when the site the structure was planned against changed,
/// the snapshot carries the reason it paused. Pausing preserves every block already
/// committed.
#[must_use]
pub fn pause_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::PauseStructure(settlements::PauseStructure {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            expected_revision,
        }),
    )
}

/// Resume one paused structure after reading its authoritative status.
///
/// Resume never compensates or replays a prior portion: core requires the
/// structure revision the caller read and refuses a footprint that no longer
/// matches its last accepted image. A successful result is the new
/// `structure-snapshot`, ready for the next bounded `advance_structure`.
#[must_use]
pub fn resume_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::ResumeStructure(settlements::ResumeStructure {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            expected_revision,
        }),
    )
}

/// Cancel one active structure.
///
/// Cancelling stops future portions and preserves what was already built, which is
/// why it is the call a plugin makes when it stops a project rather than a rollback.
/// A structure that is already committed or cancelled is refused `blocked`, and one
/// the caller does not own is `forbidden`.
#[must_use]
pub fn cancel_structure(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::CancelStructure(settlements::CancelStructure {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            expected_revision,
        }),
    )
}

/// Read one durable structure back.
///
/// The answer is the structure's whole `structure-snapshot`: its state, its stage
/// plan, the watermark and materials of the portions committed so far, and the
/// reservation it is bound to. An id the owner does not hold is `not-found` and one
/// another plugin owns is `forbidden`, never an empty snapshot.
///
/// A query carries no durable operation id, exactly as [`list_settlement_sites`]
/// does not.
#[must_use]
pub fn structure_status(request: &str, structure_id: &str) -> Command {
    operation(
        request,
        settlements::SettlementOperation::StructureStatus(settlements::StructureStatus {
            structure_id: structure_id.to_owned(),
        }),
    )
}

/// Bind one authored container of one active structure and receive its handle.
///
/// The plugin names the structure and the container's authored ordinal; core mints
/// the opaque handle, because a plugin never chooses a container by coordinates. A
/// container already bound to the same plugin answers its original handle and
/// revision, which is why repeating a bind is safe, and one bound to another plugin
/// is `forbidden`. An unplaced structure is `blocked`, a container whose chunk is
/// not loaded is `unloaded`, and one with no container at its placed position is
/// `not-found` - never an empty container.
#[must_use]
pub fn bind_warehouse(
    request: &str,
    operation_id: &str,
    structure_id: &str,
    container_id: u32,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::BindWarehouse(settlements::BindWarehouse {
            operation_id: operation_id.to_owned(),
            structure_id: structure_id.to_owned(),
            container_id,
        }),
    )
}

/// Bind one materialized container of a generator-authenticated vanilla village.
///
/// The plugin names the site core described and its canonical materialized
/// container ordinal, never a block position. Core authenticates the site from
/// its world identity and generator, resolves the exact position, and mints the
/// same opaque warehouse handle used by every resident haul.
#[must_use]
pub fn bind_village_warehouse(
    request: &str,
    operation_id: &str,
    site_id: &str,
    container_id: u32,
) -> Command {
    operation(
        request,
        settlements::SettlementOperation::BindVillageWarehouse(settlements::BindVillageWarehouse {
            operation_id: operation_id.to_owned(),
            site_id: site_id.to_owned(),
            container_id,
        }),
    )
}

/// One settlement record wrapped in the single operation envelope every family
/// shares.
fn operation(request: &str, record: settlements::SettlementOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::Settlement(record),
    })
}
