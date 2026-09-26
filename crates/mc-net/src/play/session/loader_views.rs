//! Server-side schema-2 view instances and world-selection contexts.
//!
//! Every instance is bound to the exact live player session, the owning plugin,
//! the verified view definition, the action whitelist and the typed field schema
//! of the currently presented model. A present is a CAS replacement that drops
//! every prior selection context; an action is admitted only after the registry
//! re-reads the instance, its revision, the plugin's granted permissions and the
//! presented field policy. Nothing here trusts a client-supplied id, price,
//! quantity or actor.

use std::collections::{BTreeMap, BTreeSet};

use mc_script::{
    ScriptClientSelectionConstraints, ScriptClientViewFieldValue, ScriptClientViewModel,
    ScriptClientViewOpen, ScriptClientViewPresent,
};

/// How many retired instance ids are remembered to distinguish `ClosedInstance`
/// from `UnknownInstance` without unbounded growth.
pub(crate) const MAX_RETIRED_VIEW_INSTANCES: usize = 256;

/// Why a view lifecycle or action request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoaderViewError {
    /// No such instance was ever opened.
    UnknownInstance,
    /// The instance was closed; delivery into it is refused.
    ClosedInstance,
    /// The caller's revision is not the live revision.
    StaleRevision,
    /// The caller does not own the instance.
    ForeignOwner,
    /// The action is not declared by the presented model.
    UnknownAction,
    /// The action is declared but disabled in the presented model.
    DisabledAction,
    /// An input field is not part of the presented typed field schema.
    SubstitutedField,
    /// The action binds a world point that no live selection context supplies.
    SelectionRequired,
    /// The armed selection context expired.
    SelectionExpired,
    /// The owner's permission pair was revoked; the server re-read it per action.
    PermissionRevoked,
    /// A submission carried more fields than the frozen bound allows.
    TooLarge,
}

/// Server-derived proof attached to one accepted world point.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectionAdmission {
    pub(crate) context_id: String,
    pub(crate) action_id: String,
    pub(crate) dimension: String,
    pub(crate) range_limit: u32,
    pub(crate) expires_at_tick: u64,
    /// Target derived from the client's typed fields after field validation.
    pub(crate) target: Option<[f64; 3]>,
}

// Positions are validated finite, so structural equality is an equivalence.
impl Eq for SelectionAdmission {}

/// One instance as the registry holds it.
#[derive(Debug, Clone)]
pub(crate) struct LoaderViewInstance {
    instance_id: String,
    revision: u64,
    player_id: u64,
    owner: String,
    view_id: String,
    title: String,
    model: ScriptClientViewModel,
    enabled_actions: BTreeSet<String>,
    field_values: BTreeMap<String, FieldKind>,
    last_action_sequence: u64,
}

impl LoaderViewInstance {
    #[must_use]
    pub(crate) fn instance_id(&self) -> &str {
        &self.instance_id
    }

    #[must_use]
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub(crate) const fn model(&self) -> &ScriptClientViewModel {
        &self.model
    }

    /// True when the presented model binds this action to a server world point.
    #[must_use]
    pub(crate) fn requires_selection(&self, action_id: &str) -> bool {
        self.model.markers().iter().any(|marker| {
            marker.selection_token().is_some() && marker.action_id() == Some(action_id)
        })
    }
}

/// The typed kind of one presented field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Unknown,
    Number,
    Text,
    Selected,
}

#[derive(Debug, Clone)]
struct SelectionContext {
    context_id: String,
    player_id: u64,
    owner: String,
    instance_id: String,
    revision: u64,
    action_id: String,
    constraints: ScriptClientSelectionConstraints,
    expires_at_tick: u64,
    consumed: bool,
    admission: Option<SelectionAdmission>,
}

/// Outcome of one admitted client action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionOutcome {
    /// False when this is a replay of an already-consumed point or sequence.
    pub(crate) deliver: bool,
    /// The same admission result a consumed context keeps returning.
    pub(crate) admission: Option<SelectionAdmission>,
}

/// One opened view instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenedView {
    pub(crate) instance_id: String,
    pub(crate) revision: u64,
}

/// One issued world-selection context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartedSelection {
    pub(crate) context_id: String,
    pub(crate) expires_at_tick: u64,
}

/// Client-supplied action ingress, already decoded from wire 3.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct IngressAction {
    pub(crate) view_instance_id: String,
    pub(crate) view_revision: u64,
    pub(crate) action_id: String,
    pub(crate) action_sequence: u64,
    pub(crate) fields: Vec<mc_script::ScriptClientViewField>,
    pub(crate) selection_token: Option<String>,
}

#[derive(Debug)]
pub(crate) struct LoaderViewRegistry {
    views: BTreeMap<String, LoaderViewInstance>,
    selections: BTreeMap<String, SelectionContext>,
    retired: BTreeSet<String>,
    declared_kinds: BTreeMap<mc_script::ScriptClientViewRequestKind, BTreeSet<String>>,
    next_instance: u64,
    next_context: u64,
}

impl Default for LoaderViewRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LoaderViewRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            views: BTreeMap::new(),
            selections: BTreeMap::new(),
            retired: BTreeSet::new(),
            declared_kinds: BTreeMap::new(),
            next_instance: 0,
            next_context: 0,
        }
    }

    /// Record that `owner` declared a view of `kind`; the host populates this
    /// from each loaded plugin's verified bundle declaration.
    #[allow(dead_code)]
    pub(crate) fn declare_owner_kind(
        &mut self,
        owner: &str,
        kind: mc_script::ScriptClientViewRequestKind,
    ) {
        self.declared_kinds
            .entry(kind)
            .or_default()
            .insert(owner.to_owned());
    }

    /// The single owner declaring `kind`, or none when zero or several do.
    #[must_use]
    pub(crate) fn owner_for_kind(
        &self,
        kind: mc_script::ScriptClientViewRequestKind,
    ) -> Option<String> {
        let owners = self.declared_kinds.get(&kind)?;
        if owners.len() == 1 {
            owners.iter().next().cloned()
        } else {
            None
        }
    }

    #[must_use]
    pub(crate) fn instance(&self, instance_id: &str) -> Option<&LoaderViewInstance> {
        self.views.get(instance_id)
    }

    /// Register a fresh instance bound to the plugin owner, the verified view
    /// definition and the presented model's action/field policy.
    pub(crate) fn open(
        &mut self,
        owner: &str,
        request: &ScriptClientViewOpen,
    ) -> Result<OpenedView, LoaderViewError> {
        self.next_instance += 1;
        let instance_id = format!("solaris:view-{}", self.next_instance);
        let model = request.model().clone();
        let instance = build_instance(
            instance_id.clone(),
            1,
            request.player_id().value(),
            owner,
            request.owned_view_id(),
            request.owned_view_id(),
            model,
        );
        self.retired.remove(&instance_id);
        self.views.insert(instance_id.clone(), instance);
        Ok(OpenedView {
            instance_id,
            revision: 1,
        })
    }

    /// CAS-replace the model and revision, dropping every prior selection
    /// context of the instance and re-arming only the tokens the new model
    /// explicitly presents.
    pub(crate) fn present(
        &mut self,
        owner: &str,
        request: &ScriptClientViewPresent,
        now_tick: u64,
    ) -> Result<u64, LoaderViewError> {
        let instance = self
            .views
            .get(request.view_instance_id())
            .ok_or_else(|| instance_error(&self.retired, request.view_instance_id()))?;
        if instance.owner != owner {
            return Err(LoaderViewError::ForeignOwner);
        }
        if instance.player_id != request.player_id().value() {
            return Err(LoaderViewError::ForeignOwner);
        }
        if instance.revision != request.expected_revision() {
            return Err(LoaderViewError::StaleRevision);
        }
        let revision = instance.revision + 1;
        let model = request.model().clone();
        let armed = self.armed_contexts(owner, instance, &model, now_tick)?;
        // A replacement invalidates every prior action binding and context.
        self.selections
            .retain(|_, context| context.instance_id != instance.instance_id);
        let replacement = build_instance(
            instance.instance_id.clone(),
            revision,
            instance.player_id,
            owner,
            &instance.view_id,
            &instance.title,
            model,
        );
        self.views.insert(instance.instance_id.clone(), replacement);
        for mut context in armed {
            context.revision = revision;
            self.selections.insert(context.context_id.clone(), context);
        }
        Ok(revision)
    }

    /// Close an instance from either side; delivery into it is refused after.
    pub(crate) fn close(
        &mut self,
        owner: Option<&str>,
        player_id: u64,
        instance_id: &str,
    ) -> Result<(), LoaderViewError> {
        let instance = self
            .views
            .get(instance_id)
            .ok_or_else(|| instance_error(&self.retired, instance_id))?;
        if instance.player_id != player_id {
            return Err(LoaderViewError::ForeignOwner);
        }
        if let Some(owner) = owner
            && instance.owner != owner
        {
            return Err(LoaderViewError::ForeignOwner);
        }
        let instance = self.views.remove(instance_id).expect("checked above");
        self.selections
            .retain(|_, context| context.instance_id != instance_id);
        self.retired.insert(instance_id.to_owned());
        while self.retired.len() > MAX_RETIRED_VIEW_INSTANCES {
            let oldest = self.retired.iter().next().cloned().unwrap();
            self.retired.remove(&oldest);
        }
        drop(instance);
        Ok(())
    }

    /// Issue an opaque context bound to the exact session, owner, instance,
    /// revision, action and constraints, with a simulation-tick expiry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin_selection(
        &mut self,
        owner: &str,
        player_id: u64,
        instance_id: &str,
        view_revision: u64,
        action_id: &str,
        constraints: &ScriptClientSelectionConstraints,
        now_tick: u64,
    ) -> Result<StartedSelection, LoaderViewError> {
        let instance = self
            .views
            .get(instance_id)
            .ok_or_else(|| instance_error(&self.retired, instance_id))?;
        if instance.owner != owner || instance.player_id != player_id {
            return Err(LoaderViewError::ForeignOwner);
        }
        if instance.revision != view_revision {
            return Err(LoaderViewError::StaleRevision);
        }
        if !instance.enabled_actions.contains(action_id) {
            return Err(LoaderViewError::UnknownAction);
        }
        self.next_context += 1;
        let context_id = format!("solaris:selection-{}", self.next_context);
        let expires_at_tick = now_tick.saturating_add(constraints.ttl_ticks());
        self.selections.insert(
            context_id.clone(),
            SelectionContext {
                context_id: context_id.clone(),
                player_id,
                owner: owner.to_owned(),
                instance_id: instance_id.to_owned(),
                revision: view_revision,
                action_id: action_id.to_owned(),
                constraints: constraints.clone(),
                expires_at_tick,
                consumed: false,
                admission: None,
            },
        );
        Ok(StartedSelection {
            context_id,
            expires_at_tick,
        })
    }

    /// Cancel a live selection context; unknown ids are refused.
    pub(crate) fn cancel_selection(
        &mut self,
        player_id: u64,
        context_id: &str,
    ) -> Result<(), LoaderViewError> {
        match self.selections.get(context_id) {
            Some(context) if context.player_id == player_id => {
                self.selections.remove(context_id);
                Ok(())
            }
            Some(_) => Err(LoaderViewError::ForeignOwner),
            None => Err(LoaderViewError::UnknownInstance),
        }
    }

    /// Drop every view and context owned by one plugin (permission revocation).
    #[allow(dead_code)]
    pub(crate) fn revoke_owner(&mut self, owner: &str) {
        let instances = self
            .views
            .values()
            .filter(|instance| instance.owner == owner)
            .map(|instance| instance.instance_id.clone())
            .collect::<Vec<_>>();
        for instance_id in instances {
            self.views.remove(&instance_id);
            self.retired.insert(instance_id.clone());
        }
        self.selections.retain(|_, context| context.owner != owner);
    }

    /// Drop every view and context of one disconnecting session.
    pub(crate) fn disconnect(&mut self, player_id: u64) {
        let instances = self
            .views
            .values()
            .filter(|instance| instance.player_id == player_id)
            .map(|instance| instance.instance_id.clone())
            .collect::<Vec<_>>();
        for instance_id in instances {
            self.views.remove(&instance_id);
            self.retired.insert(instance_id);
        }
        self.selections
            .retain(|_, context| context.player_id != player_id);
    }

    /// Admit one client action after re-reading the instance, its revision, the
    /// owner's granted permissions, the presented field policy and any echoed
    /// selection context.
    pub(crate) fn handle_action(
        &mut self,
        player_id: u64,
        action: &IngressAction,
        owner_grants: bool,
        now_tick: u64,
    ) -> Result<ActionOutcome, LoaderViewError> {
        if !owner_grants {
            // The right is re-read from the live manifest on every action.
            return Err(LoaderViewError::PermissionRevoked);
        }
        let instance = self
            .views
            .get(action.view_instance_id.as_str())
            .ok_or_else(|| instance_error(&self.retired, &action.view_instance_id))?;
        if instance.player_id != player_id {
            return Err(LoaderViewError::ForeignOwner);
        }
        if instance.revision != action.view_revision {
            return Err(LoaderViewError::StaleRevision);
        }
        if !instance
            .model
            .actions()
            .iter()
            .any(|declared| declared.action_id() == action.action_id)
        {
            return Err(LoaderViewError::UnknownAction);
        }
        if !instance.enabled_actions.contains(&action.action_id) {
            return Err(LoaderViewError::DisabledAction);
        }
        if action.fields.len() > mc_script::MAX_CLIENT_VIEW_FIELDS {
            return Err(LoaderViewError::TooLarge);
        }
        for field in &action.fields {
            let expected = instance
                .field_values
                .get(field.id())
                .ok_or(LoaderViewError::SubstitutedField)?;
            let actual = match field.value() {
                ScriptClientViewFieldValue::Number(_) => FieldKind::Number,
                ScriptClientViewFieldValue::Text(_) => FieldKind::Text,
                ScriptClientViewFieldValue::Selected(_) => FieldKind::Selected,
                // The DTO is `#[non_exhaustive]`: an unknown field kind can
                // never match what this instance declared.
                _ => return Err(LoaderViewError::SubstitutedField),
            };
            if *expected != actual {
                return Err(LoaderViewError::SubstitutedField);
            }
        }
        let instance_id = instance.instance_id.clone();
        let requires_selection = instance.requires_selection(&action.action_id);
        let last_sequence = instance.last_action_sequence;
        let Some(token) = action.selection_token.as_deref() else {
            if requires_selection {
                return Err(LoaderViewError::SelectionRequired);
            }
            if action.action_sequence <= last_sequence {
                return Ok(ActionOutcome {
                    deliver: false,
                    admission: None,
                });
            }
            self.record_sequence(&instance_id, action.action_sequence);
            return Ok(ActionOutcome {
                deliver: true,
                admission: None,
            });
        };
        let context = self
            .selections
            .get(token)
            .ok_or(LoaderViewError::SelectionRequired)?;
        if context.player_id != player_id
            || context.instance_id != instance_id
            || context.action_id != action.action_id
        {
            return Err(LoaderViewError::SelectionRequired);
        }
        if context.consumed {
            // Re-sending a consumed context returns the same admission result
            // with no new event and no duplicated effect.
            return Ok(ActionOutcome {
                deliver: false,
                admission: context.admission.clone(),
            });
        }
        if action.action_sequence <= last_sequence {
            return Ok(ActionOutcome {
                deliver: false,
                admission: None,
            });
        }
        if now_tick > context.expires_at_tick {
            return Err(LoaderViewError::SelectionExpired);
        }
        let admission = SelectionAdmission {
            context_id: token.to_owned(),
            action_id: action.action_id.clone(),
            dimension: context.constraints.dimension().to_owned(),
            range_limit: context.constraints.range_limit(),
            expires_at_tick: context.expires_at_tick,
            target: derive_target(&action.fields),
        };
        let context = self
            .selections
            .get_mut(token)
            .expect("selection context was checked above");
        context.consumed = true;
        context.admission = Some(admission.clone());
        self.record_sequence(&instance_id, action.action_sequence);
        Ok(ActionOutcome {
            deliver: true,
            admission: Some(admission),
        })
    }

    fn record_sequence(&mut self, instance_id: &str, action_sequence: u64) {
        if let Some(instance) = self.views.get_mut(instance_id) {
            instance.last_action_sequence = instance.last_action_sequence.max(action_sequence);
        }
    }

    fn armed_contexts(
        &self,
        owner: &str,
        instance: &LoaderViewInstance,
        model: &ScriptClientViewModel,
        now_tick: u64,
    ) -> Result<Vec<SelectionContext>, LoaderViewError> {
        let mut armed = Vec::new();
        for marker in model.markers() {
            let Some(token) = marker.selection_token() else {
                continue;
            };
            let context = self
                .selections
                .get(token)
                .ok_or(LoaderViewError::SelectionRequired)?;
            if context.player_id != instance.player_id
                || context.owner != owner
                || context.instance_id != instance.instance_id
                || Some(context.action_id.as_str()) != marker.action_id()
                || context.consumed
                || now_tick > context.expires_at_tick
            {
                return Err(LoaderViewError::SelectionRequired);
            }
            armed.push(context.clone());
        }
        Ok(armed)
    }
}

fn instance_error(retired: &BTreeSet<String>, instance_id: &str) -> LoaderViewError {
    if retired.contains(instance_id) {
        LoaderViewError::ClosedInstance
    } else {
        LoaderViewError::UnknownInstance
    }
}

fn build_instance(
    instance_id: String,
    revision: u64,
    player_id: u64,
    owner: &str,
    view_id: &str,
    title: &str,
    model: ScriptClientViewModel,
) -> LoaderViewInstance {
    let enabled_actions = model
        .actions()
        .iter()
        .filter(|action| action.enabled())
        .map(|action| action.action_id().to_owned())
        .collect();
    let field_values = model
        .fields()
        .iter()
        .map(|field| {
            let kind = match field.value() {
                ScriptClientViewFieldValue::Number(_) => FieldKind::Number,
                ScriptClientViewFieldValue::Text(_) => FieldKind::Text,
                ScriptClientViewFieldValue::Selected(_) => FieldKind::Selected,
                // The DTO is `#[non_exhaustive]`: an unknown kind cannot be a
                // declared model field.
                _ => FieldKind::Unknown,
            };
            (field.id().to_owned(), kind)
        })
        .collect();
    LoaderViewInstance {
        instance_id,
        revision,
        player_id,
        owner: owner.to_owned(),
        view_id: view_id.to_owned(),
        title: title.to_owned(),
        model,
        enabled_actions,
        field_values,
        last_action_sequence: 0,
    }
}

fn derive_target(fields: &[mc_script::ScriptClientViewField]) -> Option<[f64; 3]> {
    let mut coordinates = [None; 3];
    for field in fields {
        let axis = match field.id() {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => continue,
        };
        if let ScriptClientViewFieldValue::Number(number) = field.value() {
            coordinates[axis] = Some(*number);
        }
    }
    match coordinates {
        [Some(x), Some(y), Some(z)] => Some([x, y, z]),
        _ => None,
    }
}

#[cfg(test)]
#[path = "loader_views_tests.rs"]
mod loader_views_tests;
