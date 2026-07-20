use super::identifier::Identifier;
use super::instance::{
    AttributeDefinition, AttributeId, AttributeInstance, AttributeInstanceError, AttributeModifier,
    DirtyEffect, MAX_MODIFIERS_PER_ATTRIBUTE, PackedAttribute,
};

pub const MAX_ATTRIBUTE_INSTANCES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierPersistence {
    Transient,
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateModifier {
    pub modifier: AttributeModifier,
    pub persistence: ModifierPersistence,
}

impl TemplateModifier {
    #[must_use]
    pub const fn transient(modifier: AttributeModifier) -> Self {
        Self {
            modifier,
            persistence: ModifierPersistence::Transient,
        }
    }

    #[must_use]
    pub const fn permanent(modifier: AttributeModifier) -> Self {
        Self {
            modifier,
            persistence: ModifierPersistence::Permanent,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AttributeTemplate<'a> {
    pub definition: AttributeDefinition,
    pub base_value: f64,
    pub modifiers: &'a [TemplateModifier],
}

impl<'a> AttributeTemplate<'a> {
    #[must_use]
    pub const fn new(
        definition: AttributeDefinition,
        base_value: f64,
        modifiers: &'a [TemplateModifier],
    ) -> Self {
        Self {
            definition,
            base_value,
            modifiers,
        }
    }

    #[must_use]
    pub const fn without_modifiers(definition: AttributeDefinition, base_value: f64) -> Self {
        Self::new(definition, base_value, &[])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeMapLimits {
    instance_capacity: usize,
    modifier_capacity: usize,
}

impl AttributeMapLimits {
    pub fn new(
        instance_capacity: usize,
        modifier_capacity: usize,
    ) -> Result<Self, AttributeMapInitError> {
        if instance_capacity > MAX_ATTRIBUTE_INSTANCES {
            return Err(AttributeMapInitError::InstanceCapacityExceedsHardLimit {
                requested: instance_capacity,
                maximum: MAX_ATTRIBUTE_INSTANCES,
            });
        }
        if modifier_capacity > MAX_MODIFIERS_PER_ATTRIBUTE {
            return Err(AttributeMapInitError::ModifierCapacityExceedsHardLimit {
                requested: modifier_capacity,
                maximum: MAX_MODIFIERS_PER_ATTRIBUTE,
            });
        }
        Ok(Self {
            instance_capacity,
            modifier_capacity,
        })
    }

    #[must_use]
    pub const fn instance_capacity(self) -> usize {
        self.instance_capacity
    }

    #[must_use]
    pub const fn modifier_capacity(self) -> usize {
        self.modifier_capacity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeMapInitError {
    InstanceCapacityExceedsHardLimit {
        requested: usize,
        maximum: usize,
    },
    ModifierCapacityExceedsHardLimit {
        requested: usize,
        maximum: usize,
    },
    TooManyTemplates {
        count: usize,
        maximum: usize,
    },
    Template {
        attribute: AttributeId,
        source: AttributeInstanceError,
    },
    AllocationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeMapError {
    UnknownAttribute {
        attribute: AttributeId,
    },
    UnknownModifier {
        attribute: AttributeId,
        id: Identifier,
    },
    InstanceCapacityExceeded {
        capacity: usize,
    },
    Instance {
        attribute: AttributeId,
        source: AttributeInstanceError,
    },
    AllocationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantiationOutcome {
    Created,
    Existing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyReport {
    pub applied: usize,
    pub ignored_unknown: usize,
}

#[derive(Debug)]
struct TemplateRecord {
    instance: AttributeInstance,
}

#[derive(Debug)]
pub struct AttributeMap {
    templates: Vec<TemplateRecord>,
    instances: Vec<AttributeInstance>,
    assignment_order: Vec<AttributeId>,
    pending_updates: Vec<AttributeId>,
    pending_syncs: Vec<AttributeId>,
    limits: AttributeMapLimits,
}

impl AttributeMap {
    pub fn try_new(
        templates: &[AttributeTemplate<'_>],
        limits: AttributeMapLimits,
    ) -> Result<Self, AttributeMapInitError> {
        if templates.len() > MAX_ATTRIBUTE_INSTANCES {
            return Err(AttributeMapInitError::TooManyTemplates {
                count: templates.len(),
                maximum: MAX_ATTRIBUTE_INSTANCES,
            });
        }

        let mut template_records = Vec::new();
        let mut instances = Vec::new();
        let mut assignment_order = Vec::new();
        let mut pending_updates = Vec::new();
        let mut pending_syncs = Vec::new();
        reserve_map_init(&mut template_records, templates.len())?;
        reserve_map_init(&mut instances, limits.instance_capacity)?;
        reserve_map_init(&mut assignment_order, limits.instance_capacity)?;
        reserve_map_init(&mut pending_updates, limits.instance_capacity)?;
        reserve_map_init(&mut pending_syncs, limits.instance_capacity)?;

        for template in templates {
            let attribute = template.definition.id();
            let location = locate_template(&template_records, attribute);
            let mut instance =
                AttributeInstance::try_new(template.definition, limits.modifier_capacity)
                    .map_err(|source| AttributeMapInitError::Template { attribute, source })?;
            instance.set_base_value(template.base_value);
            for template_modifier in template.modifiers {
                let result = match template_modifier.persistence {
                    ModifierPersistence::Transient => {
                        instance.add_transient_modifier(template_modifier.modifier.clone())
                    }
                    ModifierPersistence::Permanent => {
                        instance.add_permanent_modifier(template_modifier.modifier.clone())
                    }
                };
                result.map_err(|source| AttributeMapInitError::Template { attribute, source })?;
            }
            instance.value();
            match location {
                Ok(index) => template_records[index] = TemplateRecord { instance },
                Err(index) => template_records.insert(index, TemplateRecord { instance }),
            }
        }

        Ok(Self {
            templates: template_records,
            instances,
            assignment_order,
            pending_updates,
            pending_syncs,
            limits,
        })
    }

    #[must_use]
    pub fn has_attribute(&self, attribute: AttributeId) -> bool {
        self.template(attribute).is_some()
    }

    #[must_use]
    pub fn has_modifier(&self, attribute: AttributeId, id: &Identifier) -> bool {
        self.instance(attribute)
            .or_else(|| self.template(attribute))
            .is_some_and(|instance| instance.has_modifier(id))
    }

    pub fn get_value(&mut self, attribute: AttributeId) -> Result<f64, AttributeMapError> {
        if let Ok(index) = locate_instance(&self.instances, attribute) {
            return Ok(self.instances[index].value());
        }
        self.template(attribute)
            .map(AttributeInstance::cached_template_value)
            .ok_or(AttributeMapError::UnknownAttribute { attribute })
    }

    pub fn get_base_value(&self, attribute: AttributeId) -> Result<f64, AttributeMapError> {
        self.instance(attribute)
            .or_else(|| self.template(attribute))
            .map(AttributeInstance::base_value)
            .ok_or(AttributeMapError::UnknownAttribute { attribute })
    }

    pub fn get_modifier_value(
        &self,
        attribute: AttributeId,
        id: &Identifier,
    ) -> Result<f64, AttributeMapError> {
        let instance = self
            .instance(attribute)
            .or_else(|| self.template(attribute))
            .ok_or(AttributeMapError::UnknownAttribute { attribute })?;
        instance
            .modifier(id)
            .map(|modifier| modifier.amount())
            .ok_or_else(|| AttributeMapError::UnknownModifier {
                attribute,
                id: id.clone(),
            })
    }

    #[must_use]
    pub fn instantiated_len(&self) -> usize {
        self.instances.len()
    }

    #[must_use]
    pub fn instance(&self, attribute: AttributeId) -> Option<&AttributeInstance> {
        locate_instance(&self.instances, attribute)
            .ok()
            .map(|index| &self.instances[index])
    }

    pub fn ensure_instance(
        &mut self,
        attribute: AttributeId,
    ) -> Result<InstantiationOutcome, AttributeMapError> {
        if locate_instance(&self.instances, attribute).is_ok() {
            return Ok(InstantiationOutcome::Existing);
        }
        let template_index = locate_template(&self.templates, attribute)
            .map_err(|_| AttributeMapError::UnknownAttribute { attribute })?;
        if self.instances.len() == self.limits.instance_capacity {
            return Err(AttributeMapError::InstanceCapacityExceeded {
                capacity: self.limits.instance_capacity,
            });
        }

        let syncable = self.templates[template_index]
            .instance
            .definition()
            .is_syncable();
        let instance = self.templates[template_index]
            .instance
            .try_clone_with_capacity(self.limits.modifier_capacity)
            .map_err(|source| AttributeMapError::Instance { attribute, source })?;
        let insertion = locate_instance(&self.instances, attribute).unwrap_err();
        self.instances.insert(insertion, instance);
        self.assignment_order.push(attribute);
        self.publish(attribute, syncable);
        Ok(InstantiationOutcome::Created)
    }

    pub fn set_base_value(
        &mut self,
        attribute: AttributeId,
        base_value: f64,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (effect, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.set_base_value(base_value),
                instance.definition().is_syncable(),
            )
        };
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn add_transient_modifier(
        &mut self,
        attribute: AttributeId,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (result, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.add_transient_modifier(modifier),
                instance.definition().is_syncable(),
            )
        };
        let effect = result.map_err(|source| AttributeMapError::Instance { attribute, source })?;
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn add_or_update_transient_modifier(
        &mut self,
        attribute: AttributeId,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (result, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.add_or_update_transient_modifier(modifier),
                instance.definition().is_syncable(),
            )
        };
        let effect = result.map_err(|source| AttributeMapError::Instance { attribute, source })?;
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    /// Models `AttributeMap.addTransientAttributeModifiers`: remove by identifier,
    /// then add the incoming value as transient.
    pub fn replace_transient_modifier(
        &mut self,
        attribute: AttributeId,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (result, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.replace_transient_modifier(modifier),
                instance.definition().is_syncable(),
            )
        };
        let effect = result.map_err(|source| AttributeMapError::Instance { attribute, source })?;
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn add_permanent_modifier(
        &mut self,
        attribute: AttributeId,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (result, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.add_permanent_modifier(modifier),
                instance.definition().is_syncable(),
            )
        };
        let effect = result.map_err(|source| AttributeMapError::Instance { attribute, source })?;
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn add_or_replace_permanent_modifier(
        &mut self,
        attribute: AttributeId,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        self.ensure_instance(attribute)?;
        let (result, syncable) = {
            let instance = self.instance_mut_existing(attribute);
            (
                instance.add_or_replace_permanent_modifier(modifier),
                instance.definition().is_syncable(),
            )
        };
        let effect = result.map_err(|source| AttributeMapError::Instance { attribute, source })?;
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn remove_modifier(
        &mut self,
        attribute: AttributeId,
        id: &Identifier,
    ) -> Result<DirtyEffect, AttributeMapError> {
        if !self.has_attribute(attribute) {
            return Err(AttributeMapError::UnknownAttribute { attribute });
        }
        let Ok(index) = locate_instance(&self.instances, attribute) else {
            return Ok(DirtyEffect::NONE);
        };
        let syncable = self.instances[index].definition().is_syncable();
        let effect = self.instances[index].remove_modifier(id);
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    pub fn remove_all_modifiers(
        &mut self,
        attribute: AttributeId,
    ) -> Result<DirtyEffect, AttributeMapError> {
        if !self.has_attribute(attribute) {
            return Err(AttributeMapError::UnknownAttribute { attribute });
        }
        let Ok(index) = locate_instance(&self.instances, attribute) else {
            return Ok(DirtyEffect::NONE);
        };
        let syncable = self.instances[index].definition().is_syncable();
        let effect = self.instances[index].remove_all_modifiers();
        self.publish_if_changed(attribute, syncable, effect);
        Ok(effect)
    }

    #[must_use]
    pub fn pending_updates(&self) -> &[AttributeId] {
        &self.pending_updates
    }

    #[must_use]
    pub fn pending_syncs(&self) -> &[AttributeId] {
        &self.pending_syncs
    }

    pub fn clear_pending_updates(&mut self) {
        self.pending_updates.clear();
    }

    pub fn clear_pending_syncs(&mut self) {
        self.pending_syncs.clear();
    }

    pub fn syncable_instances(&self) -> impl Iterator<Item = AttributeId> + '_ {
        self.instances.iter().filter_map(|instance| {
            instance
                .definition()
                .is_syncable()
                .then_some(instance.attribute())
        })
    }

    pub fn reset_base_value(&mut self, attribute: AttributeId) -> bool {
        let Some(template_base) = self.template(attribute).map(AttributeInstance::base_value)
        else {
            return false;
        };
        let Ok(index) = locate_instance(&self.instances, attribute) else {
            return true;
        };
        let syncable = self.instances[index].definition().is_syncable();
        let effect = self.instances[index].set_base_value(template_base);
        self.publish_if_changed(attribute, syncable, effect);
        true
    }

    pub fn try_pack(&self) -> Result<Vec<PackedAttribute>, AttributeMapError> {
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(self.instances.len())
            .map_err(|_| AttributeMapError::AllocationFailed)?;
        for instance in &self.instances {
            packed.push(
                instance
                    .try_pack()
                    .map_err(|source| AttributeMapError::Instance {
                        attribute: instance.attribute(),
                        source,
                    })?,
            );
        }
        Ok(packed)
    }

    pub fn apply_packed(
        &mut self,
        packed_attributes: &[PackedAttribute],
    ) -> Result<ApplyReport, AttributeMapError> {
        let mut report = ApplyReport {
            applied: 0,
            ignored_unknown: 0,
        };
        for packed in packed_attributes {
            if !self.has_attribute(packed.attribute) {
                report.ignored_unknown += 1;
                continue;
            }
            let prospective = self
                .instance(packed.attribute)
                .or_else(|| self.template(packed.attribute))
                .expect("supported attribute has a template");
            prospective
                .preflight_packed(packed)
                .map_err(|source| AttributeMapError::Instance {
                    attribute: packed.attribute,
                    source,
                })?;
            self.ensure_instance(packed.attribute)?;
            let (effect, syncable) = {
                let instance = self.instance_mut_existing(packed.attribute);
                let syncable = instance.definition().is_syncable();
                let effect = instance
                    .apply_packed(packed)
                    .expect("packed record was preflighted");
                (effect, syncable)
            };
            self.publish_if_changed(packed.attribute, syncable, effect);
            report.applied += 1;
        }
        Ok(report)
    }

    pub fn assign_all_values(&mut self, other: &Self) -> Result<(), AttributeMapError> {
        self.preflight_assignment_slots(other)?;
        for &attribute in &other.assignment_order {
            let source = other
                .instance(attribute)
                .expect("assignment order only contains materialized attributes");
            let Some(target) = self
                .instance(attribute)
                .or_else(|| self.template(attribute))
            else {
                continue;
            };
            target
                .preflight_copy(source)
                .map_err(|source| AttributeMapError::Instance { attribute, source })?;
        }
        for &attribute in &other.assignment_order {
            let source = other
                .instance(attribute)
                .expect("assignment order only contains materialized attributes");
            if !self.has_attribute(attribute) {
                continue;
            }
            self.ensure_instance(attribute)?;
            let syncable = self
                .instance(attribute)
                .expect("instance was ensured")
                .definition()
                .is_syncable();
            let effect = self
                .instance_mut_existing(attribute)
                .replace_from(source)
                .expect("source instance was preflighted");
            self.publish_if_changed(attribute, syncable, effect);
        }
        Ok(())
    }

    pub fn assign_base_values(&mut self, other: &Self) -> Result<(), AttributeMapError> {
        self.preflight_assignment_slots(other)?;
        for &attribute in &other.assignment_order {
            let source = other
                .instance(attribute)
                .expect("assignment order only contains materialized attributes");
            if !self.has_attribute(attribute) {
                continue;
            }
            self.ensure_instance(attribute)?;
            let (effect, syncable) = {
                let target = self.instance_mut_existing(attribute);
                (
                    target.set_base_value(source.base_value()),
                    target.definition().is_syncable(),
                )
            };
            self.publish_if_changed(attribute, syncable, effect);
        }
        Ok(())
    }

    pub fn assign_permanent_modifiers(&mut self, other: &Self) -> Result<(), AttributeMapError> {
        for &attribute in &other.assignment_order {
            let source = other
                .instance(attribute)
                .expect("assignment order only contains materialized attributes");
            if !self.has_attribute(attribute) {
                continue;
            }
            self.ensure_instance(attribute)?;
            let syncable = self
                .instance(attribute)
                .expect("instance was ensured")
                .definition()
                .is_syncable();
            for modifier in source.permanent_slice() {
                let effect = self
                    .instance_mut_existing(attribute)
                    .add_permanent_modifier(modifier.clone())
                    .map_err(|source| AttributeMapError::Instance { attribute, source })?;
                self.publish_if_changed(attribute, syncable, effect);
            }
        }
        Ok(())
    }

    fn template(&self, attribute: AttributeId) -> Option<&AttributeInstance> {
        locate_template(&self.templates, attribute)
            .ok()
            .map(|index| &self.templates[index].instance)
    }

    fn instance_mut_existing(&mut self, attribute: AttributeId) -> &mut AttributeInstance {
        let index = locate_instance(&self.instances, attribute)
            .expect("attribute instance must have been ensured");
        &mut self.instances[index]
    }

    fn publish_if_changed(&mut self, attribute: AttributeId, syncable: bool, effect: DirtyEffect) {
        if effect.changed() {
            self.publish(attribute, syncable);
        }
    }

    fn publish(&mut self, attribute: AttributeId, syncable: bool) {
        insert_id(&mut self.pending_updates, attribute);
        if syncable {
            insert_id(&mut self.pending_syncs, attribute);
        }
    }

    fn preflight_assignment_slots(&self, other: &Self) -> Result<(), AttributeMapError> {
        let required = other
            .assignment_order
            .iter()
            .filter(|&&attribute| {
                self.has_attribute(attribute) && self.instance(attribute).is_none()
            })
            .count();
        if self.instances.len() + required > self.limits.instance_capacity {
            return Err(AttributeMapError::InstanceCapacityExceeded {
                capacity: self.limits.instance_capacity,
            });
        }
        Ok(())
    }
}

fn locate_template(values: &[TemplateRecord], attribute: AttributeId) -> Result<usize, usize> {
    values.binary_search_by_key(&attribute, |record| record.instance.attribute())
}

fn locate_instance(values: &[AttributeInstance], attribute: AttributeId) -> Result<usize, usize> {
    values.binary_search_by_key(&attribute, AttributeInstance::attribute)
}

fn insert_id(values: &mut Vec<AttributeId>, attribute: AttributeId) {
    if let Err(index) = values.binary_search(&attribute) {
        values.insert(index, attribute);
    }
}

fn reserve_map_init<T>(values: &mut Vec<T>, capacity: usize) -> Result<(), AttributeMapInitError> {
    values
        .try_reserve_exact(capacity)
        .map_err(|_| AttributeMapInitError::AllocationFailed)
}
