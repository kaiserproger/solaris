use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const LOCK_OWNER_TYPES: &[&str] = &["WorldHandle", "SessionRegistry", "WorldStorage"];

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
    let is_extension_api =
        path_has_component(path, "mc-extension") || path_has_component(path, "mc-script");
    if !is_extension_api {
        return;
    }

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let publicish = trimmed.starts_with("pub ") || trimmed.starts_with("pub(");
        if publicish && LOCK_OWNER_TYPES.iter().any(|name| trimmed.contains(name)) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API exposes lock-owning runtime type".into(),
            });
        }
    }
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
            &["pub struct Api {", "pub world: WorldHandle,", "}"],
            &mut findings,
        );
        scan_api_leaks(
            Path::new("crates/mc-net/src/lib.rs"),
            &["pub world: WorldHandle,"],
            &mut findings,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 2);
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
