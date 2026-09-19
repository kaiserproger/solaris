//! The manifest's `required_features` rule, for every package the host loads.
//!
//! A capability name is what a package asks the operator to grant; a required
//! feature is what the package tells core about its authored data - the durable
//! subsystems it cannot run without, which core reads before a world exists (a
//! deployed settlement profile, for one). The two lists share one vocabulary, and
//! a package that declares a feature-capability without also requiring it would
//! silently ship data core is not told to load, so the pair is checked together.
//!
//! This is the component deployment's only required-feature rule, so one package
//! cannot be admitted with data core is not told to load.

use mc_script::{MAX_MANIFEST_CAPABILITIES, validate_script_id_value};

/// Capability names a package must also declare in `required_features`.
const FEATURE_CAPABILITIES: [&str; 7] = [
    "storage_batches",
    "inventory_transfers",
    "persistent_residents",
    "resident_work",
    "resident_orders",
    "world_sites",
    "structure_operations",
];

/// Validate one manifest's `required_features` against its own `capabilities`.
///
/// Every entry must be a bounded script id naming one of the features core knows,
/// and every feature-capability the manifest declares must be required as well.
/// The message is the operator's diagnosis, so it names the field and the value.
pub fn validate(capabilities: &[String], required_features: &[String]) -> Result<(), String> {
    if required_features.len() > MAX_MANIFEST_CAPABILITIES {
        return Err("too many required plugin features".to_owned());
    }
    for feature in required_features {
        validate_script_id_value(feature).map_err(|error| error.to_string())?;
        if !FEATURE_CAPABILITIES.contains(&feature.as_str()) {
            return Err(format!("unsupported required plugin feature {feature:?}"));
        }
    }
    for feature in FEATURE_CAPABILITIES {
        if capabilities.iter().any(|capability| capability == feature)
            && !required_features.iter().any(|declared| declared == feature)
        {
            return Err(format!(
                "{feature} capability requires required_features = [\"{feature}\"]"
            ));
        }
    }
    Ok(())
}
