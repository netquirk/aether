//! Dependency checks for the `Cargo.toml` and `package.json` manifests.

use std::fs;
use std::path::Path;

use crate::{Finding, ForbiddenKind};

/// The dependency names the lint bans, with the kind each one is reported as.
const BANNED_DEPENDENCIES: [(&str, ForbiddenKind); 2] =
    [("anyhow", ForbiddenKind::AnyhowCrate), ("color-eyre", ForbiddenKind::ColorEyreCrate)];

/// The `package.json` tables that hold dependencies.
const PACKAGE_DEPENDENCY_TABLES: [&str; 4] =
    ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];

/// Scan a `Cargo.toml` for a dependency on a banned crate.
pub fn scan_cargo_manifest(path: &Path) -> Vec<Finding> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((name, kind)) = dependency_in_table_header(trimmed) {
            findings.push(finding(path, index + 1, name, kind));
            continue;
        }
        if let Some((name, kind)) = dependency_key(trimmed) {
            findings.push(finding(path, index + 1, name, kind));
        }
    }
    findings
}

/// Scan a `package.json` for a dependency on a banned crate.
pub fn scan_package_manifest(path: &Path) -> Vec<Finding> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for table in PACKAGE_DEPENDENCY_TABLES {
        let Some(dependencies) = value.get(table).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for name in dependencies.keys() {
            let Some((banned, kind)) = banned_dependency(name) else {
                continue;
            };
            let Some(line) = line_of_key(&text, banned) else {
                continue;
            };
            findings.push(finding(path, line, banned, kind));
        }
    }
    findings
}

fn dependency_in_table_header(line: &str) -> Option<(&'static str, ForbiddenKind)> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let parts: Vec<&str> = inner.split('.').collect();
    let (name, tables) = parts.split_last()?;
    if !tables.iter().any(|table| table.contains("dependencies")) {
        return None;
    }
    banned_dependency(name)
}

fn dependency_key(line: &str) -> Option<(&'static str, ForbiddenKind)> {
    let (key, value) = line.split_once('=')?;
    let name = key.trim().rsplit('.').next()?;
    if let Some(found) = banned_dependency(name) {
        return Some(found);
    }
    for (banned, kind) in BANNED_DEPENDENCIES {
        let renamed = format!("package = \"{banned}\"");
        let renamed_single = format!("package = '{banned}'");
        if value.contains(&renamed) || value.contains(&renamed_single) {
            return Some((banned, kind));
        }
    }
    None
}

fn banned_dependency(name: &str) -> Option<(&'static str, ForbiddenKind)> {
    let name = name.trim_matches(['"', '\'']);
    BANNED_DEPENDENCIES.iter().find(|(banned, _)| *banned == name).map(|(banned, kind)| (*banned, *kind))
}

fn line_of_key(text: &str, name: &str) -> Option<usize> {
    let needle = format!("\"{name}\"");
    for (index, line) in text.lines().enumerate() {
        let Some(position) = line.find(&needle) else {
            continue;
        };
        if line[position + needle.len()..].trim_start().starts_with(':') {
            return Some(index + 1);
        }
    }
    None
}

fn finding(path: &Path, line: usize, name: &str, kind: ForbiddenKind) -> Finding {
    Finding { path: path.to_path_buf(), line, column: 1, kind, detail: name.to_string() }
}
