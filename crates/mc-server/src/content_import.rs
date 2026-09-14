//! The packaged vanilla-content importer.
//!
//! One code path produces the content cache the server runs on, shared by
//! `solaris content import` and the automatic pre-listener import on a normal
//! launch. It resolves the pinned release through Mojang's public version
//! manifest, downloads the server bundle with the manifest's size and sha1
//! verified before anything is staged, derives the cache exactly as the local
//! extraction tooling does (jar data subset, the server's own datagen, the
//! three Solaris Java extractors, and a wire-accurate `RegistryData` capture),
//! validates the result, and publishes it atomically. Any failure leaves the
//! previous valid cache untouched.
//!
//! Nothing here reads a repo shell script and nothing rewrites existing content
//! in place: the installer is the only writer, and it writes a staging tree
//! beside the cache before swapping.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};

use crate::content_cache::{ContentCache, ContentSearch, discover_content_cache};

/// Mojang's public launcher metadata; the same endpoints a launcher uses.
const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Java major version the pinned release's bundle declares.
const REQUIRED_JAVA_MAJOR: u32 = 25;

/// How much of a failing JVM's stderr is folded into the error.
const JVM_STDERR_TAIL_BYTES: usize = 2048;

/// Timeout for launching the bundled vanilla server during the wire capture.
const ORACLE_STARTUP_TIMEOUT: Duration = Duration::from_secs(180);

/// Stream bytes between two reported download checkpoints. The checkpoints are
/// driven by the artifact stream itself, never by a timer.
const DOWNLOAD_PROGRESS_BYTES: u64 = 8 * 1024 * 1024;

/// `data/minecraft` subdirectories the Configuration registries need.
const REGISTRY_DIRS: &[&str] = &[
    "banner_pattern",
    "cat_sound_variant",
    "cat_variant",
    "chat_type",
    "chicken_sound_variant",
    "chicken_variant",
    "cow_sound_variant",
    "cow_variant",
    "damage_type",
    "dialog",
    "dimension_type",
    "enchantment",
    "frog_variant",
    "instrument",
    "jukebox_song",
    "painting_variant",
    "pig_sound_variant",
    "pig_variant",
    "test_environment",
    "test_instance",
    "timeline",
    "trim_material",
    "trim_pattern",
    "wolf_sound_variant",
    "wolf_variant",
    "world_clock",
    "zombie_nautilus_variant",
];

/// `data/minecraft/worldgen` subdirectories worldgen reads.
const WORLDGEN_DIRS: &[&str] = &[
    "biome",
    "configured_feature",
    "placed_feature",
    "structure",
    "structure_set",
    "template_pool",
    "processor_list",
    "multi_noise_biome_source_parameter_list",
];

/// The Java extractors Solaris compiles against the bundle's classpath. Their
/// sources are the repo's own tooling, kept in-tree rather than shelled.
const JAVA_EXTRACTORS: &[JavaExtractor] = &[
    JavaExtractor {
        class_name: "LightExtractor",
        source: include_str!("../../../tools/extract-block-light/LightExtractor.java"),
        output_relative: "reports/block_light.json",
    },
    JavaExtractor {
        class_name: "MiningExtractor",
        source: include_str!("../../../tools/extract-block-mining/MiningExtractor.java"),
        output_relative: "reports/block_mining.json",
    },
    JavaExtractor {
        class_name: "ExplosionExtractor",
        source: include_str!("../../../tools/extract-block-explosion/ExplosionExtractor.java"),
        output_relative: "reports/block_explosion.json",
    },
];

struct JavaExtractor {
    class_name: &'static str,
    source: &'static str,
    output_relative: &'static str,
}

/// Where to take the licensed artifact from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentSource {
    /// Fetch through Mojang's public metadata and CDN.
    Download,
    /// Use an operator-provided server bundle jar.
    LocalJar(PathBuf),
}

/// One `content import` request.
#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub version: String,
    pub source: ContentSource,
    pub cache: PathBuf,
}

impl ImportRequest {
    /// The request a normal launch runs when no cache is usable: download the
    /// pinned release into the preferred cache location.
    #[must_use]
    pub fn automatic(search: &ContentSearch) -> Self {
        Self {
            version: mc_protocol::TARGET_RELEASE.to_owned(),
            source: ContentSource::Download,
            cache: search.import_target(),
        }
    }
}

/// What a completed import published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub cache: PathBuf,
    pub registries: usize,
    pub entries: usize,
}

/// Report one import step to the operator watching the terminal.
///
/// The tracing layers of a normal launch write `logs/latest.log` and
/// `logs/debug.log` and the interactive console pane reads its own ring, so
/// without this the automatic import would show nothing at all until the
/// listener binds — which is exactly the "is it dead?" startup the owner hit.
/// Every line of the import goes through this one sink, a complete line per
/// event, flushed as it is written, so the sequence on the terminal is the
/// sequence the import ran in: no buffering, no interleaving, no timer.
fn report(message: &str) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "content import: {message}");
    let _ = stderr.flush();
}

/// MiB as the operator sees a download size.
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// The checkpoint line for a download that has received `written` of `total`
/// bytes.
fn download_progress(written: u64, total: u64) -> String {
    format!(
        "downloaded {:.1} MiB / {:.1} MiB ({}%)",
        mib(written),
        mib(total),
        written.saturating_mul(100).checked_div(total).unwrap_or(0),
    )
}

/// Resolve the cache a launch runs on, importing it automatically when none is
/// valid. An already-valid cache is reused without touching the network.
pub async fn resolve_or_import(search: &ContentSearch) -> Result<ContentCache> {
    match discover_content_cache(search) {
        Ok(cache) => Ok(cache),
        Err(discovery) => {
            let request = ImportRequest::automatic(search);
            tracing::info!(
                cache = %request.cache.display(),
                version = request.version,
                "no valid vanilla content cache found; importing automatically",
            );
            report(&format!(
                "no usable vanilla content cache at {}; importing {} into it now, and a first \
                 run downloads the server bundle and can take a minute or longer",
                request.cache.display(),
                request.version,
            ));
            let started = Instant::now();
            let imported = import_content(&request).await.with_context(|| {
                format!("automatic vanilla content import failed after: {discovery:#}")
            })?;
            tracing::info!(
                cache = %imported.cache.display(),
                registries = imported.registries,
                entries = imported.entries,
                "vanilla content cache imported",
            );
            report(&format!(
                "published {} (registries={} entries={}) in {:.1}s",
                imported.cache.display(),
                imported.registries,
                imported.entries,
                started.elapsed().as_secs_f64(),
            ));
            Ok(ContentCache::from_validated(imported.cache))
        }
    }
}

/// Run one import: derive a complete cache from the licensed artifact and
/// publish it atomically, leaving any previous cache intact on failure.
pub async fn import_content(request: &ImportRequest) -> Result<ImportReport> {
    require_target_version(&request.version)?;
    let staging = staging_dir(&request.cache)?;
    let derived = derive_into(request, &staging).await;
    finish_import(&request.cache, &staging, derived)
}

/// Publish a staging tree produced by a derivation, or discard it and leave the
/// live cache exactly as it was. Every derivation failure funnels through here,
/// so the previous cache is never touched before a successful derive.
fn finish_import(
    cache: &Path,
    staging: &Path,
    derived: Result<ImportReport>,
) -> Result<ImportReport> {
    let imported = match derived {
        Ok(report) => report,
        Err(error) => {
            let _ = fs::remove_dir_all(staging);
            return Err(error);
        }
    };
    report(&format!("publishing {} atomically", cache.display()));
    publish(staging, cache)?;
    Ok(ImportReport {
        cache: cache.to_path_buf(),
        ..imported
    })
}

/// Solaris derives content for exactly one release.
fn require_target_version(version: &str) -> Result<()> {
    ensure!(
        version == mc_protocol::TARGET_RELEASE,
        "Solaris targets {} and cannot derive content for {}",
        mc_protocol::TARGET_RELEASE,
        version,
    );
    Ok(())
}

/// A staging tree beside the cache, so the final rename cannot cross devices.
fn staging_dir(cache: &Path) -> Result<PathBuf> {
    let parent = cache.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("create content cache parent {}", parent.display()))?;
    let staging = parent.join(format!(
        ".{}.import.{}",
        cache
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("content"),
        std::process::id(),
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .with_context(|| format!("clear stale staging {}", staging.display()))?;
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("create staging {}", staging.display()))?;
    Ok(staging)
}

async fn derive_into(request: &ImportRequest, staging: &Path) -> Result<ImportReport> {
    // Every JVM below runs with a scratch working directory of its own, so from
    // here on every path handed to one is absolute.
    let staging = fs::canonicalize(staging)
        .with_context(|| format!("resolve the staging tree {}", staging.display()))?;
    let staging = staging.as_path();
    // The licensed jar is scratch, never part of the published cache.
    let work = staging.join(".work");
    fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;
    // A JVM that cannot load the bundle's class files would fail deep inside the
    // derivation with an opaque exit status, so the runtime is validated first:
    // nothing is downloaded and no JVM is started against a wrong runtime.
    report(&format!(
        "checking the Java runtime (deriving {} needs JDK {REQUIRED_JAVA_MAJOR})",
        mc_protocol::TARGET_RELEASE,
    ));
    let java = preflight_java()?;
    let javac = javac_command(&java);
    report(&format!(
        "resolving {} through Mojang's public version manifest",
        request.version,
    ));
    let metadata = resolve_version_metadata(&request.version).await?;
    let jar = match &request.source {
        ContentSource::LocalJar(path) => {
            report(&format!(
                "verifying the supplied server jar {}",
                path.display()
            ));
            verify_local_jar(path, metadata.server.as_ref(), &work)?
        }
        ContentSource::Download => download_server_jar(&metadata, &work).await?,
    };

    report("unpacking the bundle and installing the data/minecraft subset");
    let bundle = unpack_bundle(&jar, &work)?;
    install_version_json(&jar, staging)?;
    copy_data_subset(&bundle.inner_jar, staging)?;
    run_datagen(&java, &jar, &work, staging)?;
    run_java_extractors(&java, &javac, &bundle, &work, staging)?;
    let captured = capture_registry_payloads(&java, &jar, &work, staging).await?;

    fs::remove_dir_all(&work).with_context(|| format!("remove {}", work.display()))?;
    report("validating the derived cache");
    validate_staged_cache(staging)?;
    let entries = captured.values().map(|names| names.len()).sum();
    Ok(ImportReport {
        cache: staging.to_path_buf(),
        registries: captured.len(),
        entries,
    })
}

/// The pinned release's metadata as the manifest states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMetadata {
    pub id: String,
    pub server: Option<Artifact>,
}

/// One downloadable artifact with the manifest's verification data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

/// Resolve the pinned version through the public manifest and verify it.
async fn resolve_version_metadata(version: &str) -> Result<VersionMetadata> {
    let client = http_client()?;
    let manifest = get_text(&client, VERSION_MANIFEST_URL).await?;
    let entry = manifest_version_entry(&manifest, version)?;
    let body = get_text_verified(&client, &entry.url, &entry.sha1).await?;
    parse_version_metadata(&body)
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("solaris-content-import")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()
        .context("building the content-import HTTP client")
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    ensure!(status.is_success(), "fetching {url} returned {status}");
    response
        .text()
        .await
        .with_context(|| format!("reading the body of {url}"))
}

async fn get_text_verified(client: &reqwest::Client, url: &str, sha1: &str) -> Result<String> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    let status = response.status();
    ensure!(status.is_success(), "fetching {url} returned {status}");
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("reading the body of {url}"))?;
    let actual = sha1_hex(&bytes);
    ensure!(
        actual == sha1,
        "{url} failed verification: expected sha1 {sha1}, got {actual}"
    );
    String::from_utf8(bytes.to_vec()).with_context(|| format!("{url} is not UTF-8"))
}

/// One version entry from the public manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub url: String,
    pub sha1: String,
}

/// Find `version` in the public manifest body.
pub fn manifest_version_entry(manifest: &str, version: &str) -> Result<ManifestEntry> {
    let value: serde_json::Value =
        serde_json::from_str(manifest).context("parsing the Mojang version manifest")?;
    let versions = value
        .get("versions")
        .and_then(|versions| versions.as_array())
        .context("the Mojang version manifest has no versions array")?;
    let entry = versions
        .iter()
        .find(|entry| entry.get("id").and_then(|id| id.as_str()) == Some(version))
        .with_context(|| format!("Mojang's version manifest does not list {version}"))?;
    let url = entry
        .get("url")
        .and_then(|url| url.as_str())
        .context("the manifest version entry has no url")?;
    let sha1 = entry
        .get("sha1")
        .and_then(|sha1| sha1.as_str())
        .context("the manifest version entry has no sha1")?;
    Ok(ManifestEntry {
        url: url.to_owned(),
        sha1: sha1.to_owned(),
    })
}

/// Parse a version's metadata JSON into the artifacts Solaris stages.
pub fn parse_version_metadata(body: &str) -> Result<VersionMetadata> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("parsing the version metadata JSON")?;
    let id = value
        .get("id")
        .and_then(|id| id.as_str())
        .context("the version metadata has no id")?;
    let server = value
        .get("downloads")
        .and_then(|downloads| downloads.get("server"))
        .map(parse_artifact)
        .transpose()?;
    Ok(VersionMetadata {
        id: id.to_owned(),
        server,
    })
}

fn parse_artifact(value: &serde_json::Value) -> Result<Artifact> {
    let url = value
        .get("url")
        .and_then(|url| url.as_str())
        .context("the artifact has no url")?;
    let sha1 = value
        .get("sha1")
        .and_then(|sha1| sha1.as_str())
        .context("the artifact has no sha1")?;
    let size = value
        .get("size")
        .and_then(|size| size.as_u64())
        .context("the artifact has no size")?;
    Ok(Artifact {
        url: url.to_owned(),
        sha1: sha1.to_owned(),
        size,
    })
}

async fn download_server_jar(metadata: &VersionMetadata, staging: &Path) -> Result<PathBuf> {
    let artifact = metadata.server.as_ref().with_context(|| {
        format!(
            "Mojang's metadata for {} exposes no downloads.server artifact",
            metadata.id
        )
    })?;
    let dest = staging.join("server.jar");
    let client = http_client()?;
    tracing::info!(
        url = artifact.url,
        bytes = artifact.size,
        "downloading server bundle"
    );
    report(&format!(
        "downloading the server bundle ({:.1} MiB)",
        mib(artifact.size),
    ));
    let response = client
        .get(&artifact.url)
        .send()
        .await
        .with_context(|| format!("downloading the server bundle from {}", artifact.url))?;
    let status = response.status();
    ensure!(
        status.is_success(),
        "downloading the server bundle from {} returned {status}",
        artifact.url
    );
    let mut file = File::create(&dest).with_context(|| format!("create {}", dest.display()))?;
    let mut hasher = Sha1::new();
    let mut written: u64 = 0;
    let mut reported: u64 = 0;
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("reading the server bundle from {}", artifact.url))?
    {
        written += chunk.len() as u64;
        ensure!(
            written <= artifact.size,
            "the server bundle from {} exceeded its manifest size {}",
            artifact.url,
            artifact.size
        );
        hasher.update(&chunk);
        file.write_all(&chunk)
            .with_context(|| format!("write {}", dest.display()))?;
        if written - reported >= DOWNLOAD_PROGRESS_BYTES {
            reported = written;
            report(&download_progress(written, artifact.size));
        }
    }
    if reported < written {
        report(&download_progress(written, artifact.size));
    }
    file.flush()?;
    ensure!(
        written == artifact.size,
        "the server bundle from {} has {written} bytes but the manifest says {}",
        artifact.url,
        artifact.size
    );
    let actual = hex(&hasher.finalize());
    ensure!(
        actual == artifact.sha1,
        "the server bundle from {} failed verification: expected sha1 {}, got {actual}",
        artifact.url,
        artifact.sha1
    );
    Ok(dest)
}

/// Stage an operator-provided jar, checking it against the manifest when the
/// manifest describes a server artifact for the pinned release.
fn verify_local_jar(path: &Path, expected: Option<&Artifact>, staging: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_file(),
        "the supplied jar {} does not exist",
        path.display()
    );
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if let Some(expected) = expected {
        ensure!(
            bytes.len() as u64 == expected.size,
            "the supplied jar {} has {} bytes but {} expects {}",
            path.display(),
            bytes.len(),
            expected.url,
            expected.size
        );
        let actual = sha1_hex(&bytes);
        ensure!(
            actual == expected.sha1,
            "the supplied jar {} failed verification for {}: expected sha1 {}, got {actual}",
            path.display(),
            expected.url,
            expected.sha1
        );
    }
    let dest = staging.join("server.jar");
    fs::write(&dest, &bytes).with_context(|| format!("stage {}", dest.display()))?;
    Ok(dest)
}

/// The unpacked bundle layout the derivation steps share.
struct UnpackedBundle {
    inner_jar: PathBuf,
    libraries: Vec<PathBuf>,
}

fn unpack_bundle(jar: &Path, work: &Path) -> Result<UnpackedBundle> {
    let bundle_dir = work.join("bundle");
    extract_zip(jar, &bundle_dir).with_context(|| format!("unpack {}", jar.display()))?;
    let versions_dir = bundle_dir.join("META-INF").join("versions");
    let inner_jar = find_inner_jar(&versions_dir)?;
    let libraries = collect_jars(&bundle_dir.join("META-INF").join("libraries"))?;
    Ok(UnpackedBundle {
        inner_jar,
        libraries,
    })
}

fn find_inner_jar(versions_dir: &Path) -> Result<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(versions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Ok(inner) = fs::read_dir(&path)
            {
                for inner in inner.flatten() {
                    let candidate = inner.path();
                    if candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("server-") && name.ends_with(".jar"))
                    {
                        found.push(candidate);
                    }
                }
            }
        }
    }
    found.sort();
    found.into_iter().next().with_context(|| {
        format!(
            "no META-INF/versions/*/server-*.jar inside the bundle at {}",
            versions_dir.display()
        )
    })
}

fn collect_jars(root: &Path) -> Result<Vec<PathBuf>> {
    let mut jars = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("jar") {
                jars.push(path);
            }
        }
    }
    jars.sort();
    Ok(jars)
}

fn install_version_json(jar: &Path, staging: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(
        File::open(jar).with_context(|| format!("open the server bundle {}", jar.display()))?,
    )
    .with_context(|| format!("read {} as ZIP", jar.display()))?;
    let mut entry = archive
        .by_name("version.json")
        .context("the server bundle has no version.json")?;
    let mut raw = Vec::new();
    entry
        .read_to_end(&mut raw)
        .context("read version.json from the server bundle")?;
    drop(entry);
    let parsed: serde_json::Value =
        serde_json::from_slice(&raw).context("parse version.json from the server bundle")?;
    let id = parsed
        .get("id")
        .and_then(|id| id.as_str())
        .context("version.json has no id")?;
    ensure!(
        id == mc_protocol::TARGET_RELEASE,
        "the supplied jar is {id} but Solaris targets {}",
        mc_protocol::TARGET_RELEASE
    );
    fs::write(staging.join("version.json"), &raw)
        .with_context(|| format!("write {}/version.json", staging.display()))?;
    Ok(())
}

fn copy_data_subset(inner_jar: &Path, staging: &Path) -> Result<()> {
    let minecraft = staging.join("data").join("minecraft");
    fs::create_dir_all(&minecraft).with_context(|| format!("create {}", minecraft.display()))?;
    let mut archive = zip::ZipArchive::new(
        File::open(inner_jar)
            .with_context(|| format!("open the inner server jar {}", inner_jar.display()))?,
    )
    .with_context(|| format!("read {} as ZIP", inner_jar.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("read entry {index} of {}", inner_jar.display()))?;
        let name = entry.name().to_owned();
        let Some(relative) = wanted_data_entry(&name) else {
            continue;
        };
        let dest = minecraft.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut out = File::create(&dest).with_context(|| format!("create {}", dest.display()))?;
        io::copy(&mut entry, &mut out).with_context(|| format!("write {}", dest.display()))?;
    }
    Ok(())
}

/// Whether one inner-jar entry is part of the consumed `data/minecraft` subset,
/// and the path it takes under `data/minecraft`.
fn wanted_data_entry(name: &str) -> Option<PathBuf> {
    let relative = name.strip_prefix("data/minecraft/")?;
    if relative.is_empty() {
        return Some(PathBuf::new());
    }
    let top = relative.split('/').next().unwrap_or_default();
    let keep = REGISTRY_DIRS.contains(&top)
        || ["structure", "tags", "recipe", "loot_table"].contains(&top)
        || (top == "worldgen"
            && relative
                .split('/')
                .nth(1)
                .is_some_and(|dir| WORLDGEN_DIRS.contains(&dir)));
    keep.then(|| PathBuf::from(relative))
}

fn run_datagen(java: &OsStr, jar: &Path, work: &Path, staging: &Path) -> Result<()> {
    let datagen = work.join("datagen");
    fs::create_dir_all(&datagen).with_context(|| format!("create {}", datagen.display()))?;
    tracing::info!("running the server bundle's own datagen for reports");
    report("running the server bundle's own datagen for reports");
    let output = Command::new(java)
        .arg("-DbundlerMainClass=net.minecraft.data.Main")
        .arg("-jar")
        .arg(jar)
        .arg("--server")
        .arg("--reports")
        .current_dir(&datagen)
        .output()
        .with_context(|| java_prerequisite(java))?;
    ensure!(
        output.status.success(),
        "the server bundle's datagen failed with {}: {}",
        output.status,
        stderr_tail(&output.stderr),
    );
    let generated = datagen.join("generated").join("reports");
    let reports = staging.join("reports");
    fs::create_dir_all(&reports).with_context(|| format!("create {}", reports.display()))?;
    for name in ["blocks.json", "registries.json", "packets.json"] {
        let source = generated.join(name);
        ensure!(
            source.is_file(),
            "the server bundle's datagen did not produce {name}"
        );
        fs::copy(&source, reports.join(name)).with_context(|| format!("install reports/{name}"))?;
    }
    let components = generated.join("minecraft").join("components").join("item");
    if components.is_dir() {
        copy_tree(
            &components,
            &reports.join("minecraft").join("components").join("item"),
        )?;
    }
    fs::remove_dir_all(&datagen).with_context(|| format!("remove {}", datagen.display()))?;
    Ok(())
}

fn run_java_extractors(
    java: &OsStr,
    javac: &OsStr,
    bundle: &UnpackedBundle,
    work: &Path,
    staging: &Path,
) -> Result<()> {
    let classpath = classpath(bundle);
    let sources = work.join("extractors");
    let classes = sources.join("classes");
    fs::create_dir_all(&classes).with_context(|| format!("create {}", classes.display()))?;
    report("running the in-tree Java extractors");
    for extractor in JAVA_EXTRACTORS {
        let source = sources.join(format!("{}.java", extractor.class_name));
        fs::write(&source, extractor.source)
            .with_context(|| format!("write {}", source.display()))?;
        let compile = Command::new(javac)
            .arg("-d")
            .arg(&classes)
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            // The extractors bootstrap vanilla, whose logging config rotates
            // `logs/latest.log` in the working directory. Inheriting the
            // launcher's directory would rotate the operator's own log file out
            // from under the running process, so they get this scratch one.
            .current_dir(&sources)
            .output()
            .with_context(|| java_prerequisite(javac))?;
        ensure!(
            compile.status.success(),
            "compiling {} failed with {}: {}",
            extractor.class_name,
            compile.status,
            stderr_tail(&compile.stderr),
        );
        let out = staging.join(extractor.output_relative);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let run = Command::new(java)
            .arg("-cp")
            .arg(format!("{}:{classpath}", classes.display()))
            .arg(extractor.class_name)
            .arg(&out)
            .current_dir(&sources)
            .output()
            .with_context(|| java_prerequisite(java))?;
        ensure!(
            run.status.success(),
            "{} failed with {}: {}",
            extractor.class_name,
            run.status,
            stderr_tail(&run.stderr),
        );
    }
    fs::remove_dir_all(&sources).with_context(|| format!("remove {}", sources.display()))?;
    Ok(())
}

/// The wire-equivalent capture is the packaged step owned by `mc-test-harness`
/// (`registry_capture`): it starts a temporary server from the same jar,
/// declines Known Packs, verifies the captured index against the staged
/// content, and publishes `reports/registry_network_nbt/**`. This importer only
/// hands it the staged tree.
async fn capture_registry_payloads(
    java: &OsStr,
    jar: &Path,
    work: &Path,
    staging: &Path,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>> {
    tracing::info!("capturing exact RegistryData payloads from a temporary vanilla server");
    report("capturing exact RegistryData payloads from a temporary vanilla server");
    mc_test_harness::registry_capture::capture_registry_payloads_from_jar(
        jar,
        work,
        staging,
        Path::new(java),
        ORACLE_STARTUP_TIMEOUT,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{error}"))
}

fn classpath(bundle: &UnpackedBundle) -> String {
    let mut parts = vec![bundle.inner_jar.display().to_string()];
    parts.extend(
        bundle
            .libraries
            .iter()
            .map(|path| path.display().to_string()),
    );
    parts.join(":")
}

fn java_command() -> Result<OsString> {
    Ok(std::env::var_os("JAVA").unwrap_or_else(|| "java".into()))
}

fn javac_command(java: &OsStr) -> OsString {
    if let Some(value) = std::env::var_os("JAVAC") {
        return value;
    }
    let sibling = Path::new(java).with_file_name("javac");
    if sibling.is_file() {
        return sibling.into_os_string();
    }
    "javac".into()
}

fn java_prerequisite(program: &OsStr) -> String {
    format!(
        "running {} failed; deriving vanilla content needs a JDK {REQUIRED_JAVA_MAJOR} on PATH \
         (set JAVA=/path/to/jdk-{REQUIRED_JAVA_MAJOR}/bin/java and \
         JAVAC=/path/to/jdk-{REQUIRED_JAVA_MAJOR}/bin/javac)",
        Path::new(program).display()
    )
}

/// Validate the selected runtime before any derivation work starts.
///
/// The bundle's classes are compiled for Java 25. An older JVM dies inside the
/// bundle with `UnsupportedClassVersionError`, which used to surface only as an
/// opaque datagen exit status, so the runtime is checked first: the failure
/// names the resolved executable, its observed version, the required major and
/// the override, and no JVM is ever started against an unusable runtime.
fn preflight_java() -> Result<OsString> {
    let java = java_command()?;
    let display = Path::new(&java).display().to_string();
    let override_hint = format!(
        "set JAVA=/path/to/jdk-{REQUIRED_JAVA_MAJOR}/bin/java and \
         JAVAC=/path/to/jdk-{REQUIRED_JAVA_MAJOR}/bin/javac"
    );
    let output = Command::new(&java)
        .arg("-version")
        .output()
        .with_context(|| format!("running `{display} -version` failed; {override_hint}"))?;
    let mut banner = String::from_utf8_lossy(&output.stdout).into_owned();
    banner.push_str(&String::from_utf8_lossy(&output.stderr));
    let observed = java_version_banner(&banner).unwrap_or_else(|| banner.trim().to_owned());
    match mc_test_harness::parity::parse_java_major_version(&banner) {
        Some(major) if major == REQUIRED_JAVA_MAJOR => Ok(java),
        Some(major) => bail!(
            "Java {major} at `{display}` cannot derive content for {}: the server bundle's \
             classes require Java {REQUIRED_JAVA_MAJOR} (observed `{observed}`). {override_hint}",
            mc_protocol::TARGET_RELEASE,
        ),
        None => bail!(
            "could not read a Java major version from `{display}` (observed `{observed}`); \
             this derivation requires Java {REQUIRED_JAVA_MAJOR}. {override_hint}"
        ),
    }
}

/// The first `version "…"` line of a `java -version` banner.
fn java_version_banner(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && line.contains("version"))
        .map(ToOwned::to_owned)
}

/// The tail of a failing JVM's stderr, so the real cause is never dropped
/// behind a bare exit status.
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(no output on stderr)".to_owned();
    }
    if trimmed.len() <= JVM_STDERR_TAIL_BYTES {
        return trimmed.to_owned();
    }
    let mut start = trimmed.len() - JVM_STDERR_TAIL_BYTES;
    while start < trimmed.len() && !trimmed.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &trimmed[start..])
}

fn extract_zip(archive_path: &Path, dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(
        File::open(archive_path).with_context(|| format!("open {}", archive_path.display()))?,
    )
    .with_context(|| format!("read {} as ZIP", archive_path.display()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .with_context(|| format!("read entry {index} of {}", archive_path.display()))?;
        let relative = entry.enclosed_name().with_context(|| {
            format!(
                "entry {index} of {} escapes the archive",
                archive_path.display()
            )
        })?;
        let out = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&out).with_context(|| format!("create {}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        let mut file = File::create(&out).with_context(|| format!("create {}", out.display()))?;
        io::copy(&mut entry, &mut file).with_context(|| format!("write {}", out.display()))?;
    }
    Ok(())
}

fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).with_context(|| format!("install {}", to.display()))?;
        }
    }
    Ok(())
}

/// Validate the staged tree before it becomes the live cache.
fn validate_staged_cache(staging: &Path) -> Result<()> {
    crate::content_cache::validate_content_cache(staging)
        .context("the derived vanilla content cache failed validation")?;
    let data = mc_data::load(staging)
        .with_context(|| format!("loading the derived cache at {}", staging.display()))?;
    ensure!(
        data.has_full_registry_payloads(),
        "the derived cache at {} is missing exact RegistryData payloads",
        staging.display()
    );
    Ok(())
}

/// Atomically replace `cache` with `staging`, keeping the previous tree until
/// the swap has succeeded.
fn publish(staging: &Path, cache: &Path) -> Result<()> {
    let parent = cache.parent().unwrap_or_else(|| Path::new("."));
    let name = cache
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("content");
    let backup = parent.join(format!(".{name}.previous"));
    if backup.exists() {
        fs::remove_dir_all(&backup).with_context(|| format!("clear {}", backup.display()))?;
    }
    let had_previous = cache.exists();
    if had_previous {
        fs::rename(cache, &backup)
            .with_context(|| format!("move the previous cache aside to {}", backup.display()))?;
    }
    match fs::rename(staging, cache) {
        Ok(()) => {
            if had_previous {
                fs::remove_dir_all(&backup)
                    .with_context(|| format!("remove {}", backup.display()))?;
            }
            Ok(())
        }
        Err(error) => {
            if had_previous {
                let _ = fs::rename(&backup, cache);
            }
            Err(error).with_context(|| format!("publish {}", cache.display()))
        }
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
#[path = "content_import_tests.rs"]
mod tests;
