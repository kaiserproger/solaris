//! # mc-test-harness
//!
//! Diff testing infrastructure.
//!
//! Part of the Solaris engine.
//!
//! ## Packaged vanilla RegistryData capture
//!
//! [`registry_capture`] is the wire-equivalent Configuration capture the content
//! importer runs as one of its steps. It needs no repository checkout, no
//! `cargo`, and no `tools/*.sh`, and the temporary server it starts is stopped
//! and its scratch tree removed on every path:
//!
//! ```text
//! capture_registry_payloads_from_jar(
//!     server_jar:      &Path,   // bundle jar for the pinned release
//!     scratch_dir:     &Path,   // scratch space for the temporary server
//!     content_root:    &Path,   // staged content; payloads land under
//!                               // content_root/reports/registry_network_nbt/**
//!     java:            &Path,   // java executable (absolute path or PATH name)
//!     startup_timeout: Duration,
//! ) -> Result<BTreeMap<String, BTreeSet<String>>, RegistryCaptureError>
//! ```
//!
//! `capture_registry_payloads(addr, staging_root)` is the same capture against a
//! server the caller already started. Failures come back as
//! [`registry_capture::RegistryCaptureError`], whose variants name the concrete
//! cause — no Java, server jar missing, staged content not indexable, server not
//! ready, server exited, handshake refused, payload missing, captured index
//! mismatch, installed payloads incomplete. See the module docs for the table.

pub mod client;
pub mod parity;
pub mod registry_capture;
pub mod replay;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn it_compiles() {
        assert_eq!(2 + 2, 4);
    }
}
