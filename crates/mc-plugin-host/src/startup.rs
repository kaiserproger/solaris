//! The startup a component deployment contributes.
//!
//! `configure` is the one phase of the component contract that answers something
//! other than commands: a `rule-plan`. This module is where that answer becomes
//! the world's own startup contract - the same [`GameplayRules`] the Luau host
//! produces from `rules.lua` - and it is the only place that does so. The run
//! path records what it produces here on the host, and `check_deployment` refuses
//! the same plan with the same refusal, so a deployment a check accepts is a
//! deployment the run path materializes the same way.
//!
//! Nothing here repairs a value. A `rule-plan` field wider than the startup field
//! that has to hold it is refused rather than truncated or saturated, because
//! materializing rules other than the ones the package declared would change the
//! world fingerprint silently - which is the very decision the fingerprint exists
//! to make. A plan that breaks a bound [`GameplayRules::validate`] enforces is
//! refused for the same reason: the native owners downstream only bound what
//! those checks let through.

use std::collections::BTreeMap;

use mc_script::{
    BiomeSpawns, ClayRule, GameplayRules, GameplayRulesError, SpawnEntry, SpawnPlacement, TreeRule,
};

use crate::bindings::exports::solaris::plugin::lifecycle::{
    ClayRules, PlacementRules, RulePlan, SpawnGroup, SpawnRules, TreeRules,
};

/// A `rule-plan` value the startup contract's own field cannot hold.
///
/// The field is named the way `lifecycle.wit` names it, so an operator can find
/// the declaration that has to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("rule-plan field {field} is {value}, the startup contract's bound is {max}")]
pub struct FieldOverflow {
    /// The WIT field that carried the value.
    pub field: &'static str,
    /// What the component declared.
    pub value: u64,
    /// The largest value the narrower contract field can hold.
    pub max: u64,
}

/// Why a component's `rule-plan` did not become startup rules.
///
/// Every variant here is a refusal to materialize, not a warning: the
/// composition root fails startup closed on one, and a package whose `configure`
/// answered no plan at all is not one of them (see
/// [`ContributionOutcome::NoPlan`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RulePlanRefusal {
    /// A declared value does not fit the narrower startup field.
    #[error(transparent)]
    Overflow(#[from] FieldOverflow),
    /// Two `spawn-group`s in one `spawn-rules` share a name.
    ///
    /// The startup contract holds a biome's groups in a map keyed by group name,
    /// so a repeated name has no second slot: keeping either one would silently
    /// drop the other replacement the package asked for.
    #[error("spawn-rules for biome {biome:?} declares group {group:?} twice")]
    DuplicateSpawnGroup {
        /// The biome whose declaration repeats itself.
        biome: String,
        /// The group name declared twice.
        group: String,
    },
    /// The converted plan breaks a bound the startup contract enforces.
    #[error(transparent)]
    Invalid(#[from] GameplayRulesError),
}

/// Convert a component's `configure` answer into the startup rule contract.
///
/// Total over the contract's shape: every category and every nested record is
/// carried across, and every value either fits its field exactly or the plan is
/// refused. The result is validated, so the rules a caller receives are rules a
/// native owner can be handed.
pub fn convert_rule_plan(plan: &RulePlan) -> Result<GameplayRules, RulePlanRefusal> {
    // `option<list<..>>` and `list<..>` are different absences: `None` is "this
    // package sets no such category" and `Some(vec![])` is "it sets an empty
    // one", which `validate` refuses below because the plan then sets nothing.
    let spawning = plan
        .spawning
        .iter()
        .flatten()
        .map(convert_spawn_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let trees = plan
        .trees
        .iter()
        .flatten()
        .map(convert_tree_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let clay = plan.clay.as_ref().map(convert_clay_rules).transpose()?;
    let placement = plan
        .placement
        .as_ref()
        .map(convert_placement_rules)
        .transpose()?;

    let rules = GameplayRules::new(spawning, trees, clay, placement);
    // The one validator, so both runtimes refuse the same plans: an empty plan
    // (`NoCategory`), more than 64 declarations (`TooManyDeclarations`), an
    // unsupported or over-full spawn group (`UnsupportedSpawnGroup`), an
    // over-long, duplicated or over-budget spawn entry (`InvalidSpawnEntry`), a
    // tree declaration outside its bounds (`InvalidTreeRule`) or naming a biome
    // twice (`InvalidTreeBiome`), and clay or placement outside the bounded
    // deposit and search dimensions (`ClayOutOfBounds`, `PlacementOutOfBounds`).
    rules.validate()?;
    Ok(rules)
}

/// One `spawn-rules` record as the contract's per-biome spawn groups.
fn convert_spawn_rules(rules: &SpawnRules) -> Result<BiomeSpawns, RulePlanRefusal> {
    let mut groups = BTreeMap::new();
    for group in &rules.groups {
        let entries = convert_spawn_group(group);
        if groups.insert(group.group.clone(), entries).is_some() {
            // Refused rather than overwritten: see `DuplicateSpawnGroup`.
            return Err(RulePlanRefusal::DuplicateSpawnGroup {
                biome: rules.biome.clone(),
                group: group.group.clone(),
            });
        }
    }
    Ok(BiomeSpawns::new(rules.biome.clone(), groups))
}

/// One `spawn-group` record's entries.
///
/// `spawn-entry` and the contract's entry have the same names and the same
/// widths: `min`, `max` and `weight` are `u32` on both sides, so this conversion
/// has no field to narrow and no value to refuse. Their bounds are `validate`'s
/// (`InvalidSpawnEntry`).
fn convert_spawn_group(group: &SpawnGroup) -> Vec<SpawnEntry> {
    group
        .entries
        .iter()
        .map(|entry| SpawnEntry::new(entry.entity.clone(), entry.min, entry.max, entry.weight))
        .collect()
}

/// One `tree-rules` record as the contract's tree declaration.
fn convert_tree_rules(rule: &TreeRules) -> Result<TreeRule, RulePlanRefusal> {
    // `spacing` is `u32` in WIT and `u64` in the contract, so every declared
    // value fits its field exactly: there is no width for this one to overflow.
    Ok(TreeRule::new(
        rule.biomes.clone(),
        u64::from(rule.spacing),
        rule.density_threshold,
    ))
}

/// One `clay-rules` record as the contract's clay declaration.
fn convert_clay_rules(rule: &ClayRules) -> Result<ClayRule, RulePlanRefusal> {
    Ok(ClayRule::new(
        // `rarity` is `u32` in WIT and `u64` in the contract: exact, never
        // truncated, and no value can overflow it.
        u64::from(rule.rarity),
        narrow_to_u8("clay-rules.radius-min", rule.radius_min)?,
        narrow_to_u8("clay-rules.radius-max", rule.radius_max)?,
        narrow_to_u8("clay-rules.max-water-depth", rule.max_water_depth)?,
    ))
}

/// One `placement-rules` record as the contract's spawn placement.
fn convert_placement_rules(rule: &PlacementRules) -> Result<SpawnPlacement, RulePlanRefusal> {
    Ok(SpawnPlacement::new(
        narrow_to_u8("placement-rules.land-spacing", rule.land_spacing)?,
        narrow_to_u8("placement-rules.water-attempts", rule.water_attempts)?,
        narrow_to_u8("placement-rules.water-depth", rule.water_depth)?,
    ))
}

/// Narrow one `u32` contract value to the `u8` its startup field holds.
///
/// A value above `u8::MAX` is refused as [`FieldOverflow`]: the bounds these
/// fields carry are single bytes in the startup contract, and `300` truncated to
/// `44` would be a different rule than the package declared.
fn narrow_to_u8(field: &'static str, value: u32) -> Result<u8, FieldOverflow> {
    u8::try_from(value).map_err(|_| FieldOverflow {
        field,
        value: u64::from(value),
        max: u64::from(u8::MAX),
    })
}

/// What one package's `configure` contributed to startup.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContributionOutcome {
    /// The package answered no plan: a legitimate package, not a refusal.
    NoPlan,
    /// The plan converted and validated into the rules the world opens with.
    Rules(GameplayRules),
    /// The plan was refused and no caller may materialize it.
    Refused(RulePlanRefusal),
}

/// One package of a deployment and what it contributed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PackageContribution {
    id: String,
    outcome: ContributionOutcome,
}

impl PackageContribution {
    /// The plugin id the manifest declared.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// What its `configure` contributed.
    #[must_use]
    pub fn outcome(&self) -> &ContributionOutcome {
        &self.outcome
    }

    /// The validated rules, when the package contributed a plan that survived
    /// validation.
    #[must_use]
    pub fn rules(&self) -> Option<&GameplayRules> {
        match &self.outcome {
            ContributionOutcome::Rules(rules) => Some(rules),
            ContributionOutcome::NoPlan | ContributionOutcome::Refused(_) => None,
        }
    }

    /// The refusal, when the package declared a plan the contract will not take.
    #[must_use]
    pub fn refusal(&self) -> Option<&RulePlanRefusal> {
        match &self.outcome {
            ContributionOutcome::Refused(refusal) => Some(refusal),
            ContributionOutcome::NoPlan | ContributionOutcome::Rules(_) => None,
        }
    }
}

/// What every package of a started deployment contributed to startup.
///
/// The composition root's input, not its policy: it answers per package, and the
/// caller decides that a refused plan fails startup while a package with no plan
/// is ordinary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeploymentContribution {
    packages: Vec<PackageContribution>,
}

impl DeploymentContribution {
    /// Every package, in the order the deployment started them.
    #[must_use]
    pub fn packages(&self) -> &[PackageContribution] {
        &self.packages
    }

    /// The first refused plan and the package that declared it, if any.
    ///
    /// A caller that must fail startup closed reads this one value; the rest of
    /// the deployment's answers are on [`DeploymentContribution::packages`].
    #[must_use]
    pub fn refusal(&self) -> Option<(&str, &RulePlanRefusal)> {
        self.packages
            .iter()
            .find_map(|package| package.refusal().map(|refusal| (package.id(), refusal)))
    }

    /// The validated rules of every package that contributed a plan.
    ///
    /// More than one is a world-contract conflict the caller has to settle: one
    /// world is opened against one rule set, so two owners cannot both win.
    pub fn rules(&self) -> impl Iterator<Item = (&str, &GameplayRules)> {
        self.packages
            .iter()
            .filter_map(|package| package.rules().map(|rules| (package.id(), rules)))
    }

    /// Record what one package's `configure` answered.
    ///
    /// Called by the host on the same answer it already logs, so the recorded
    /// contribution is the run path's own, not a second run of the phase.
    pub(crate) fn record(&mut self, id: &str, plan: Option<&RulePlan>) {
        let outcome = match plan {
            None => ContributionOutcome::NoPlan,
            Some(plan) => match convert_rule_plan(plan) {
                Ok(rules) => ContributionOutcome::Rules(rules),
                Err(refusal) => ContributionOutcome::Refused(refusal),
            },
        };
        self.packages.push(PackageContribution {
            id: id.to_owned(),
            outcome,
        });
    }
}
