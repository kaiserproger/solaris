use super::super::context::LootRandom;
use super::model::*;
use crate::Identifier;

impl EntityLootCatalog {
    /// Evaluates one compiled entity root with the stream bound by its context.
    pub fn evaluate(
        &self,
        table: &Identifier,
        context: &EntityLootContext<'_>,
    ) -> Result<Vec<EntityLootStack>, EntityLootEvaluationError> {
        if !self.tables.contains_key(table) {
            return Err(EntityLootEvaluationError::UnknownTable {
                table: table.clone(),
            });
        }
        if !self.roots.contains(table) {
            return Err(EntityLootEvaluationError::NotConfiguredRoot {
                table: table.clone(),
            });
        }
        let expected_sequence = self.random_sequence(table);
        let actual_sequence = context.random_binding().sequence();
        if expected_sequence != actual_sequence {
            return Err(EntityLootEvaluationError::RandomSequenceMismatch {
                expected: expected_sequence.cloned(),
                actual: actual_sequence.cloned(),
            });
        }
        if !context.luck.is_finite() {
            return Err(EntityLootEvaluationError::InvalidContext {
                message: "luck must be finite".to_string(),
            });
        }
        if context
            .origin
            .iter()
            .any(|coordinate| !coordinate.is_finite())
        {
            return Err(EntityLootEvaluationError::InvalidContext {
                message: "origin must be finite".to_string(),
            });
        }
        validate_context_collections(context)?;

        let mut evaluator = Evaluator {
            catalog: self,
            context,
            random: context.random_binding().random(),
            output: Vec::new(),
            output_items: 0,
            operations: 0,
        };
        let mut decorators = Vec::new();
        evaluator.evaluate_table(table, &mut decorators, 0)?;
        Ok(evaluator.output)
    }
}

struct Evaluator<'catalog, 'context, 'lookup> {
    catalog: &'catalog EntityLootCatalog,
    context: &'context EntityLootContext<'lookup>,
    random: LootRandom,
    output: Vec<EntityLootStack>,
    output_items: u64,
    operations: usize,
}

impl<'catalog> Evaluator<'catalog, '_, '_> {
    fn evaluate_table(
        &mut self,
        table_id: &Identifier,
        decorators: &mut Vec<&'catalog [LootFunction]>,
        depth: usize,
    ) -> Result<(), EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        let table = self.catalog.tables.get(table_id).ok_or_else(|| {
            EntityLootEvaluationError::CatalogInvariant {
                message: format!("validated reference {table_id} disappeared"),
            }
        })?;

        decorators.push(&table.functions);
        let result = table
            .pools
            .iter()
            .try_for_each(|pool| self.evaluate_pool(pool, decorators, self.next_depth(depth)?));
        decorators.pop();
        result
    }

    fn evaluate_pool(
        &mut self,
        pool: &'catalog LootPool,
        decorators: &mut Vec<&'catalog [LootFunction]>,
        depth: usize,
    ) -> Result<(), EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        if !self.conditions_match(&pool.conditions, self.next_depth(depth)?)? {
            return Ok(());
        }

        let rolls = i64::from(self.sample_int(&pool.rolls)?);
        let bonus = self.sample_float(&pool.bonus_rolls)? * f64::from(self.context.luck);
        let bonus = finite_floor_i64(bonus, "calculating bonus rolls")?;
        let roll_count =
            rolls
                .checked_add(bonus)
                .ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                    operation: "adding pool rolls",
                })?;
        if roll_count <= 0 {
            return Ok(());
        }
        if roll_count > i64::from(MAX_POOL_ROLLS) {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::PoolRolls,
                u64::try_from(roll_count).unwrap_or(u64::MAX),
            ));
        }

        decorators.push(&pool.functions);
        let result = (0..roll_count).try_for_each(|_| {
            self.charge(1)?;
            let mut candidates = Vec::new();
            for entry in &pool.entries {
                self.expand_entry(entry, &mut candidates, self.next_depth(depth)?)?;
            }
            let Some(selected) = self.choose_candidate(&candidates)? else {
                return Ok(());
            };
            let candidate = candidates.get(selected).cloned().ok_or_else(|| {
                EntityLootEvaluationError::CatalogInvariant {
                    message: "weighted candidate index was outside its bounded list".to_string(),
                }
            })?;
            self.create_candidate(&candidate, decorators, self.next_depth(depth)?)
        });
        decorators.pop();
        result
    }

    fn expand_entry(
        &mut self,
        entry: &'catalog LootEntry,
        output: &mut Vec<Candidate<'catalog>>,
        depth: usize,
    ) -> Result<bool, EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        match entry {
            LootEntry::Alternatives {
                conditions,
                children,
            } => {
                if !self.conditions_match(conditions, self.next_depth(depth)?)? {
                    return Ok(false);
                }
                for child in children {
                    if self.expand_entry(child, output, self.next_depth(depth)?)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            LootEntry::Tag {
                singleton,
                tag,
                expand: true,
            } => {
                if !self.conditions_match(&singleton.conditions, self.next_depth(depth)?)? {
                    return Ok(false);
                }
                let items = self.context.tags.item_tag(tag).ok_or_else(|| {
                    EntityLootEvaluationError::MissingItemTag { tag: tag.clone() }
                })?;
                self.check_tag_expansion(items.len())?;
                for item in items {
                    self.charge(1)?;
                    self.push_candidate(
                        output,
                        Candidate::TagItem {
                            singleton,
                            item: item.clone(),
                        },
                    )?;
                }
                Ok(true)
            }
            _ => {
                let singleton = entry.singleton().ok_or_else(|| {
                    EntityLootEvaluationError::CatalogInvariant {
                        message: "non-alternative entry had no singleton metadata".to_string(),
                    }
                })?;
                if !self.conditions_match(&singleton.conditions, self.next_depth(depth)?)? {
                    return Ok(false);
                }
                self.push_candidate(output, Candidate::Entry(entry))?;
                Ok(true)
            }
        }
    }

    fn push_candidate(
        &mut self,
        output: &mut Vec<Candidate<'catalog>>,
        candidate: Candidate<'catalog>,
    ) -> Result<(), EntityLootEvaluationError> {
        self.charge(1)?;
        let actual =
            output
                .len()
                .checked_add(1)
                .ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                    operation: "counting loot candidates",
                })?;
        if actual > MAX_CANDIDATES_PER_ROLL {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::CandidatesPerRoll,
                actual as u64,
            ));
        }
        output.push(candidate);
        Ok(())
    }

    fn choose_candidate(
        &mut self,
        candidates: &[Candidate<'catalog>],
    ) -> Result<Option<usize>, EntityLootEvaluationError> {
        let mut total_weight = 0_i32;
        let mut positive_count = 0_usize;
        let mut sole_positive = None;
        let mut weights = Vec::with_capacity(candidates.len());
        for (index, candidate) in candidates.iter().enumerate() {
            self.charge(1)?;
            let singleton = candidate.singleton()?;
            let weighted = f64::from(singleton.weight)
                + f64::from(singleton.quality) * f64::from(self.context.luck);
            let weight = finite_floor_i64(weighted, "calculating an entry weight")?.max(0);
            let weight = i32::try_from(weight).map_err(|_| {
                EntityLootEvaluationError::ArithmeticOverflow {
                    operation: "converting an entry weight",
                }
            })?;
            total_weight = total_weight.checked_add(weight).ok_or(
                EntityLootEvaluationError::ArithmeticOverflow {
                    operation: "adding entry weights",
                },
            )?;
            if weight > 0 {
                positive_count = positive_count.checked_add(1).ok_or(
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "counting positive loot weights",
                    },
                )?;
                sole_positive = Some(index);
            }
            weights.push(weight);
        }
        match positive_count {
            0 => return Ok(None),
            1 => return Ok(sole_positive),
            _ => {}
        }

        let bound = u64::try_from(total_weight).map_err(|_| {
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "converting total entry weight",
            }
        })?;
        let mut selected = i64::try_from(self.bounded_random(bound)?).map_err(|_| {
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "converting weighted random selection",
            }
        })?;
        for (index, weight) in weights.into_iter().enumerate() {
            self.charge(1)?;
            selected -= i64::from(weight);
            if selected < 0 {
                return Ok(Some(index));
            }
        }
        Err(EntityLootEvaluationError::CatalogInvariant {
            message: "positive weighted selection did not choose a candidate".to_string(),
        })
    }

    fn create_candidate(
        &mut self,
        candidate: &Candidate<'catalog>,
        decorators: &mut Vec<&'catalog [LootFunction]>,
        depth: usize,
    ) -> Result<(), EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        match candidate {
            Candidate::Entry(entry) => match entry {
                LootEntry::Item { singleton, item } => self.emit(
                    WorkingStack::one(item.clone()),
                    &singleton.functions,
                    decorators,
                    self.next_depth(depth)?,
                ),
                LootEntry::Table { singleton, table } => {
                    decorators.push(&singleton.functions);
                    let result = self.evaluate_table(table, decorators, self.next_depth(depth)?);
                    decorators.pop();
                    result
                }
                LootEntry::Empty { .. } => Ok(()),
                LootEntry::Tag {
                    singleton,
                    tag,
                    expand: false,
                } => {
                    let items = self.context.tags.item_tag(tag).ok_or_else(|| {
                        EntityLootEvaluationError::MissingItemTag { tag: tag.clone() }
                    })?;
                    self.check_tag_expansion(items.len())?;
                    for item in items {
                        self.charge(1)?;
                        self.emit(
                            WorkingStack::one(item.clone()),
                            &singleton.functions,
                            decorators,
                            self.next_depth(depth)?,
                        )?;
                    }
                    Ok(())
                }
                LootEntry::Tag { expand: true, .. } => {
                    Err(EntityLootEvaluationError::CatalogInvariant {
                        message: "expanded tag reached candidate creation without an item"
                            .to_string(),
                    })
                }
                LootEntry::Alternatives { .. } => {
                    Err(EntityLootEvaluationError::CatalogInvariant {
                        message: "alternatives entry reached weighted candidate creation"
                            .to_string(),
                    })
                }
            },
            Candidate::TagItem { singleton, item } => self.emit(
                WorkingStack::one(item.clone()),
                &singleton.functions,
                decorators,
                self.next_depth(depth)?,
            ),
        }
    }

    fn emit(
        &mut self,
        mut stack: WorkingStack,
        entry_functions: &[LootFunction],
        decorators: &[&'catalog [LootFunction]],
        depth: usize,
    ) -> Result<(), EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.apply_functions(entry_functions, &mut stack, self.next_depth(depth)?)?;
        for functions in decorators.iter().rev() {
            self.apply_functions(functions, &mut stack, self.next_depth(depth)?)?;
        }
        if stack.count <= 0 {
            return Ok(());
        }
        self.charge(1)?;
        let count = u32::try_from(stack.count).map_err(|_| {
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "converting final stack count",
            }
        })?;
        let stack_count = self.output.len().checked_add(1).ok_or(
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "counting output stacks",
            },
        )?;
        if stack_count > MAX_OUTPUT_STACKS {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::OutputStacks,
                stack_count as u64,
            ));
        }
        let output_items = self.output_items.checked_add(u64::from(count)).ok_or(
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "adding total output item count",
            },
        )?;
        if output_items > MAX_OUTPUT_ITEMS {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::OutputItems,
                output_items,
            ));
        }
        self.output_items = output_items;
        self.output.push(EntityLootStack {
            item: stack.item,
            count,
            components: stack.components,
        });
        Ok(())
    }

    fn apply_functions(
        &mut self,
        functions: &[LootFunction],
        stack: &mut WorkingStack,
        depth: usize,
    ) -> Result<(), EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        for function in functions {
            self.charge(1)?;
            match function {
                LootFunction::SetCount {
                    conditions,
                    count,
                    add,
                } => {
                    if !self.conditions_match(conditions, self.next_depth(depth)?)? {
                        continue;
                    }
                    let base = if *add { stack.count } else { 0 };
                    let count = base.checked_add(i64::from(self.sample_int(count)?)).ok_or(
                        EntityLootEvaluationError::ArithmeticOverflow {
                            operation: "applying set_count",
                        },
                    )?;
                    stack.count = checked_item_count(count, "applying set_count")?;
                }
                LootFunction::EnchantedCountIncrease {
                    conditions,
                    enchantment,
                    count,
                    limit,
                } => {
                    if !self.conditions_match(conditions, self.next_depth(depth)?)? {
                        continue;
                    }
                    self.charge(1)?;
                    let level = self
                        .context
                        .cause
                        .source_attacker()
                        .filter(|attacker| attacker.is_living)
                        .map(|attacker| attacker.active_enchantments.level(enchantment))
                        .unwrap_or(0);
                    if level == 0 {
                        continue;
                    }
                    let addition = f64::from(level) * self.sample_float(count)?;
                    let addition =
                        finite_round_i64(addition, "calculating enchanted count increase")?;
                    let count = stack.count.checked_add(addition).ok_or(
                        EntityLootEvaluationError::ArithmeticOverflow {
                            operation: "adding enchanted count increase",
                        },
                    )?;
                    stack.count = checked_item_count(count, "adding enchanted count increase")?;
                    if let Some(limit) = limit.filter(|limit| *limit > 0) {
                        stack.count = stack.count.min(i64::from(limit));
                    }
                }
                LootFunction::FurnaceSmelt {
                    conditions,
                    use_input_count,
                } => {
                    if stack.count <= 0
                        || !self.conditions_match(conditions, self.next_depth(depth)?)?
                    {
                        continue;
                    }
                    let Some(recipe) = self.context.recipes.smelting_recipe(&stack.item) else {
                        continue;
                    };
                    self.charge(1)?;
                    if recipe.output_count == 0 || recipe.max_stack_size == 0 {
                        return Err(EntityLootEvaluationError::InvalidContext {
                            message: format!(
                                "smelting recipe for {} has an empty output",
                                stack.item
                            ),
                        });
                    }
                    let input_count = if *use_input_count { stack.count } else { 1 };
                    let output_count = input_count
                        .checked_mul(i64::from(recipe.output_count))
                        .ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                            operation: "multiplying a smelting result count",
                        })?
                        .min(i64::from(recipe.max_stack_size));
                    stack.item = recipe.output.clone();
                    stack.count = checked_item_count(output_count, "applying furnace_smelt")?;
                    stack.components = EntityLootComponents::default();
                }
                LootFunction::SetPotion { conditions, potion } => {
                    if self.conditions_match(conditions, self.next_depth(depth)?)? {
                        stack.components.potion = Some(potion.clone());
                    }
                }
                LootFunction::SetOminousBottleAmplifier {
                    conditions,
                    amplifier,
                } => {
                    if self.conditions_match(conditions, self.next_depth(depth)?)? {
                        let amplifier = self.sample_int(amplifier)?.clamp(0, 4);
                        stack.components.ominous_bottle_amplifier =
                            Some(u8::try_from(amplifier).map_err(|_| {
                                EntityLootEvaluationError::ArithmeticOverflow {
                                    operation: "converting ominous bottle amplifier",
                                }
                            })?);
                    }
                }
            }
        }
        Ok(())
    }

    fn conditions_match(
        &mut self,
        conditions: &[LootCondition],
        depth: usize,
    ) -> Result<bool, EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        for condition in conditions {
            if !self.condition_matches(condition, self.next_depth(depth)?)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn condition_matches(
        &mut self,
        condition: &LootCondition,
        depth: usize,
    ) -> Result<bool, EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        match condition {
            LootCondition::AnyOf(terms) => {
                for term in terms {
                    if self.condition_matches(term, self.next_depth(depth)?)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            LootCondition::DamageSource(predicate) => {
                self.damage_source_matches(predicate, self.next_depth(depth)?)
            }
            LootCondition::EntityProperties { target, predicate } => {
                let context = self.context;
                let Some(entity) = Self::target_entity(context, *target) else {
                    return Ok(false);
                };
                self.entity_matches(predicate, entity, self.next_depth(depth)?)
            }
            LootCondition::Inverted(term) => self
                .condition_matches(term, self.next_depth(depth)?)
                .map(|matched| !matched),
            LootCondition::KilledByPlayer => Ok(self.context.cause.attributed_player().is_some()),
            LootCondition::RandomChance(chance) => {
                let chance = self.sample_float(chance)?;
                if chance <= 0.0 {
                    return Ok(false);
                }
                if chance >= 1.0 {
                    return Ok(true);
                }
                Ok(self.next_unit_f64()? < chance)
            }
            LootCondition::RandomChanceWithEnchantedBonus {
                unenchanted_chance,
                base,
                per_level_above_first,
                enchantment,
            } => {
                self.charge(1)?;
                let level = self
                    .context
                    .cause
                    .source_attacker()
                    .filter(|attacker| attacker.is_living)
                    .map(|attacker| attacker.active_enchantments.level(enchantment))
                    .unwrap_or(0);
                let chance = if level == 0 {
                    *unenchanted_chance
                } else {
                    base + per_level_above_first * f64::from(level - 1)
                };
                if !chance.is_finite() {
                    return Err(EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "calculating enchanted random chance",
                    });
                }
                if chance <= 0.0 {
                    return Ok(false);
                }
                if chance >= 1.0 {
                    return Ok(true);
                }
                Ok(self.next_unit_f64()? < chance)
            }
        }
    }

    fn target_entity<'a>(
        context: &'a EntityLootContext<'_>,
        target: EntityTarget,
    ) -> Option<&'a EntityLootEntity> {
        match target {
            EntityTarget::This => Some(&context.this_entity),
            EntityTarget::Attacker => context.cause.source_attacker(),
            EntityTarget::DirectAttacker => context.cause.direct_attacker(),
            EntityTarget::AttackingPlayer => context.cause.attributed_player(),
        }
    }

    fn entity_matches(
        &mut self,
        predicate: &EntityPredicate,
        entity: &EntityLootEntity,
        depth: usize,
    ) -> Result<bool, EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        if predicate
            .entity_type
            .as_ref()
            .is_some_and(|expected| !self.tagged_entity_type_matches(expected, &entity.entity_type))
        {
            return Ok(false);
        }
        for (component, expected) in &predicate.components {
            self.charge(1)?;
            if entity.components.get(component) != Some(expected) {
                return Ok(false);
            }
        }
        if predicate
            .is_baby
            .is_some_and(|expected| entity.flags.is_baby != expected)
            || predicate
                .is_on_fire
                .is_some_and(|expected| entity.flags.is_on_fire != expected)
        {
            return Ok(false);
        }
        if let Some(expected) = &predicate.mainhand_enchantments {
            if !entity.is_living {
                return Ok(false);
            }
            let Some(mainhand) = &entity.mainhand else {
                return Ok(false);
            };
            for expected in expected {
                self.charge(1)?;
                if !self.tagged_enchantment_matches(expected, mainhand.enchantments())? {
                    return Ok(false);
                }
            }
        }
        if let Some(vehicle) = &predicate.vehicle {
            let Some(actual_vehicle) = &entity.vehicle else {
                return Ok(false);
            };
            if !self.entity_matches(vehicle, actual_vehicle, self.next_depth(depth)?)? {
                return Ok(false);
            }
        }
        if predicate
            .type_specific
            .as_ref()
            .is_some_and(|specific| !type_specific_matches(specific, entity))
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn tagged_entity_type_matches(
        &self,
        expected: &TaggedIdentifier,
        entity_type: &Identifier,
    ) -> bool {
        match expected {
            TaggedIdentifier::Exact(expected) => expected == entity_type,
            TaggedIdentifier::Tag(tag) => self.context.tags.entity_type_in_tag(tag, entity_type),
        }
    }

    fn tagged_enchantment_matches(
        &mut self,
        expected: &TaggedIdentifier,
        enchantments: &super::super::LootEnchantments,
    ) -> Result<bool, EntityLootEvaluationError> {
        match expected {
            TaggedIdentifier::Exact(expected) => Ok(enchantments.contains(expected)),
            TaggedIdentifier::Tag(tag) => {
                for enchantment in enchantments.identifiers() {
                    self.charge(1)?;
                    if self.context.tags.enchantment_in_tag(tag, enchantment) {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn damage_source_matches(
        &mut self,
        predicate: &DamageSourcePredicate,
        depth: usize,
    ) -> Result<bool, EntityLootEvaluationError> {
        self.check_runtime_depth(depth)?;
        self.charge(1)?;
        for (tag, expected) in &predicate.tags {
            self.charge(1)?;
            if self.context.cause.tags().contains(tag) != *expected {
                return Ok(false);
            }
        }
        if let Some(direct_predicate) = &predicate.direct_entity {
            let context = self.context;
            let Some(entity) = context.cause.direct_attacker() else {
                return Ok(false);
            };
            if !self.entity_matches(direct_predicate, entity, self.next_depth(depth)?)? {
                return Ok(false);
            }
        }
        if let Some(source_predicate) = &predicate.source_entity {
            let context = self.context;
            let Some(entity) = context.cause.source_attacker() else {
                return Ok(false);
            };
            if !self.entity_matches(source_predicate, entity, self.next_depth(depth)?)? {
                return Ok(false);
            }
        }
        Ok(predicate
            .is_direct
            .is_none_or(|expected| self.context.cause.is_direct() == expected))
    }

    fn sample_int(&mut self, provider: &NumberProvider) -> Result<i32, EntityLootEvaluationError> {
        self.charge(1)?;
        match provider {
            NumberProvider::Constant(value) => {
                floor_f64_to_i32(*value).ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                    operation: "sampling a constant number provider",
                })
            }
            NumberProvider::Uniform { min, max } => {
                let min = floor_f64_to_i32(*min).ok_or(
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "sampling a uniform minimum",
                    },
                )?;
                let max = floor_f64_to_i32(*max).ok_or(
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "sampling a uniform maximum",
                    },
                )?;
                if min >= max {
                    return Ok(min);
                }
                let width = i64::from(max) - i64::from(min) + 1;
                let width = u64::try_from(width).map_err(|_| {
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "converting uniform integer width",
                    }
                })?;
                let offset = i64::try_from(self.bounded_random(width)?).map_err(|_| {
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "converting uniform integer offset",
                    }
                })?;
                i32::try_from(i64::from(min) + offset).map_err(|_| {
                    EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "sampling a uniform integer",
                    }
                })
            }
        }
    }

    fn sample_float(
        &mut self,
        provider: &NumberProvider,
    ) -> Result<f64, EntityLootEvaluationError> {
        self.charge(1)?;
        let value = match provider {
            NumberProvider::Constant(value) => *value,
            NumberProvider::Uniform { min, max } if min >= max => *min,
            NumberProvider::Uniform { min, max } => min + self.next_unit_f64()? * (max - min),
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(EntityLootEvaluationError::ArithmeticOverflow {
                operation: "sampling a floating-point number provider",
            })
        }
    }

    fn bounded_random(&mut self, bound: u64) -> Result<u64, EntityLootEvaluationError> {
        if bound == 0 {
            return Err(EntityLootEvaluationError::CatalogInvariant {
                message: "bounded random selection received a zero bound".to_string(),
            });
        }
        let threshold = bound.wrapping_neg() % bound;
        loop {
            self.charge(1)?;
            let value = self.random.next_u64();
            if value >= threshold {
                return Ok(value % bound);
            }
        }
    }

    fn next_unit_f64(&mut self) -> Result<f64, EntityLootEvaluationError> {
        self.charge(1)?;
        const SCALE: f64 = 1.0 / ((1_u64 << 53) as f64);
        Ok(((self.random.next_u64() >> 11) as f64) * SCALE)
    }

    fn check_tag_expansion(&self, actual: usize) -> Result<(), EntityLootEvaluationError> {
        if actual > MAX_TAG_EXPANSION {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::TagExpansion,
                actual as u64,
            ));
        }
        Ok(())
    }

    fn check_runtime_depth(&self, depth: usize) -> Result<(), EntityLootEvaluationError> {
        if depth > MAX_RUNTIME_RECURSION {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::RuntimeRecursion,
                depth as u64,
            ));
        }
        Ok(())
    }

    fn next_depth(&self, depth: usize) -> Result<usize, EntityLootEvaluationError> {
        depth
            .checked_add(1)
            .ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                operation: "increasing runtime recursion depth",
            })
    }

    fn charge(&mut self, count: usize) -> Result<(), EntityLootEvaluationError> {
        let operations = self.operations.checked_add(count).ok_or(
            EntityLootEvaluationError::ArithmeticOverflow {
                operation: "adding total evaluation operations",
            },
        )?;
        if operations > MAX_TOTAL_OPERATIONS {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::TotalOperations,
                operations as u64,
            ));
        }
        self.operations = operations;
        Ok(())
    }
}

#[derive(Clone)]
enum Candidate<'catalog> {
    Entry(&'catalog LootEntry),
    TagItem {
        singleton: &'catalog SingletonEntry,
        item: Identifier,
    },
}

impl Candidate<'_> {
    fn singleton(&self) -> Result<&SingletonEntry, EntityLootEvaluationError> {
        match self {
            Self::Entry(entry) => {
                entry
                    .singleton()
                    .ok_or_else(|| EntityLootEvaluationError::CatalogInvariant {
                        message: "non-singleton entry entered weighted selection".to_string(),
                    })
            }
            Self::TagItem { singleton, .. } => Ok(singleton),
        }
    }
}

fn validate_context_collections(
    context: &EntityLootContext<'_>,
) -> Result<(), EntityLootEvaluationError> {
    check_context_limit(
        EntityLootLimit::ContextDamageTags,
        context.cause.tags().len(),
    )?;
    validate_entity_collections(&context.this_entity)?;
    if let Some(entity) = context.cause.source_attacker() {
        validate_entity_collections(entity)?;
    }
    if let Some(entity) = context.cause.direct_attacker() {
        validate_entity_collections(entity)?;
    }
    if let Some(entity) = context.cause.attributed_player() {
        validate_entity_collections(entity)?;
    }
    Ok(())
}

fn validate_entity_collections(entity: &EntityLootEntity) -> Result<(), EntityLootEvaluationError> {
    let mut pending = vec![(entity, 0_usize)];
    while let Some((entity, depth)) = pending.pop() {
        if depth > MAX_RUNTIME_RECURSION {
            return Err(EntityLootEvaluationError::limit(
                EntityLootLimit::RuntimeRecursion,
                depth as u64,
            ));
        }
        check_context_limit(EntityLootLimit::ContextComponents, entity.components.len())?;
        check_context_limit(
            EntityLootLimit::ContextEnchantmentLevels,
            entity.active_enchantments.len(),
        )?;
        if let Some(mainhand) = &entity.mainhand {
            check_context_limit(
                EntityLootLimit::ContextEnchantments,
                mainhand.enchantments().len(),
            )?;
        }
        if let Some(vehicle) = entity.vehicle.as_deref() {
            let next_depth =
                depth
                    .checked_add(1)
                    .ok_or(EntityLootEvaluationError::ArithmeticOverflow {
                        operation: "increasing context recursion depth",
                    })?;
            pending.push((vehicle, next_depth));
        }
    }
    Ok(())
}

fn check_context_limit(
    limit: EntityLootLimit,
    actual: usize,
) -> Result<(), EntityLootEvaluationError> {
    if actual as u64 > limit.maximum() {
        return Err(EntityLootEvaluationError::limit(limit, actual as u64));
    }
    Ok(())
}

fn type_specific_matches(predicate: &TypeSpecificPredicate, entity: &EntityLootEntity) -> bool {
    match predicate {
        TypeSpecificPredicate::Sheep { sheared } => entity.sheep_sheared == Some(*sheared),
        TypeSpecificPredicate::Slime { size } => entity
            .slime_size
            .is_some_and(|actual| size.min <= actual && actual <= size.max),
        TypeSpecificPredicate::Raider { is_captain } => {
            entity.raider_is_captain == Some(*is_captain)
        }
    }
}

fn checked_item_count(
    count: i64,
    operation: &'static str,
) -> Result<i64, EntityLootEvaluationError> {
    if count < i64::from(i32::MIN) || count > i64::from(i32::MAX) {
        return Err(EntityLootEvaluationError::ArithmeticOverflow { operation });
    }
    Ok(count)
}

fn finite_floor_i64(value: f64, operation: &'static str) -> Result<i64, EntityLootEvaluationError> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(EntityLootEvaluationError::ArithmeticOverflow { operation });
    }
    Ok(value.floor() as i64)
}

fn finite_round_i64(value: f64, operation: &'static str) -> Result<i64, EntityLootEvaluationError> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(EntityLootEvaluationError::ArithmeticOverflow { operation });
    }
    // Java's Math.round is floor(value + 0.5), including for negatives.
    Ok((value + 0.5).floor() as i64)
}

fn floor_f64_to_i32(value: f64) -> Option<i32> {
    let value = value.floor();
    (value.is_finite() && value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX))
        .then_some(value as i32)
}
