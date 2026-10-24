//! Textual scan of the TypeScript packages, where Rust snippets live in docs and fixtures.

use std::fs;
use std::path::Path;

use crate::{Finding, ForbiddenKind};

/// The crate names the lint bans in a text file.
const BANNED_CRATES: [(&str, ForbiddenKind); 2] =
    [("anyhow", ForbiddenKind::AnyhowCrate), ("color_eyre", ForbiddenKind::ColorEyreCrate)];

/// Scan one text file for the banned crates and for boxed errors.
pub fn scan_text_file(path: &Path) -> Vec<Finding> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for (crate_name, kind) in BANNED_CRATES {
            if mentions_crate(line, crate_name) {
                findings.push(finding(path, index + 1, column_of(line, crate_name), kind, crate_name));
            }
        }
        if let Some(detail) = boxed_error_in_line(line) {
            findings.push(finding(path, index + 1, column_of(line, "Box"), ForbiddenKind::BoxedPublicError, &detail));
        }
    }
    findings
}

fn mentions_crate(line: &str, name: &str) -> bool {
    let path_use = format!("{name}::");
    let macro_use = format!("{name}!");
    if line.contains(&path_use) || line.contains(&macro_use) {
        return true;
    }
    let use_statement = format!("use {name}");
    let Some(position) = line.find(&use_statement) else {
        return false;
    };
    let rest = line[position + use_statement.len()..].trim_start();
    rest.starts_with("::")
        || rest.starts_with(';')
        || rest.starts_with(',')
        || rest.starts_with('{')
        || rest.starts_with("as ")
}

fn boxed_error_in_line(line: &str) -> Option<String> {
    let position = line.find("Box")?;
    let rest = line[position + "Box".len()..].trim_start();
    let rest = rest.strip_prefix('<')?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("dyn")?;
    let end = rest.find('>')?;
    let inner = rest[..end].trim();
    if !inner.contains("Error") {
        return None;
    }
    Some(format!("Box<dyn {inner}>"))
}

fn column_of(line: &str, needle: &str) -> usize {
    line.find(needle).map_or(1, |position| position + 1)
}

fn finding(path: &Path, line: usize, column: usize, kind: ForbiddenKind, detail: &str) -> Finding {
    Finding { path: path.to_path_buf(), line, column, kind, detail: detail.to_string() }
}
