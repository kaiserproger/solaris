//! Closed schema-2 client view DTOs and the wire-3 view lifecycle requests.
//!
//! These types are the frozen shared contract between the core UI endpoint and
//! the Solaris Loader: bundle schema 2 / wire protocol 3, a closed widget model
//! of bounded rows/fields/actions and explicit byte bounds on every string.
//! They carry no client authority: the server supplies view instance ids,
//! revisions, actor identity and prices, and a plugin may only echo a
//! server-issued selection context.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use crate::{
    ScriptDtoError, ScriptPlayerId, check_contract_resource_id, validate_bounded_nonempty,
    validate_contract_resource_id, validate_script_id,
};

/// Largest encoded wire-3 view message, including its protocol header.
pub const MAX_CLIENT_VIEW_PACKET_BYTES: usize = 64 * 1024;
/// Hard bound on rows carried by one presented page.
pub const MAX_CLIENT_VIEW_ROWS: usize = 64;
/// Hard bound on typed form fields carried by one presented model.
pub const MAX_CLIENT_VIEW_FIELDS: usize = 16;
/// Hard bound on actions carried by one presented model.
pub const MAX_CLIENT_VIEW_ACTIONS: usize = 16;
/// Hard bound on tabs carried by one presented model.
pub const MAX_CLIENT_VIEW_TABS: usize = 16;
/// Hard bound on resource entries carried by one presented model.
pub const MAX_CLIENT_VIEW_RESOURCES: usize = 16;
/// Hard bound on world markers carried by one presented model.
pub const MAX_CLIENT_VIEW_MARKERS: usize = 16;
/// Byte bound on a view title.
pub const MAX_CLIENT_VIEW_TITLE_BYTES: usize = 128;
/// Byte bound on a view, field, action, tab, marker, resource or context id.
pub const MAX_CLIENT_VIEW_ID_BYTES: usize = 128;
/// Byte bound on one table cell.
pub const MAX_CLIENT_VIEW_CELL_BYTES: usize = 256;
/// Byte bound on one typed text field value.
pub const MAX_CLIENT_VIEW_TEXT_BYTES: usize = 256;
/// Byte bound on an action deny reason.
pub const MAX_CLIENT_VIEW_DENY_REASON_BYTES: usize = 128;
/// Byte bound on a model reason.
pub const MAX_CLIENT_VIEW_REASON_BYTES: usize = 256;
/// Largest accepted marker radius.
pub const MAX_CLIENT_VIEW_MARKER_RADIUS: f64 = 128.0;
/// Largest accepted selection range limit in blocks.
pub const MAX_CLIENT_VIEW_RANGE_LIMIT: u32 = 256;
/// Largest accepted selection context lifetime in simulation ticks.
pub const MAX_CLIENT_VIEW_SELECTION_TTL_TICKS: u64 = 72_000;
/// Largest exactly-representable revision or sequence (Luau exact integers).
pub const MAX_CLIENT_VIEW_EXACT_INTEGER: u64 = (1_u64 << 53) - 1;

/// The six declarative settlement screen kinds of contract section 8.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ScriptClientViewScreenKind {
    Settlement,
    Construction,
    Economy,
    Garrison,
    Army,
    Hud,
}

impl ScriptClientViewScreenKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Settlement => "settlement",
            Self::Construction => "construction",
            Self::Economy => "economy",
            Self::Garrison => "garrison",
            Self::Army => "army",
            Self::Hud => "hud",
        }
    }
}

/// The two key-driven view kinds a client may explicitly request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ScriptClientViewRequestKind {
    Settlement,
    Army,
}

impl ScriptClientViewRequestKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Settlement => "settlement",
            Self::Army => "army",
        }
    }

    #[must_use]
    pub const fn screen_kind(self) -> ScriptClientViewScreenKind {
        match self {
            Self::Settlement => ScriptClientViewScreenKind::Settlement,
            Self::Army => ScriptClientViewScreenKind::Army,
        }
    }

    #[must_use]
    pub fn from_contract_name(value: &str) -> Option<Self> {
        match value {
            "settlement" => Some(Self::Settlement),
            "army" => Some(Self::Army),
            _ => None,
        }
    }
}

/// One formation the server may render for a squad order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ScriptClientViewFormation {
    Line,
    Column,
    Wedge,
    Square,
}

impl ScriptClientViewFormation {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Column => "column",
            Self::Wedge => "wedge",
            Self::Square => "square",
        }
    }
}

/// One table row; cells are bounded display strings, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScriptClientViewRow {
    cells: Vec<String>,
}

impl ScriptClientViewRow {
    pub fn try_new(cells: Vec<String>) -> Result<Self, ScriptDtoError> {
        if cells.len() > MAX_CLIENT_VIEW_FIELDS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view row cells",
                max: MAX_CLIENT_VIEW_FIELDS,
            });
        }
        for cell in &cells {
            crate::validate_bounded_value("view cell", cell, MAX_CLIENT_VIEW_CELL_BYTES)?;
        }
        Ok(Self { cells })
    }

    #[must_use]
    pub fn cells(&self) -> &[String] {
        &self.cells
    }
}

/// One typed form value; exactly one variant is present.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ScriptClientViewFieldValue {
    Number(f64),
    Text(String),
    Selected(String),
}

// Every number is validated finite at construction, so structural equality is
// a valid equivalence relation here (`ScriptCommand`/`ScriptEventKind` are `Eq`).
impl Eq for ScriptClientViewFieldValue {}

/// One typed form field echoed by the client and validated by the server.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptClientViewField {
    id: String,
    value: ScriptClientViewFieldValue,
}

impl Eq for ScriptClientViewField {}

impl ScriptClientViewField {
    pub fn try_number(id: &str, number: f64) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view field id", id, MAX_CLIENT_VIEW_ID_BYTES)?;
        if !number.is_finite() {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            id: id.to_owned(),
            value: ScriptClientViewFieldValue::Number(number),
        })
    }

    pub fn try_text(id: &str, text: String) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view field id", id, MAX_CLIENT_VIEW_ID_BYTES)?;
        crate::validate_bounded_value("view text field", &text, MAX_CLIENT_VIEW_TEXT_BYTES)?;
        Ok(Self {
            id: id.to_owned(),
            value: ScriptClientViewFieldValue::Text(text),
        })
    }

    pub fn try_selected(id: &str, selected: &str) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view field id", id, MAX_CLIENT_VIEW_ID_BYTES)?;
        validate_bounded_nonempty("view selection", selected, MAX_CLIENT_VIEW_ID_BYTES)?;
        Ok(Self {
            id: id.to_owned(),
            value: ScriptClientViewFieldValue::Selected(selected.to_owned()),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn value(&self) -> &ScriptClientViewFieldValue {
        &self.value
    }
}

impl Serialize for ScriptClientViewField {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("id", &self.id)?;
        match &self.value {
            ScriptClientViewFieldValue::Number(number) => {
                map.serialize_entry("number", number)?;
            }
            ScriptClientViewFieldValue::Text(text) => {
                map.serialize_entry("text", text)?;
            }
            ScriptClientViewFieldValue::Selected(selected) => {
                map.serialize_entry("selected", selected)?;
            }
        }
        map.end()
    }
}

/// One declared action plus its current enablement and optional caption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScriptClientViewAction {
    action_id: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deny_reason: Option<String>,
}

impl ScriptClientViewAction {
    pub fn try_new(
        action_id: &str,
        enabled: bool,
        label: Option<String>,
        deny_reason: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view action id", action_id, MAX_CLIENT_VIEW_ID_BYTES)?;
        if let Some(label) = label.as_deref() {
            crate::validate_bounded_value("view action label", label, MAX_CLIENT_VIEW_TITLE_BYTES)?;
        }
        if let Some(reason) = deny_reason.as_deref() {
            crate::validate_bounded_value(
                "view action deny reason",
                reason,
                MAX_CLIENT_VIEW_DENY_REASON_BYTES,
            )?;
        }
        Ok(Self {
            action_id: action_id.to_owned(),
            enabled,
            label,
            deny_reason,
        })
    }

    #[must_use]
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }
}

/// One tab heading bound to a tabs widget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScriptClientViewTab {
    id: String,
    label: String,
}

impl ScriptClientViewTab {
    pub fn try_new(id: &str, label: &str) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view tab id", id, MAX_CLIENT_VIEW_ID_BYTES)?;
        validate_bounded_nonempty("view tab label", label, MAX_CLIENT_VIEW_CELL_BYTES)?;
        Ok(Self {
            id: id.to_owned(),
            label: label.to_owned(),
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// One server-authoritative resource amount shown in a resource panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptClientViewResourceEntry {
    id: String,
    have: f64,
    need: f64,
}

impl Eq for ScriptClientViewResourceEntry {}

impl ScriptClientViewResourceEntry {
    pub fn try_new(id: &str, have: f64, need: f64) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view resource id", id, MAX_CLIENT_VIEW_ID_BYTES)?;
        if !have.is_finite() || !need.is_finite() || have < 0.0 || need < 0.0 {
            return Err(ScriptDtoError::InvalidAmount);
        }
        Ok(Self {
            id: id.to_owned(),
            have,
            need,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn have(&self) -> f64 {
        self.have
    }

    #[must_use]
    pub const fn need(&self) -> f64 {
        self.need
    }
}

/// One world-selection affordance. `selection_token` is the opaque,
/// server-issued selection-context id the client may only echo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptClientViewMarker {
    marker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    formation: Option<ScriptClientViewFormation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    radius: Option<f64>,
}

impl Eq for ScriptClientViewMarker {}

impl ScriptClientViewMarker {
    pub fn try_new(
        marker_id: &str,
        selection_token: Option<String>,
        action_id: Option<String>,
        formation: Option<ScriptClientViewFormation>,
        radius: Option<f64>,
    ) -> Result<Self, ScriptDtoError> {
        validate_bounded_nonempty("view marker id", marker_id, MAX_CLIENT_VIEW_ID_BYTES)?;
        if let Some(token) = selection_token.as_deref() {
            validate_bounded_nonempty("view selection token", token, MAX_CLIENT_VIEW_ID_BYTES)?;
        }
        if let Some(action_id) = action_id.as_deref() {
            validate_bounded_nonempty("view marker action", action_id, MAX_CLIENT_VIEW_ID_BYTES)?;
        }
        if selection_token.is_some() && action_id.is_none() {
            return Err(ScriptDtoError::EmptyValue {
                field: "view marker action",
            });
        }
        if let Some(radius) = radius
            && (!radius.is_finite() || !(0.0..=MAX_CLIENT_VIEW_MARKER_RADIUS).contains(&radius))
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            marker_id: marker_id.to_owned(),
            selection_token,
            action_id,
            formation,
            radius,
        })
    }

    #[must_use]
    pub fn marker_id(&self) -> &str {
        &self.marker_id
    }

    #[must_use]
    pub fn selection_token(&self) -> Option<&str> {
        self.selection_token.as_deref()
    }

    #[must_use]
    pub fn action_id(&self) -> Option<&str> {
        self.action_id.as_deref()
    }
}

impl Serialize for ScriptClientViewFormation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.contract_name())
    }
}

/// One bounded page of one declarative screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptClientViewModel {
    page: u32,
    page_count: u32,
    rows: Vec<ScriptClientViewRow>,
    fields: Vec<ScriptClientViewField>,
    actions: Vec<ScriptClientViewAction>,
    tabs: Vec<ScriptClientViewTab>,
    resource_entries: Vec<ScriptClientViewResourceEntry>,
    markers: Vec<ScriptClientViewMarker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl Eq for ScriptClientViewModel {}

impl ScriptClientViewModel {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        page: u32,
        page_count: u32,
        rows: Vec<ScriptClientViewRow>,
        fields: Vec<ScriptClientViewField>,
        actions: Vec<ScriptClientViewAction>,
        tabs: Vec<ScriptClientViewTab>,
        resource_entries: Vec<ScriptClientViewResourceEntry>,
        markers: Vec<ScriptClientViewMarker>,
        reason: Option<String>,
    ) -> Result<Self, ScriptDtoError> {
        if page_count == 0 || page >= page_count {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if rows.len() > MAX_CLIENT_VIEW_ROWS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view rows",
                max: MAX_CLIENT_VIEW_ROWS,
            });
        }
        if fields.len() > MAX_CLIENT_VIEW_FIELDS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view fields",
                max: MAX_CLIENT_VIEW_FIELDS,
            });
        }
        if actions.len() > MAX_CLIENT_VIEW_ACTIONS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view actions",
                max: MAX_CLIENT_VIEW_ACTIONS,
            });
        }
        if tabs.len() > MAX_CLIENT_VIEW_TABS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view tabs",
                max: MAX_CLIENT_VIEW_TABS,
            });
        }
        if resource_entries.len() > MAX_CLIENT_VIEW_RESOURCES {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view resource entries",
                max: MAX_CLIENT_VIEW_RESOURCES,
            });
        }
        if markers.len() > MAX_CLIENT_VIEW_MARKERS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "view markers",
                max: MAX_CLIENT_VIEW_MARKERS,
            });
        }
        if let Some(reason) = reason.as_deref() {
            crate::validate_bounded_value("view reason", reason, MAX_CLIENT_VIEW_REASON_BYTES)?;
        }
        require_unique(fields.iter().map(ScriptClientViewField::id), "view field")?;
        require_unique(
            actions.iter().map(ScriptClientViewAction::action_id),
            "view action",
        )?;
        require_unique(tabs.iter().map(ScriptClientViewTab::id), "view tab")?;
        require_unique(
            resource_entries
                .iter()
                .map(ScriptClientViewResourceEntry::id),
            "view resource",
        )?;
        require_unique(
            markers.iter().map(ScriptClientViewMarker::marker_id),
            "view marker",
        )?;
        let mut tokens = std::collections::BTreeSet::new();
        for marker in &markers {
            if let Some(token) = marker.selection_token()
                && !tokens.insert(token)
            {
                return Err(ScriptDtoError::DuplicateId {
                    field: "view selection token",
                    actual_bytes: token.len(),
                });
            }
        }
        let model = Self {
            page,
            page_count,
            rows,
            fields,
            actions,
            tabs,
            resource_entries,
            markers,
            reason,
        };
        Ok(model)
    }

    #[must_use]
    pub const fn page(&self) -> u32 {
        self.page
    }

    #[must_use]
    pub const fn page_count(&self) -> u32 {
        self.page_count
    }

    #[must_use]
    pub fn rows(&self) -> &[ScriptClientViewRow] {
        &self.rows
    }

    #[must_use]
    pub fn fields(&self) -> &[ScriptClientViewField] {
        &self.fields
    }

    #[must_use]
    pub fn actions(&self) -> &[ScriptClientViewAction] {
        &self.actions
    }

    #[must_use]
    pub fn tabs(&self) -> &[ScriptClientViewTab] {
        &self.tabs
    }

    #[must_use]
    pub fn resource_entries(&self) -> &[ScriptClientViewResourceEntry] {
        &self.resource_entries
    }

    #[must_use]
    pub fn markers(&self) -> &[ScriptClientViewMarker] {
        &self.markers
    }

    /// Typed field ids the model currently presents; the only ids a client may
    /// echo back in a `view_action`.
    #[must_use]
    pub fn field_ids(&self) -> std::collections::BTreeSet<&str> {
        self.fields.iter().map(ScriptClientViewField::id).collect()
    }
}

fn require_unique<'a>(
    values: impl Iterator<Item = &'a str>,
    field: &'static str,
) -> Result<(), ScriptDtoError> {
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ScriptDtoError::DuplicateId {
                field,
                actual_bytes: value.len(),
            });
        }
    }
    Ok(())
}

/// Constraints bound into one server-issued world-selection context.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptClientSelectionConstraints {
    dimension: String,
    range_limit: u32,
    ttl_ticks: u64,
    formation: Option<ScriptClientViewFormation>,
    radius: Option<f64>,
}

impl Eq for ScriptClientSelectionConstraints {}

impl ScriptClientSelectionConstraints {
    pub fn try_new(
        dimension: &str,
        range_limit: u32,
        ttl_ticks: u64,
        formation: Option<ScriptClientViewFormation>,
        radius: Option<f64>,
    ) -> Result<Self, ScriptDtoError> {
        check_contract_resource_id(dimension)?;
        if range_limit == 0 || range_limit > MAX_CLIENT_VIEW_RANGE_LIMIT {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if ttl_ticks == 0 || ttl_ticks > MAX_CLIENT_VIEW_SELECTION_TTL_TICKS {
            return Err(ScriptDtoError::InvalidBounds);
        }
        if let Some(radius) = radius
            && (!radius.is_finite() || !(0.0..=MAX_CLIENT_VIEW_MARKER_RADIUS).contains(&radius))
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            dimension: dimension.to_owned(),
            range_limit,
            ttl_ticks,
            formation,
            radius,
        })
    }

    #[must_use]
    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    #[must_use]
    pub const fn range_limit(&self) -> u32 {
        self.range_limit
    }

    #[must_use]
    pub const fn ttl_ticks(&self) -> u64 {
        self.ttl_ticks
    }

    #[must_use]
    pub const fn formation(&self) -> Option<ScriptClientViewFormation> {
        self.formation
    }

    #[must_use]
    pub const fn radius(&self) -> Option<f64> {
        self.radius
    }
}

/// Validated `begin_client_selection(request_id, player_id, view_instance_id,
/// view_revision, action_id, constraints)` inputs after admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptClientSelectionBegin {
    request_id: String,
    player_id: ScriptPlayerId,
    view_instance_id: String,
    view_revision: u64,
    action_id: String,
    constraints: ScriptClientSelectionConstraints,
}

impl ScriptClientSelectionBegin {
    pub fn try_new(
        request_id: &str,
        player_id: ScriptPlayerId,
        view_instance_id: &str,
        view_revision: u64,
        action_id: &str,
        constraints: ScriptClientSelectionConstraints,
    ) -> Result<Self, ScriptDtoError> {
        if view_revision == 0 || view_revision > MAX_CLIENT_VIEW_EXACT_INTEGER {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            request_id: validate_script_id(request_id)?,
            player_id,
            view_instance_id: check_contract_resource_id_owned(view_instance_id)?,
            view_revision,
            action_id: check_contract_resource_id_owned(action_id)?,
            constraints,
        })
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    #[must_use]
    pub fn view_instance_id(&self) -> &str {
        &self.view_instance_id
    }

    #[must_use]
    pub const fn view_revision(&self) -> u64 {
        self.view_revision
    }

    #[must_use]
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    #[must_use]
    pub const fn constraints(&self) -> &ScriptClientSelectionConstraints {
        &self.constraints
    }
}

/// Validated `open_client_view(request_id, player_id, owned_view_id, model)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptClientViewOpen {
    request_id: String,
    player_id: ScriptPlayerId,
    owned_view_id: String,
    model: ScriptClientViewModel,
}

impl Eq for ScriptClientViewOpen {}

impl ScriptClientViewOpen {
    pub fn try_new(
        request_id: &str,
        player_id: ScriptPlayerId,
        owned_view_id: &str,
        model: ScriptClientViewModel,
    ) -> Result<Self, ScriptDtoError> {
        Ok(Self {
            request_id: validate_script_id(request_id)?,
            player_id,
            owned_view_id: check_contract_resource_id_owned(owned_view_id)?,
            model,
        })
    }

    #[must_use]
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    #[must_use]
    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    #[must_use]
    pub fn owned_view_id(&self) -> &str {
        &self.owned_view_id
    }

    #[must_use]
    pub const fn model(&self) -> &ScriptClientViewModel {
        &self.model
    }
}

/// Validated `present_client_view(player_id, view_instance_id, expected_revision, model)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptClientViewPresent {
    player_id: ScriptPlayerId,
    view_instance_id: String,
    expected_revision: u64,
    model: ScriptClientViewModel,
}

impl Eq for ScriptClientViewPresent {}

impl ScriptClientViewPresent {
    pub fn try_new(
        player_id: ScriptPlayerId,
        view_instance_id: &str,
        expected_revision: u64,
        model: ScriptClientViewModel,
    ) -> Result<Self, ScriptDtoError> {
        if expected_revision == 0 || expected_revision > MAX_CLIENT_VIEW_EXACT_INTEGER {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(Self {
            player_id,
            view_instance_id: check_contract_resource_id_owned(view_instance_id)?,
            expected_revision,
            model,
        })
    }

    #[must_use]
    pub const fn player_id(&self) -> ScriptPlayerId {
        self.player_id
    }

    #[must_use]
    pub fn view_instance_id(&self) -> &str {
        &self.view_instance_id
    }

    #[must_use]
    pub const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    #[must_use]
    pub const fn model(&self) -> &ScriptClientViewModel {
        &self.model
    }
}

fn check_contract_resource_id_owned(value: &str) -> Result<String, ScriptDtoError> {
    validate_contract_resource_id(value)
}

#[cfg(test)]
#[path = "client_view_tests.rs"]
mod client_view_tests;
