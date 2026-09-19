//! The startup a component deployment contributes.
//!
//! `configure` is the one phase of the component contract that answers something
//! other than commands: a `startup-contribution`. This module materializes that
//! answer into the world's startup contract, and it is the only place that does
//! so. The run path records what it produces here on the host, and
//! `check_deployment` refuses the same contribution with the same refusal, so a
//! deployment a check accepts is a deployment the run path materializes the same
//! way.
//!
//! The deployment's *own* declarations live here too: the client bundles and the
//! two worldgen profiles, settled by [`aggregate_deployment`] for the check and the
//! run path alike.
//!
//! Nothing here repairs a value. A `startup-contribution` field wider than the
//! startup field that has to hold it is refused rather than truncated or
//! saturated, because materializing rules other than the ones the package declared
//! would change the world fingerprint silently - which is the very decision the
//! fingerprint exists to make. A contribution that breaks a bound
//! [`GameplayRules::validate`] enforces is refused for the same reason: the native
//! owners downstream only bound what those checks let through.

use std::collections::BTreeMap;

use mc_script::{
    BiomeSpawns, ClayRule, ClientBundle, GameplayRules, GameplayRulesError, PluginSettlementPlan,
    PluginWorldgenOreProfile, SpawnEntry, SpawnPlacement, TreeRule,
};

use crate::bindings::exports::solaris::plugin::lifecycle::{
    ClayRules, PlacementRules, SpawnGroup, SpawnRules, StartupContribution, TreeRules,
};
use crate::package::LoadedPackage;

/// A `startup-contribution` value the startup contract's own field cannot hold.
///
/// The field is named the way `lifecycle.wit` names it, so an operator can find
/// the declaration that has to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("startup-contribution field {field} is {value}, the startup contract's bound is {max}")]
pub struct FieldOverflow {
    /// The WIT field that carried the value.
    pub field: &'static str,
    /// What the component declared.
    pub value: u64,
    /// The largest value the narrower contract field can hold.
    pub max: u64,
}

/// The deployment's own declarations: the client bundles the Loader stages and
/// the two worldgen profiles a world opens against.
///
/// They belong to the deployment rather than to one package, so both the check and
/// the run path settle them with [`aggregate_deployment`] - one function, one
/// policy - and a deployment a check accepts and a deployment that starts cannot
/// disagree about them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeploymentSurface {
    client_bundles: Vec<ClientBundle>,
    ore_profile: Option<(String, PluginWorldgenOreProfile)>,
    settlement_plan: Option<(String, PluginSettlementPlan)>,
}

impl DeploymentSurface {
    /// Every client bundle the deployment declared, in package order.
    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundle] {
        &self.client_bundles
    }

    /// The one ore profile the deployment declared, if any.
    #[must_use]
    pub fn ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.ore_profile.as_ref().map(|(_, profile)| *profile)
    }

    /// The one settlement plan the deployment declared, if any.
    #[must_use]
    pub fn settlement_plan(&self) -> Option<&PluginSettlementPlan> {
        self.settlement_plan.as_ref().map(|(_, plan)| plan)
    }
}

/// Two packages that each declared the same worldgen profile.
///
/// A world opens against exactly one ore profile and one settlement plan, so a
/// second declaration has no slot: it is refused rather than settled by keeping
/// the later one, which would silently change the world a package asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("plugins {first} and {second} both declare a worldgen {kind} profile")]
pub struct WorldgenConflict {
    /// Which profile both packages declared.
    pub kind: &'static str,
    /// The package whose declaration came first.
    pub first: String,
    /// The package that repeated it.
    pub second: String,
}

/// Settle the deployment-level declarations of `packages`.
///
/// The one place the catalog's bundles and the world's profiles are read from a
/// deployment, so the check and the run path decide duplicates under the same
/// refusal and cannot drift into a second policy.
pub fn aggregate_deployment(
    packages: &[LoadedPackage],
) -> Result<DeploymentSurface, WorldgenConflict> {
    let mut surface = DeploymentSurface::default();
    for package in packages {
        let id = package.manifest().plugin_id();
        if let Some(profile) = package.worldgen_ore_profile() {
            if let Some((first, _)) = &surface.ore_profile {
                return Err(WorldgenConflict {
                    kind: "ore",
                    first: first.clone(),
                    second: id.to_owned(),
                });
            }
            surface.ore_profile = Some((id.to_owned(), profile));
        }
        if let Some(plan) = package.worldgen_settlement_plan() {
            if let Some((first, _)) = &surface.settlement_plan {
                return Err(WorldgenConflict {
                    kind: "settlement",
                    first: first.clone(),
                    second: id.to_owned(),
                });
            }
            surface.settlement_plan = Some((id.to_owned(), plan.clone()));
        }
        surface
            .client_bundles
            .extend(package.client_bundles().iter().cloned());
    }
    Ok(surface)
}

/// Why a component's `startup-contribution` did not become startup rules.
///
/// Every variant here is a refusal to materialize, not a warning: the
/// composition root fails startup closed on one, and a package whose `configure`
/// answered no contribution at all is not one of them (see
/// [`ContributionOutcome::NoContribution`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ContributionRefusal {
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
    /// The converted contribution breaks a bound the startup contract enforces.
    #[error(transparent)]
    Invalid(#[from] GameplayRulesError),
}

/// Convert a component's `configure` answer into the startup rule contract.
///
/// Total over the contract's shape: every category and every nested record is
/// carried across, and every value either fits its field exactly or the
/// contribution is refused. The result is validated, so the rules a caller
/// receives are rules a native owner can be handed.
pub fn convert_startup_contribution(
    contribution: &StartupContribution,
) -> Result<GameplayRules, ContributionRefusal> {
    // `option<list<..>>` and `list<..>` are different absences: `None` is "this
    // package sets no such category" and `Some(vec![])` is "it sets an empty
    // one", which `validate` refuses below because the plan then sets nothing.
    let spawning = contribution
        .spawning
        .iter()
        .flatten()
        .map(convert_spawn_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let trees = contribution
        .trees
        .iter()
        .flatten()
        .map(convert_tree_rules)
        .collect::<Result<Vec<_>, _>>()?;
    let clay = contribution
        .clay
        .as_ref()
        .map(convert_clay_rules)
        .transpose()?;
    let placement = contribution
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
fn convert_spawn_rules(rules: &SpawnRules) -> Result<BiomeSpawns, ContributionRefusal> {
    let mut groups = BTreeMap::new();
    for group in &rules.groups {
        let entries = convert_spawn_group(group);
        if groups.insert(group.group.clone(), entries).is_some() {
            // Refused rather than overwritten: see `DuplicateSpawnGroup`.
            return Err(ContributionRefusal::DuplicateSpawnGroup {
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
fn convert_tree_rules(rule: &TreeRules) -> Result<TreeRule, ContributionRefusal> {
    // `spacing` is `u32` in WIT and `u64` in the contract, so every declared
    // value fits its field exactly: there is no width for this one to overflow.
    Ok(TreeRule::new(
        rule.biomes.clone(),
        u64::from(rule.spacing),
        rule.density_threshold,
    ))
}

/// One `clay-rules` record as the contract's clay declaration.
fn convert_clay_rules(rule: &ClayRules) -> Result<ClayRule, ContributionRefusal> {
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
fn convert_placement_rules(rule: &PlacementRules) -> Result<SpawnPlacement, ContributionRefusal> {
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
    /// The package answered no contribution: a legitimate package, not a refusal.
    NoContribution,
    /// The contribution converted and validated into the rules the world opens
    /// with.
    Rules(GameplayRules),
    /// The contribution was refused and no caller may materialize it.
    Refused(ContributionRefusal),
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

    /// The validated rules, when the package's contribution survived validation.
    #[must_use]
    pub fn rules(&self) -> Option<&GameplayRules> {
        match &self.outcome {
            ContributionOutcome::Rules(rules) => Some(rules),
            ContributionOutcome::NoContribution | ContributionOutcome::Refused(_) => None,
        }
    }

    /// The refusal, when the package declared a contribution the contract will not
    /// take.
    #[must_use]
    pub fn refusal(&self) -> Option<&ContributionRefusal> {
        match &self.outcome {
            ContributionOutcome::Refused(refusal) => Some(refusal),
            ContributionOutcome::NoContribution | ContributionOutcome::Rules(_) => None,
        }
    }
}

/// What every package of a started deployment contributed to startup.
///
/// The composition root's input, not its policy: it answers per package, and the
/// caller decides that a refused contribution fails startup while a package that
/// declared none is ordinary.
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

    /// The first refused contribution and the package that declared it, if any.
    ///
    /// A caller that must fail startup closed reads this one value; the rest of
    /// the deployment's answers are on [`DeploymentContribution::packages`].
    #[must_use]
    pub fn refusal(&self) -> Option<(&str, &ContributionRefusal)> {
        self.packages
            .iter()
            .find_map(|package| package.refusal().map(|refusal| (package.id(), refusal)))
    }

    /// The validated rules of every package that contributed them.
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
    /// Called by the host on the same answer it already has, so the recorded
    /// contribution is the run path's own, not a second run of the phase.
    pub(crate) fn record(&mut self, id: &str, contribution: Option<&StartupContribution>) {
        let outcome = match contribution {
            None => ContributionOutcome::NoContribution,
            Some(contribution) => match convert_startup_contribution(contribution) {
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
