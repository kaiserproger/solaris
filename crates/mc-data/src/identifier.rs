//! `namespace:path` identifiers, as used pervasively by Minecraft
//! data packs and the protocol.
//!
//! Lives in `mc-data` rather than `mc-protocol` because data files,
//! registry IDs, block and item names all reference it independently
//! of any wire concern. `mc-protocol` re-exports the type so existing
//! `mc_protocol::codec::Identifier` paths keep working.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A validated `namespace:path` identifier.
///
/// Stored as a single owned string and an index pointing at the colon.
/// We don't intern here; if interning turns out to matter for the
/// data-pack loader we'll revisit (PROJECT_SPEC §3.2 leaves room).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Identifier {
    full: String,
    colon: usize,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IdentifierRepr {
    Canonical(String),
    Legacy(LegacyIdentifier),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyIdentifier {
    full: String,
    colon: usize,
}

#[derive(Debug, Error)]
#[error("invalid identifier: {0:?}")]
pub struct IdentifierError(pub String);

impl Identifier {
    /// Parse a `namespace:path` (or bare `path`, which defaults to the
    /// `minecraft` namespace, as the vanilla protocol does).
    pub fn parse(input: impl Into<String>) -> Result<Self, IdentifierError> {
        let input = input.into();
        let (namespace, path) = match input.find(':') {
            Some(idx) => (&input[..idx], &input[idx + 1..]),
            None => ("minecraft", input.as_str()),
        };
        if !is_valid_namespace(namespace) || !is_valid_path(path) {
            return Err(IdentifierError(input));
        }
        if input.contains(':') {
            let colon = input.find(':').unwrap();
            Ok(Self { full: input, colon })
        } else {
            let full = format!("minecraft:{input}");
            let colon = "minecraft".len();
            Ok(Self { full, colon })
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.full
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.full[..self.colon]
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.full[self.colon + 1..]
    }
}

impl Serialize for Identifier {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match IdentifierRepr::deserialize(deserializer)? {
            IdentifierRepr::Canonical(value) => Self::parse(value).map_err(D::Error::custom),
            IdentifierRepr::Legacy(LegacyIdentifier { full, colon }) => {
                let parsed = Self::parse(full.clone()).map_err(D::Error::custom)?;
                if parsed.as_str() != full || parsed.colon != colon {
                    return Err(D::Error::custom(
                        "legacy identifier fields do not match the canonical identifier",
                    ));
                }
                Ok(parsed)
            }
        }
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.full)
    }
}

fn is_valid_namespace(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

fn is_valid_path(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_with_namespace() {
        let id = Identifier::parse("minecraft:stone").unwrap();
        assert_eq!(id.namespace(), "minecraft");
        assert_eq!(id.path(), "stone");
        assert_eq!(id.as_str(), "minecraft:stone");
    }

    #[test]
    fn defaults_namespace() {
        let id = Identifier::parse("stone").unwrap();
        assert_eq!(id.namespace(), "minecraft");
        assert_eq!(id.path(), "stone");
    }

    #[test]
    fn nested_path_ok() {
        let id = Identifier::parse("minecraft:worldgen/biome").unwrap();
        assert_eq!(id.path(), "worldgen/biome");
    }

    #[test]
    fn rejects_uppercase() {
        assert!(Identifier::parse("Minecraft:Stone").is_err());
    }

    #[test]
    fn rejects_empty_path() {
        assert!(Identifier::parse("minecraft:").is_err());
    }

    #[test]
    fn serde_writes_one_canonical_string() {
        let id = Identifier::parse("stone").unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""minecraft:stone""#);
    }

    #[test]
    fn serde_reads_canonical_string_and_strict_legacy_object() {
        let canonical: Identifier = serde_json::from_str(r#""stone""#).unwrap();
        assert_eq!(canonical.as_str(), "minecraft:stone");

        let legacy: Identifier =
            serde_json::from_str(r#"{"full":"minecraft:stone","colon":9}"#).unwrap();
        assert_eq!(legacy, canonical);
    }

    #[test]
    fn serde_rejects_impossible_legacy_state() {
        for value in [
            r#"{"full":"x","colon":999}"#,
            r#"{"full":"minecraft:stone","colon":8}"#,
            r#"{"full":"Minecraft:stone","colon":9}"#,
            r#"{"full":"minecraft:stone","colon":9,"extra":true}"#,
        ] {
            assert!(
                serde_json::from_str::<Identifier>(value).is_err(),
                "accepted invalid legacy identifier {value}"
            );
        }
    }
}
