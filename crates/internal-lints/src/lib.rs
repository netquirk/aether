//! Lints that enforce the conventions of `AGENTS.md`.
//!
//! `AGENTS.md` forbids the `anyhow` and `color-eyre` crates and asks for concrete error enums
//! instead of `Box<dyn std::error::Error>` in a public type, and it asks for tests without
//! timeouts. This crate reads the repository and reports every occurrence of those constructs. It
//! only reads files: it writes nothing, opens no socket and needs no credential.
//!
//! - [`scan`] and the types around it are the error-handling lint (`just error-lint`).
//! - [`scan_workspace_waits`] and the types around it are the fixed-wait lint (`just wait-lint`),
//!   which reports the wall-clock waits in the test functions of the workspace.
//! - [`scan_workspace_private`] and the types around it are the private-item lint
//!   (`just private-lint`), which reports the tests that reach a private item of the crate under
//!   test.

use std::fs;
use std::path::{Path, PathBuf};

mod manifests;
mod private_index;
mod private_macro;
mod private_reach;
mod private_scan;
mod rust_scan;
mod text_scan;
mod wait_scan;

pub use private_scan::{PrivateFinding, PrivateItemKind, PrivateReport, scan_crate_private, scan_workspace_private};
pub use wait_scan::{
    ExemptionReason, WaitExemption, WaitFinding, WaitKind, WaitReport, default_allowlist_path, scan_workspace_waits,
    scan_workspace_waits_allowing,
};

/// The forbidden error-handling construct a [`Finding`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenKind {
    /// The `anyhow` crate.
    AnyhowCrate,
    /// The `color-eyre` crate.
    ColorEyreCrate,
    /// A boxed `std::error::Error` in a public type.
    BoxedPublicError,
}

impl ForbiddenKind {
    /// Every kind the lint reports, in the order it prints the counts.
    pub fn all() -> [Self; 3] {
        [Self::AnyhowCrate, Self::ColorEyreCrate, Self::BoxedPublicError]
    }

    /// The name the lint prints for this kind.
    pub fn label(&self) -> &'static str {
        match self {
            Self::AnyhowCrate => "anyhow crate",
            Self::ColorEyreCrate => "color-eyre crate",
            Self::BoxedPublicError => "boxed public error",
        }
    }
}

/// One forbidden construct found while scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The file the construct was found in.
    pub path: PathBuf,
    /// The 1-based line the construct is on.
    pub line: usize,
    /// The 1-based column the construct starts at.
    pub column: usize,
    /// Which forbidden construct this is.
    pub kind: ForbiddenKind,
    /// The construct as it was written, for example `anyhow::Result`.
    pub detail: String,
}

/// What a scan covered and what it found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Every forbidden construct found, in scan order.
    pub findings: Vec<Finding>,
    /// Every path the scan read.
    pub scanned: Vec<PathBuf>,
    /// Paths that could not be read or parsed, so their contents went unchecked.
    pub problems: Vec<String>,
}

impl Report {
    /// Whether the scan found nothing and checked everything it read.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.problems.is_empty()
    }

    /// How many findings of `kind` the scan made.
    pub fn count(&self, kind: ForbiddenKind) -> usize {
        self.findings.iter().filter(|finding| finding.kind == kind).count()
    }

    fn merge(&mut self, other: Report) {
        self.findings.extend(other.findings);
        self.scanned.extend(other.scanned);
        self.problems.extend(other.problems);
    }
}

/// Scan a Cargo workspace root: every member crate and every TypeScript package.
pub fn scan_workspace(root: &Path) -> Report {
    let mut report = Report::default();
    let manifest = root.join("Cargo.toml");
    if manifest.is_file() {
        report.scanned.push(manifest.clone());
        report.findings.extend(manifests::scan_cargo_manifest(&manifest));
    }
    for member in workspace_members(root) {
        report.merge(scan_crate(&member));
    }
    for package in workspace_packages(root) {
        report.merge(scan_package(&package));
    }
    report
}

/// Scan one crate: the directory that holds its `Cargo.toml`.
pub fn scan_crate(crate_root: &Path) -> Report {
    let mut report = Report::default();
    let manifest = crate_root.join("Cargo.toml");
    if manifest.is_file() {
        report.scanned.push(manifest.clone());
        report.findings.extend(manifests::scan_cargo_manifest(&manifest));
    }
    for file in crate_files(crate_root) {
        report.scanned.push(file.clone());
        let (findings, problems) = rust_scan::scan_rust_file(&file);
        report.findings.extend(findings);
        report.problems.extend(problems);
    }
    report.findings.extend(rust_scan::scan_public_surface(crate_root));
    report
}

/// Scan one TypeScript package: the directory that holds its `package.json`.
pub fn scan_package(package_root: &Path) -> Report {
    let mut report = Report::default();
    let manifest = package_root.join("package.json");
    if manifest.is_file() {
        report.scanned.push(manifest.clone());
        report.findings.extend(manifests::scan_package_manifest(&manifest));
    }
    for file in package_files(package_root) {
        report.scanned.push(file.clone());
        report.findings.extend(text_scan::scan_text_file(&file));
    }
    report
}

/// Scan whatever `path` points at: a workspace root, a crate or a TypeScript package.
pub fn scan(path: &Path) -> Report {
    let manifest = path.join("Cargo.toml");
    if manifest.is_file() {
        if workspace_members(path).is_empty() {
            return scan_crate(path);
        }
        return scan_workspace(path);
    }
    if path.join("package.json").is_file() {
        return scan_package(path);
    }
    Report::default()
}

pub(crate) fn workspace_members(root: &Path) -> Vec<PathBuf> {
    let manifest = root.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return Vec::new();
    };
    let Some(members) =
        value.get("workspace").and_then(|workspace| workspace.get("members")).and_then(toml::Value::as_array)
    else {
        return Vec::new();
    };
    let mut crates = Vec::new();
    for member in members.iter().filter_map(toml::Value::as_str) {
        crates.extend(expand_pattern(root, member));
    }
    crates
}

fn workspace_packages(root: &Path) -> Vec<PathBuf> {
    let patterns = pnpm_package_patterns(root);
    let patterns = if patterns.is_empty() { vec!["packages".to_string()] } else { patterns };
    let mut packages = Vec::new();
    for pattern in patterns {
        for directory in expand_pattern(root, &pattern) {
            if directory.join("package.json").is_file() {
                packages.push(directory);
            }
        }
    }
    packages
}

fn pnpm_package_patterns(root: &Path) -> Vec<String> {
    let workspace = root.join("pnpm-workspace.yaml");
    let Ok(text) = fs::read_to_string(workspace) else {
        return Vec::new();
    };
    let mut patterns = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if in_packages {
            if let Some(item) = trimmed.strip_prefix('-') {
                patterns.push(item.trim().trim_matches(['"', '\'']).to_string());
                continue;
            }
            if !trimmed.is_empty() {
                in_packages = false;
            }
        }
        if trimmed == "packages:" {
            in_packages = true;
        }
    }
    patterns
}

fn expand_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let Some(star) = pattern.find('*') else {
        let path = root.join(pattern);
        return if path.is_dir() { vec![path] } else { Vec::new() };
    };
    let prefix = pattern[..star].trim_end_matches('/');
    let suffix = &pattern[star + 1..];
    let Ok(entries) = fs::read_dir(root.join(prefix)) else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || (!suffix.is_empty() && !name.ends_with(suffix)) {
            continue;
        }
        directories.push(path);
    }
    directories.sort();
    directories
}

pub(crate) fn crate_files(crate_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(crate_root, &mut files, &is_rust_file, true);
    files
}

fn package_files(package_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files(package_root, &mut files, &is_package_file, false);
    files
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>, keep: &dyn Fn(&Path) -> bool, skip_nested: bool) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            if skip_nested && path.join("Cargo.toml").is_file() {
                continue;
            }
            collect_files(&path, files, keep, skip_nested);
        } else if keep(&path) {
            files.push(path);
        }
    }
}

fn is_rust_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "rs")
}

fn is_package_file(path: &Path) -> bool {
    if path.file_name().is_some_and(|name| name == "package.json") {
        return false;
    }
    path.extension().is_some_and(|extension| {
        matches!(
            extension.to_str(),
            Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "astro" | "json" | "md" | "mdx")
        )
    })
}
