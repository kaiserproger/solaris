use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub const MAX_IDENTIFIER_BYTES: usize = 32_767;
const DEFAULT_NAMESPACE: &str = "minecraft";

#[derive(Clone)]
pub struct Identifier {
    text: Arc<str>,
    namespace_len: u16,
    java_hash: i32,
}

impl Identifier {
    pub fn parse(value: &str) -> Result<Self, IdentifierError> {
        match value.find(':') {
            Some(0) => Self::from_namespace_and_path(DEFAULT_NAMESPACE, &value[1..]),
            Some(separator) => {
                Self::from_namespace_and_path(&value[..separator], &value[separator + 1..])
            }
            None => Self::from_namespace_and_path(DEFAULT_NAMESPACE, value),
        }
    }

    pub fn from_namespace_and_path(namespace: &str, path: &str) -> Result<Self, IdentifierError> {
        if !valid_namespace(namespace) {
            return Err(IdentifierError::InvalidNamespace);
        }
        if !valid_path(path) {
            return Err(IdentifierError::InvalidPath);
        }

        let length = namespace
            .len()
            .checked_add(1)
            .and_then(|length| length.checked_add(path.len()))
            .unwrap_or(usize::MAX);
        if length > MAX_IDENTIFIER_BYTES {
            return Err(IdentifierError::TooLong {
                length,
                maximum: MAX_IDENTIFIER_BYTES,
            });
        }

        let mut text = String::with_capacity(length);
        text.push_str(namespace);
        text.push(':');
        text.push_str(path);

        Ok(Self {
            text: Arc::from(text),
            namespace_len: namespace.len() as u16,
            java_hash: java_identifier_hash(namespace, path),
        })
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.text[..usize::from(self.namespace_len)]
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.text[usize::from(self.namespace_len) + 1..]
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn java_hash_code(&self) -> i32 {
        self.java_hash
    }
}

impl fmt::Debug for Identifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Identifier")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl PartialEq for Identifier {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.text, &other.text) || self.text == other.text
    }
}

impl Eq for Identifier {}

impl Hash for Identifier {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.namespace().hash(state);
        self.path().hash(state);
    }
}

impl PartialOrd for Identifier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Identifier {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path()
            .cmp(other.path())
            .then_with(|| self.namespace().cmp(other.namespace()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierError {
    InvalidNamespace,
    InvalidPath,
    TooLong { length: usize, maximum: usize },
}

fn valid_namespace(namespace: &str) -> bool {
    namespace != ".."
        && namespace.bytes().all(|value| {
            value.is_ascii_lowercase()
                || value.is_ascii_digit()
                || matches!(value, b'_' | b'-' | b'.')
        })
}

fn valid_path(path: &str) -> bool {
    path.bytes().all(|value| {
        value.is_ascii_lowercase()
            || value.is_ascii_digit()
            || matches!(value, b'_' | b'-' | b'/' | b'.')
    })
}

fn java_identifier_hash(namespace: &str, path: &str) -> i32 {
    java_string_hash(namespace)
        .wrapping_mul(31)
        .wrapping_add(java_string_hash(path))
}

fn java_string_hash(value: &str) -> i32 {
    value.bytes().fold(0_i32, |hash, character| {
        hash.wrapping_mul(31).wrapping_add(i32::from(character))
    })
}
