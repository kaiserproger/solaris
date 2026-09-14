//! Synthetic-fixture tests for the packaged content importer.
//!
//! Nothing here touches the network or Java: manifest/version parsing, cache
//! validation, discovery and atomic publish are all exercised on fixtures, and
//! the live derivation path stays a manual field run.

use std::path::Path;

use super::{
    ImportRequest, finish_import, java_version_banner, manifest_version_entry,
    parse_version_metadata, publish, require_target_version, resolve_or_import, stderr_tail,
    wanted_data_entry,
};
use crate::content_cache::{
    CONTENT_CACHE_ENV, ContentCache, ContentSearch, REQUIRED_FILES, REQUIRED_REGISTRY_PAYLOAD_DIR,
    discover_content_cache, validate_content_cache,
};

/// A minimal cache tree that passes validation: the version pin plus every
/// required artifact name. Contents are irrelevant to discovery.
fn write_valid_cache(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION,
            mc_protocol::PROTOCOL_VERSION,
        ),
    )
    .unwrap();
    for relative in REQUIRED_FILES {
        if *relative == "version.json" {
            continue;
        }
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        if relative.ends_with(".json") {
            std::fs::write(&path, "{}").unwrap();
        } else {
            std::fs::create_dir_all(&path).unwrap();
        }
    }
    std::fs::create_dir_all(root.join(REQUIRED_REGISTRY_PAYLOAD_DIR)).unwrap();
}

#[test]
fn manifest_entry_selection_is_pinned_and_hash_only() {
    // Shape observed live: the manifest entry carries id/type/url/sha1 and no
    // size, so selection must not require one.
    let manifest = r#"{
        "latest": {"release": "26.2", "snapshot": "26.3-rc-2"},
        "versions": [
            {"id":"26.3-rc-2","type":"snapshot","url":"https://piston-meta.mojang.com/v1/packages/ee/26.3-rc-2.json","sha1":"e19d6245fce40de8a9c929488d73b4dad55712c9","time":"2026-09-11T10:40:47+00:00"},
            {"id":"26.1.2","type":"release","url":"https://piston-meta.mojang.com/v1/packages/0d0f7a851642f3c81d1cff0a608aa048e5e8319f/26.1.2.json","sha1":"0d0f7a851642f3c81d1cff0a608aa048e5e8319f","time":"2026-09-11T06:42:29+00:00"}
        ]
    }"#;
    let entry = manifest_version_entry(manifest, "26.1.2").unwrap();
    assert_eq!(
        entry.url,
        "https://piston-meta.mojang.com/v1/packages/0d0f7a851642f3c81d1cff0a608aa048e5e8319f/26.1.2.json"
    );
    assert_eq!(entry.sha1, "0d0f7a851642f3c81d1cff0a608aa048e5e8319f");

    let error = manifest_version_entry(manifest, "26.9").unwrap_err();
    assert!(error.to_string().contains("does not list 26.9"), "{error}");
}

#[test]
fn version_metadata_reads_the_server_artifact() {
    let body = r#"{
        "id": "26.1.2",
        "downloads": {
            "client": {"sha1":"4e618f09a0c649dde3fdf829df443ce0b8831e65","size":38113927,"url":"https://piston-data.mojang.com/v1/objects/4e618f09a0c649dde3fdf829df443ce0b8831e65/client.jar"},
            "server": {"sha1":"97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51","size":60417480,"url":"https://piston-data.mojang.com/v1/objects/97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51/server.jar"}
        }
    }"#;
    let metadata = parse_version_metadata(body).unwrap();
    assert_eq!(metadata.id, "26.1.2");
    let server = metadata.server.expect("server artifact");
    assert_eq!(server.size, 60_417_480);
    assert_eq!(server.sha1, "97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51");

    // A version JSON without a server artifact is a hard error at import time,
    // never a silent substitution.
    let client_only = parse_version_metadata(r#"{"id":"26.1.2","downloads":{}}"#).unwrap();
    assert!(client_only.server.is_none());
}

#[test]
fn validate_rejects_an_incomplete_cache_by_name() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_cache(dir.path());
    validate_content_cache(dir.path()).unwrap();

    // A partial derivation (datagen without the wire capture) is refused.
    std::fs::remove_dir_all(dir.path().join(REQUIRED_REGISTRY_PAYLOAD_DIR)).unwrap();
    let error = validate_content_cache(dir.path()).unwrap_err();
    assert!(
        format!("{error:#}").contains(REQUIRED_REGISTRY_PAYLOAD_DIR),
        "{error:#}"
    );

    let partial = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(partial.path()).unwrap();
    let error = validate_content_cache(partial.path()).unwrap_err();
    assert!(format!("{error:#}").contains("version.json"), "{error:#}");
}

#[test]
fn discovery_prefers_the_explicit_override_and_never_falls_through() {
    let explicit = tempfile::tempdir().unwrap();
    let standard = tempfile::tempdir().unwrap();
    let cache_dir = standard.path().join("data").join("vanilla");
    write_valid_cache(&cache_dir);

    // No explicit override: the standard location wins.
    let search = ContentSearch {
        explicit: None,
        environment: None,
        standard: vec![cache_dir.clone()],
    };
    let found = discover_content_cache(&search).unwrap();
    assert_eq!(found.root(), cache_dir);

    // An explicit override that is not a valid cache is an error, even though a
    // later candidate would have been usable: falling through would silently
    // change content.
    let broken = explicit.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    let search = ContentSearch {
        explicit: Some(broken),
        environment: None,
        standard: vec![cache_dir],
    };
    let error = discover_content_cache(&search).unwrap_err();
    assert!(
        format!("{error:#}").contains("configured vanilla content cache"),
        "{error:#}"
    );
}

#[test]
fn discovery_failure_names_the_prerequisite_locations_and_command() {
    let missing_a = tempfile::tempdir().unwrap().path().join("nowhere-a");
    let missing_b = tempfile::tempdir().unwrap().path().join("nowhere-b");
    let search = ContentSearch {
        explicit: None,
        environment: None,
        standard: vec![missing_a.clone(), missing_b.clone()],
    };
    let error = format!("{:#}", discover_content_cache(&search).unwrap_err());
    assert!(error.contains("no vanilla content cache found"), "{error}");
    assert!(error.contains(&missing_a.display().to_string()), "{error}");
    assert!(error.contains(&missing_b.display().to_string()), "{error}");
    assert!(error.contains("mc-server content import"), "{error}");
    assert!(error.contains("JDK 25"), "{error}");
}

#[test]
fn import_target_follows_the_override_then_environment_then_standard() {
    let explicit = Path::new("/tmp/explicit-cache");
    let environment = Path::new("/tmp/environment-cache").to_path_buf();
    let standard = Path::new("/tmp/standard-cache").to_path_buf();

    let with_override = ContentSearch {
        explicit: Some(explicit.to_path_buf()),
        environment: Some(environment.clone()),
        standard: vec![standard.clone()],
    };
    assert_eq!(with_override.import_target(), explicit);

    let with_environment = ContentSearch {
        explicit: None,
        environment: Some(environment.clone()),
        standard: vec![standard.clone()],
    };
    assert_eq!(with_environment.import_target(), environment);

    let with_standard = ContentSearch {
        explicit: None,
        environment: None,
        standard: vec![standard.clone()],
    };
    assert_eq!(with_standard.import_target(), standard);
}

#[test]
fn valid_cache_is_reused_without_importing() {
    let dir = tempfile::tempdir().unwrap();
    write_valid_cache(dir.path());
    let search = ContentSearch {
        explicit: Some(dir.path().to_path_buf()),
        environment: None,
        standard: Vec::new(),
    };
    // The reuse path is pure discovery: it never reaches the importer.
    let cache: ContentCache = discover_content_cache(&search).unwrap();
    assert_eq!(cache.root(), dir.path());
}

#[test]
fn publish_replaces_the_previous_cache_atomically() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("content");
    write_valid_cache(&cache);
    let marker = cache.join("previous-marker");
    std::fs::write(&marker, "old").unwrap();

    let staging = parent.path().join(".content.import");
    write_valid_cache(&staging);
    assert!(!staging.join("previous-marker").exists());

    publish(&staging, &cache).unwrap();
    assert!(cache.join("version.json").is_file());
    assert!(
        !cache.join("previous-marker").exists(),
        "the published cache must be the staged tree"
    );
    assert!(!staging.exists(), "staging is consumed by the publish");
    assert!(
        !parent.path().join(".content.previous").exists(),
        "the backup is removed once the swap succeeded"
    );
}

#[test]
fn data_subset_keeps_only_consumed_directories() {
    assert_eq!(
        wanted_data_entry("data/minecraft/worldgen/structure_set/villages.json"),
        Some("worldgen/structure_set/villages.json".into())
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/structure/village/plains/houses/x.nbt"),
        Some("structure/village/plains/houses/x.nbt".into())
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/tags/block/mineable/pickaxe.json"),
        Some("tags/block/mineable/pickaxe.json".into())
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/recipe/oak_planks.json"),
        Some("recipe/oak_planks.json".into())
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/loot_table/chests/village.json"),
        Some("loot_table/chests/village.json".into())
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/dimension_type/overworld.json"),
        Some("dimension_type/overworld.json".into())
    );
    // Un-consumed data stays out of the cache.
    assert_eq!(
        wanted_data_entry("data/minecraft/advancement/husbandry/x.json"),
        None
    );
    assert_eq!(
        wanted_data_entry("data/minecraft/worldgen/noise/x.json"),
        None
    );
    assert_eq!(wanted_data_entry("assets/minecraft/lang/en_us.json"), None);
    assert_eq!(wanted_data_entry("META-INF/MANIFEST.MF"), None);
}

#[test]
fn import_rejects_a_foreign_version() {
    let error = require_target_version("26.2").unwrap_err();
    assert!(error.to_string().contains("targets"), "{error}");
    require_target_version(mc_protocol::TARGET_RELEASE).unwrap();
    assert_eq!(
        ImportRequest::automatic(&ContentSearch::default()).version,
        mc_protocol::TARGET_RELEASE
    );
}

#[test]
fn failed_derivation_leaves_the_previous_cache_intact() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("content");
    write_valid_cache(&cache);
    std::fs::write(
        cache.join("version.json"),
        r#"{"id":"26.1.2","world_version":4790,"protocol_version":775}"#,
    )
    .unwrap();
    let before = std::fs::read(cache.join("version.json")).unwrap();

    // A derivation failure (download, Java or validation) funnels through the
    // same driver as a real import; the live cache must be untouched and the
    // partial staging tree discarded.
    let staging = parent.path().join(".content.import");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("partial"), "half-derived").unwrap();
    let error = finish_import(
        &cache,
        &staging,
        Err(anyhow::anyhow!("java: datagen failed")),
    )
    .unwrap_err();
    assert!(error.to_string().contains("datagen failed"), "{error}");
    assert!(
        !staging.exists(),
        "a failed derivation must discard its staging tree"
    );
    assert_eq!(
        std::fs::read(cache.join("version.json")).unwrap(),
        before,
        "the previous cache must survive a failed import"
    );
    assert!(
        cache.join(REQUIRED_REGISTRY_PAYLOAD_DIR).is_dir(),
        "the previous cache stays complete"
    );
}

/// The real startup entry point, not just discovery: a complete cache is
/// returned as-is and the importer is never entered.
///
/// "Never entered" is proven positively: the exact staging path the importer
/// would create is occupied by a file, so any import attempt would fail (and
/// `staging_dir` would also have to remove it), and the parent directory must
/// still hold exactly the cache plus that tripwire.
#[tokio::test]
async fn resolve_or_import_reuses_a_valid_cache_without_running_the_importer() {
    let parent = tempfile::tempdir().unwrap();
    let cache = parent.path().join("content");
    write_valid_cache(&cache);
    std::fs::write(cache.join("marker"), "live-cache").unwrap();

    let tripwire = parent
        .path()
        .join(format!(".content.import.{}", std::process::id()));
    std::fs::write(&tripwire, "tripwire").unwrap();
    let tripwire_name = tripwire.file_name().unwrap().to_string_lossy().into_owned();

    let search = ContentSearch {
        explicit: Some(cache.clone()),
        environment: None,
        standard: Vec::new(),
    };
    let resolved = resolve_or_import(&search).await.unwrap();
    assert_eq!(resolved.root(), cache);
    assert!(
        cache.join("marker").is_file(),
        "the live cache must be untouched"
    );
    assert!(
        tripwire.is_file(),
        "an import attempt would have removed the staging tripwire"
    );

    let mut entries: Vec<String> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    let mut expected = vec![tripwire_name, "content".to_owned()];
    expected.sort();
    assert_eq!(
        entries, expected,
        "no staging or work directory may appear beside a reused cache"
    );
}

/// The real startup entry point on a cold machine whose import cannot proceed:
/// the error carries the discovery failure (prerequisite, searched locations,
/// command, JDK requirement) plus the concrete import cause, and nothing is
/// published. The import is stopped before any network or Java work by making
/// the cache target sit under a regular file.
#[tokio::test]
async fn resolve_or_import_reports_the_missing_cache_and_publishes_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let blocker = parent.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let target = blocker.join("content");

    let search = ContentSearch {
        explicit: None,
        environment: None,
        standard: vec![target.clone()],
    };
    let error = resolve_or_import(&search).await.unwrap_err();
    let text = format!("{error:#}");
    assert!(text.contains("no vanilla content cache found"), "{text}");
    assert!(
        text.contains(&target.display().to_string()),
        "the failure must name every searched location: {text}"
    );
    assert!(text.contains("mc-server content import"), "{text}");
    assert!(text.contains("JDK 25"), "{text}");
    assert!(
        text.contains("automatic vanilla content import failed"),
        "the concrete import cause must survive: {text}"
    );
    assert!(!target.exists(), "a failed import must publish nothing");
    assert_eq!(
        std::fs::read_to_string(&blocker).unwrap(),
        "not a directory"
    );
    let entries: Vec<String> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["blocker".to_owned()]);
}

#[test]
fn java_version_banner_reads_the_reported_version() {
    // Both shapes a JDK prints; the preflight feeds this to the shared parser.
    let modern = "openjdk version \"25.0.2\" 2026-01-15\nOpenJDK Runtime Environment\n";
    assert_eq!(
        java_version_banner(modern).unwrap(),
        "openjdk version \"25.0.2\" 2026-01-15"
    );
    let legacy = "java version \"1.8.0_402\"\n";
    assert_eq!(
        java_version_banner(legacy).unwrap(),
        "java version \"1.8.0_402\""
    );
    assert_eq!(java_version_banner("\n\n"), None);
}

#[test]
fn stderr_tail_keeps_the_real_cause() {
    // The JVM's own diagnostic must survive into the failure text; a long
    // stderr keeps its tail rather than being dropped or truncated mid-char.
    assert_eq!(
        stderr_tail(b"Error: LinkageError occurred\n"),
        "Error: LinkageError occurred"
    );
    assert_eq!(stderr_tail(b"   \n"), "(no output on stderr)");
    let long = "x".repeat(super::JVM_STDERR_TAIL_BYTES + 512);
    let tail = stderr_tail(long.as_bytes());
    assert_eq!(tail.len(), super::JVM_STDERR_TAIL_BYTES + "…".len());
    let mut text = String::new();
    for _ in 0..super::JVM_STDERR_TAIL_BYTES {
        text.push('é');
    }
    let tail = stderr_tail(text.as_bytes());
    assert!(tail.starts_with('…'));
    assert!(tail.chars().skip(1).all(|character| character == 'é'));
}

#[test]
fn content_cache_env_constant_is_documented() {
    assert_eq!(CONTENT_CACHE_ENV, "SOLARIS_CONTENT_CACHE");
}
