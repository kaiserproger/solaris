use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const PLUGIN_API_CRATES: &[&str] = &["mc-extension", "mc-script"];
const FORBIDDEN_API_TYPES: &[&str] = &[
    "WorldHandle",
    "SessionRegistry",
    "WorldStorage",
    "ShutdownHandle",
    "SaveHandle",
    "OutboundPressureHandle",
    "RuntimeControlPlane",
    "EntityStore",
    "ChunkScheduler",
];
const FORBIDDEN_API_TRANSPORTS: &[&str] = &[
    "TryRecvError",
    "std::sync::mpsc",
    "tokio::sync",
    "mpsc::Sender",
    "mpsc::Receiver",
    "SyncSender",
    "Receiver<",
    "Sender<",
    "Arc<Mutex",
    "Arc<RwLock",
    "parking_lot",
    "DashMap",
    "JoinHandle",
];
const FORBIDDEN_API_DEPENDENCIES: &[&str] = &[
    "mc-net",
    "mc-world",
    "mc-server",
    "mc-entity",
    "mc-physics",
    "mc-data",
    "mc-protocol",
    "mc-nbt",
    "mc-worldgen",
];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    path: PathBuf,
    line: usize,
    message: String,
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("code-health") => {
            if let Some(arg) = args.next() {
                eprintln!("code-health: unknown option {arg:?}");
                std::process::exit(2);
            }
            if let Err(code) = run_code_health() {
                std::process::exit(code);
            }
        }
        _ => {
            eprintln!("usage: cargo run -p xtask -- code-health");
            std::process::exit(2);
        }
    }
}

fn run_code_health() -> Result<(), i32> {
    let root = workspace_root().map_err(|err| {
        eprintln!("code-health: {err}");
        2
    })?;
    let mut findings = Vec::new();
    scan_rust_sources(&root.join("crates"), &mut findings);
    scan_api_manifests(&root, &mut findings);

    println!("Solaris code-health report");
    println!();

    if findings.is_empty() {
        println!("summary: 0 fail");
        println!("verdict: KEEP");
        return Ok(());
    }

    for finding in &findings {
        println!(
            "FAIL: {}:{}: {}",
            display_path(&root, &finding.path),
            finding.line,
            finding.message
        );
    }

    println!();
    println!("summary: {} fail", findings.len());
    println!("verdict: CLEANUP_REQUIRED");
    Err(1)
}

fn workspace_root() -> Result<PathBuf, String> {
    let mut dir = env::current_dir().map_err(|err| format!("current_dir failed: {err}"))?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("could not find workspace root with Cargo.toml and crates/".into());
        }
    }
}

fn scan_rust_sources(dir: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            scan_rust_sources(&path, findings);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            scan_rust_file(&path, findings);
        }
    }
}

fn scan_rust_file(path: &Path, findings: &mut Vec<Finding>) {
    let Ok(source) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = source.lines().collect();
    scan_generic_modules(path, &lines, findings);
    scan_api_leaks(path, &lines, findings);
}

fn scan_generic_modules(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if matches!(trimmed, "mod utils;" | "mod common;" | "mod shared;")
            || matches!(
                trimmed,
                "pub mod utils;" | "pub mod common;" | "pub mod shared;"
            )
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "generic utils/common/shared module; use a domain module".into(),
            });
        }
    }
}

fn scan_api_leaks(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let is_extension_api = PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name));
    if !is_extension_api {
        return;
    }

    scan_api_evolution_guards(path, lines, findings);

    let aliases = forbidden_api_aliases(lines);
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let publicish = trimmed.starts_with("pub ") || trimmed.starts_with("pub(");
        if !publicish {
            continue;
        }

        let mut public_item = String::new();
        for item_line in &lines[index..] {
            public_item.push_str(item_line);
            public_item.push('\n');
            if public_item_end(item_line) {
                break;
            }
        }

        if contains_forbidden_api_surface(&public_item, &aliases) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API exposes internal runtime or transport type".into(),
            });
        }
    }
}

fn scan_api_evolution_guards(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub enum ") && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public enum missing #[non_exhaustive]".into(),
            });
        }
        if public_struct_exposes_fields(lines, index) && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public field struct missing #[non_exhaustive]".into(),
            });
        }
    }
}

fn has_non_exhaustive_attr(lines: &[&str], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        if trimmed.starts_with("#[") {
            if trimmed == "#[non_exhaustive]" {
                return true;
            }
            continue;
        }
        break;
    }
    false
}

fn public_struct_exposes_fields(lines: &[&str], index: usize) -> bool {
    let trimmed = lines[index].trim_start();
    if !trimmed.starts_with("pub struct ") {
        return false;
    }

    if let Some((_, tuple_tail)) = trimmed.split_once('(') {
        if tuple_tail.trim_start().starts_with("pub ") {
            return true;
        }
        if tuple_tail.contains(");") || tuple_tail.trim_end().ends_with(';') {
            return false;
        }
        for line in lines.iter().skip(index + 1) {
            let trimmed = line.trim_start();
            if trimmed.starts_with("pub ") || trimmed.starts_with("pub(") {
                return true;
            }
            if trimmed.contains(");") || trimmed.ends_with(';') {
                break;
            }
        }
        return false;
    }

    let mut saw_brace = false;
    for (offset, line) in lines[index..].iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.contains('{') {
            saw_brace = true;
            if trimmed.contains("{ pub ") || trimmed.contains("{pub ") {
                return true;
            }
        }
        if saw_brace && offset > 0 && (trimmed.starts_with("pub ") || trimmed.starts_with("pub(")) {
            return true;
        }
        if saw_brace && trimmed.contains('}') {
            break;
        }
        if !saw_brace && trimmed.ends_with(';') {
            break;
        }
    }
    false
}

fn scan_api_manifests(root: &Path, findings: &mut Vec<Finding>) {
    for crate_name in PLUGIN_API_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let Ok(source) = fs::read_to_string(&manifest) else {
            continue;
        };
        scan_api_manifest(&manifest, &source, findings);
    }
}

fn scan_api_manifest(path: &Path, source: &str, findings: &mut Vec<Finding>) {
    if !PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name))
    {
        return;
    }

    for (index, line) in source.lines().enumerate() {
        let Some(dependency) = manifest_dependency_name(line.trim()) else {
            continue;
        };
        if FORBIDDEN_API_DEPENDENCIES.contains(&dependency) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: format!("plugin/script API depends on internal crate `{dependency}`"),
            });
        }
    }
}

fn manifest_dependency_name(line: &str) -> Option<&str> {
    if let Some(section) = line
        .strip_prefix("[dependencies.")
        .or_else(|| line.strip_prefix("[dev-dependencies."))
        .or_else(|| line.strip_prefix("[build-dependencies."))
    {
        return section.strip_suffix(']');
    }

    if line.starts_with('[') {
        return None;
    }

    let (name, _) = line.split_once('=')?;
    let name = name.trim().trim_matches('"');
    (!name.is_empty()).then_some(name)
}

fn forbidden_api_aliases(lines: &[&str]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::new();
    for line in lines {
        let trimmed = line.trim();
        if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
            continue;
        }
        if !contains_forbidden_api_surface(trimmed, &BTreeSet::new()) {
            continue;
        }
        if let Some((_, alias)) = trimmed.rsplit_once(" as ") {
            let alias = alias.trim_end_matches(';').trim();
            if !alias.is_empty() {
                aliases.insert(alias.to_owned());
            }
        }
    }
    aliases
}

fn contains_forbidden_api_surface(source: &str, aliases: &BTreeSet<String>) -> bool {
    FORBIDDEN_API_TYPES
        .iter()
        .chain(FORBIDDEN_API_TRANSPORTS.iter())
        .any(|token| source.contains(token))
        || aliases.iter().any(|alias| source.contains(alias))
}

fn public_item_end(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.ends_with(';')
        || trimmed.ends_with('{')
        || trimmed.ends_with('}')
        || trimmed.ends_with(',')
}

fn path_has_component(path: &Path, needle: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == needle)
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_module_names_are_failures() {
        let mut findings = Vec::new();
        scan_generic_modules(
            Path::new("crates/mc-net/src/lib.rs"),
            &["mod utils;", "pub mod common;", "mod gameplay;"],
            &mut findings,
        );

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[1].line, 2);
    }

    #[test]
    fn api_leak_rule_is_limited_to_plugin_api_crates() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &[
                "#[non_exhaustive]",
                "pub struct Api {",
                "pub world: WorldHandle,",
                "}",
            ],
            &mut findings,
        );
        scan_api_leaks(
            Path::new("crates/mc-net/src/lib.rs"),
            &["pub world: WorldHandle,"],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 3);
    }

    #[test]
    fn api_guard_rejects_multiline_public_runtime_handle_signature() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &[
                "pub fn api(",
                "    world: WorldHandle,",
                ") -> Result<(), ExtensionError>;",
            ],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn api_guard_rejects_public_runtime_handle_alias() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &[
                "use mc_net::server::WorldHandle as HostWorld;",
                "pub type ApiWorld = HostWorld;",
            ],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
    }

    #[test]
    fn api_guard_rejects_public_transport_errors() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &["pub fn try_recv_event(&self) -> Result<InboundEvent, TryRecvError> {"],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 1);
    }

    #[test]
    fn api_guard_rejects_internal_workspace_dependencies() {
        let mut findings = Vec::new();
        scan_api_manifest(
            Path::new("crates/mc-extension/Cargo.toml"),
            "[dependencies]\nmc-world = { path = \"../mc-world\" }\n",
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
    }

    #[test]
    fn api_guard_allows_current_extension_payload_dependency() {
        let mut findings = Vec::new();
        scan_api_manifest(
            Path::new("crates/mc-extension/Cargo.toml"),
            "[dependencies]\nbytes = { workspace = true }\n",
            &mut findings,
        );

        assert!(findings.is_empty());
    }

    #[test]
    fn api_guard_rejects_public_enums_without_non_exhaustive() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-script/src/lib.rs"),
            &[
                "#[derive(Debug, Clone, PartialEq, Eq)]",
                "pub enum ScriptCommand {",
                "    BroadcastChatMessage { message: String },",
                "}",
            ],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
        assert_eq!(
            findings[0].message,
            "plugin/script API public enum missing #[non_exhaustive]"
        );
    }

    #[test]
    fn api_guard_rejects_public_field_structs_without_non_exhaustive() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &[
                "#[derive(Debug, Clone, PartialEq, Eq)]",
                "pub struct CustomPayloadEvent {",
                "    pub channel: String,",
                "}",
                "pub struct PlayerId(pub u64);",
                "pub struct MultilineId(",
                "    pub u64,",
                ");",
            ],
            &mut findings,
        );

        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].line, 2);
        assert_eq!(
            findings[0].message,
            "plugin/script API public field struct missing #[non_exhaustive]"
        );
        assert_eq!(findings[1].line, 5);
        assert_eq!(findings[2].line, 6);
    }

    #[test]
    fn api_guard_allows_non_exhaustive_api_items() {
        let mut findings = Vec::new();
        scan_api_leaks(
            Path::new("crates/mc-extension/src/lib.rs"),
            &[
                "#[derive(Debug, Clone, PartialEq, Eq)]",
                "#[non_exhaustive]",
                "pub enum InboundEvent {",
                "    PlayerJoined,",
                "}",
                "#[derive(Debug, Clone, PartialEq, Eq)]",
                "#[non_exhaustive]",
                "pub struct CustomPayloadEvent {",
                "    pub channel: String,",
                "}",
            ],
            &mut findings,
        );

        assert!(findings.is_empty());
    }

    #[test]
    fn display_path_prefers_workspace_relative_paths() {
        assert_eq!(
            display_path(
                Path::new("/repo"),
                Path::new("/repo/crates/mc-extension/src/lib.rs")
            ),
            "crates/mc-extension/src/lib.rs"
        );
    }
}
