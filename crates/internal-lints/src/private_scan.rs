//! The private-item lint: the tests that reach a private item of the crate under test.
//!
//! `AGENTS.md` asks for tests that exercise only the public API, so that the suite asserts
//! behaviour a user of the crate can observe. This module finds every test function of the
//! workspace that reaches a private item of the crate under test: a function, a method, a field
//! or a type that a user of the crate cannot see.
//!
//! A reach is a reach in every form: the call, the method call, the field access, the struct
//! literal and the `use` declaration that imports the item. An import reaches the item it names,
//! even when the test never calls it.
//!
//! The scan is syntactic. It reads files, runs nothing, and needs no network, no credential, no
//! model provider and no deployed service.
//!
//! The lint reads the crate under test as the compiler would:
//!
//! - a **unit test** lives inside the crate, so it may reach the private items of the module it
//!   is written in: those are the internals of the module under test. The private items of the
//!   crate root are the internals of the whole crate, and no test reaches them.
//! - an **integration test** lives outside the crate, so it may use the public API of the crate
//!   under test only, and may use everything it declares itself.
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

use crate::private_index::CrateIndex;
use crate::private_reach::{ReachVisitor, Scope, collect_locals};
use crate::workspace_members;

/// The kind of item a [`PrivateFinding`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateItemKind {
    /// A type: a struct, an enum or a union.
    Type,
    /// A free function.
    Function,
    /// A method of an inherent implementation.
    Method,
    /// A field of a struct, an enum or a union.
    Field,
    /// A constant or a static.
    Constant,
}

impl PrivateItemKind {
    /// Every kind the lint reports, in the order it prints the counts.
    pub fn all() -> [Self; 5] {
        [Self::Type, Self::Function, Self::Method, Self::Field, Self::Constant]
    }

    /// The name the lint prints for this kind.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Type => "private type",
            Self::Function => "private function",
            Self::Method => "private method",
            Self::Field => "private field",
            Self::Constant => "private constant",
        }
    }
}

/// One private item a test reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateFinding {
    /// The file of the test that reached the item.
    pub path: PathBuf,
    /// The 1-based line the reach is on.
    pub line: usize,
    /// The 1-based column the reach starts at.
    pub column: usize,
    /// The name of the test function that reached the item.
    pub test: String,
    /// Which kind of item the test reached.
    pub kind: PrivateItemKind,
    /// The name of the item the test reached.
    pub item: String,
    /// The module of the crate under test that declares the item.
    pub module: String,
    /// The reach as it was written, for example `private_helper()` or `use my_crate::private_helper;`.
    pub detail: String,
}

/// What the scan covered and what it found.
#[derive(Debug, Clone, Default)]
pub struct PrivateReport {
    /// Every private item a test reached, in scan order.
    pub findings: Vec<PrivateFinding>,
    /// Every test function the scan read.
    pub tests_scanned: usize,
    /// How many of the test functions the scan read reach a private item.
    pub tests_flagged: usize,
    /// Every path the scan read.
    pub files: Vec<PathBuf>,
    /// Paths that could not be read or parsed, so their contents went unchecked.
    pub problems: Vec<String>,
}

impl PrivateReport {
    /// Whether the scan found nothing and checked everything it read.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.problems.is_empty()
    }

    /// How many findings of `kind` the scan made.
    pub fn count(&self, kind: PrivateItemKind) -> usize {
        self.findings.iter().filter(|finding| finding.kind == kind).count()
    }

    fn merge(&mut self, other: PrivateReport) {
        for finding in other.findings {
            let known = self.findings.iter().any(|existing| {
                existing.path == finding.path
                    && existing.line == finding.line
                    && existing.column == finding.column
                    && existing.item == finding.item
            });
            if !known {
                self.findings.push(finding);
            }
        }
        self.tests_scanned += other.tests_scanned;
        self.tests_flagged += other.tests_flagged;
        self.files.extend(other.files);
        self.problems.extend(other.problems);
    }
}

/// Scan every test function of the workspace for a reach of a private item.
///
/// A root that holds a crate but names no workspace member is scanned as that one crate, so the
/// lint also runs on a single crate.
pub fn scan_workspace_private(root: &Path) -> PrivateReport {
    let mut report = PrivateReport::default();
    let members = workspace_members(root);
    let crates =
        if members.is_empty() && root.join("Cargo.toml").is_file() { vec![root.to_path_buf()] } else { members };
    for member in crates {
        report.merge(scan_crate_private(&member));
    }
    report
}

/// Scan every test function of one crate for a reach of a private item.
pub fn scan_crate_private(crate_root: &Path) -> PrivateReport {
    let mut report = PrivateReport::default();
    let manifest = crate_root.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return report;
    };
    let Ok(value) = text.parse::<toml::Value>() else {
        return report;
    };
    let Some(package) = value.get("package").and_then(|package| package.get("name")).and_then(toml::Value::as_str)
    else {
        return report;
    };
    let library = value.get("lib");
    let library_name = library
        .and_then(|library| library.get("name"))
        .and_then(toml::Value::as_str)
        .map_or_else(|| package.replace('-', "_"), str::to_string);
    let library_path = library
        .and_then(|library| library.get("path"))
        .and_then(toml::Value::as_str)
        .map_or_else(|| "src/lib.rs".to_string(), str::to_string);

    let mut library_index = CrateIndex::new(library_name.clone());
    let library_file = crate_root.join(&library_path);
    let mut have_library = false;
    if library_file.is_file() {
        match library_index.read_root(&library_file) {
            Ok(()) => have_library = true,
            Err(problem) => report.problems.push(problem),
        }
    }

    for root_file in crate_roots(crate_root) {
        if root_file == library_file {
            continue;
        }
        let mut index = CrateIndex::new(library_name.clone());
        match index.read_root(&root_file) {
            Ok(()) => scan_unit_tests(&index, &mut report),
            Err(problem) => report.problems.push(problem),
        }
    }

    if have_library {
        library_index.read_sources(&crate_root.join("src"));
        scan_unit_tests(&library_index, &mut report);
        for file in test_files(crate_root, &value) {
            scan_integration_tests(&library_index, &file, &mut report);
        }
    }
    report
}

/// The files that hold the unit tests of one crate root.
fn crate_roots(crate_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for name in ["src/lib.rs", "src/main.rs"] {
        let path = crate_root.join(name);
        if path.is_file() {
            roots.push(path);
        }
    }
    let binaries = crate_root.join("src/bin");
    if binaries.is_dir() {
        let Ok(entries) = fs::read_dir(&binaries) else {
            return roots;
        };
        let mut paths: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_file()).collect();
        paths.sort();
        roots.extend(paths);
    }
    roots
}

/// The test files of one crate: the integration tests the compiler builds for it.
///
/// Cargo builds one integration test for every file of the `tests` directory and one for every
/// `main.rs` of a subdirectory of it. The other files of the `tests` directory are the modules of
/// those tests, and the lint reads them as such.
fn test_files(crate_root: &Path, manifest: &toml::Value) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let directory = crate_root.join("tests");
    let Ok(entries) = fs::read_dir(&directory) else {
        return files;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        } else if path.is_dir() {
            let main = path.join("main.rs");
            if main.is_file() {
                files.push(main);
            }
        }
    }
    if let Some(targets) = manifest.get("test").and_then(toml::Value::as_array) {
        for target in targets {
            let Some(path) = target.get("path").and_then(toml::Value::as_str) else {
                continue;
            };
            let path = crate_root.join(path);
            if path.is_file() && !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Scan the unit tests of one crate root.
fn scan_unit_tests(index: &CrateIndex, report: &mut PrivateReport) {
    report.files.extend(index.sources.keys().cloned());
    for module in &index.modules {
        let Some(file) = module.file.clone() else {
            continue;
        };
        let Some(source) = index.sources.get(&file) else {
            continue;
        };
        for test in &module.tests {
            report.tests_scanned += 1;
            let scope = Scope { module: module.path.clone(), imports: module.imports.clone() };
            let mut visitor = ReachVisitor {
                index,
                scope_index: index,
                scope,
                outside: false,
                file: &file,
                source,
                test: test.name.clone(),
                findings: Vec::new(),
                locals: collect_locals(&test.body),
            };
            visitor.check_imports();
            visitor.visit_block(&test.body);
            if !visitor.findings.is_empty() {
                report.tests_flagged += 1;
            }
            report.findings.extend(visitor.findings);
        }
    }
}

/// Scan the integration tests of one file: the tests compiled outside the crate under test.
fn scan_integration_tests(library: &CrateIndex, file: &Path, report: &mut PrivateReport) {
    let mut file_index = CrateIndex::new(library.name.clone());
    if file_index.read_root(file).is_err() {
        return;
    }
    report.files.extend(file_index.sources.keys().cloned());
    for module in &file_index.modules {
        let Some(module_file) = module.file.clone() else {
            continue;
        };
        let Some(source) = file_index.sources.get(&module_file) else {
            continue;
        };
        for test in &module.tests {
            report.tests_scanned += 1;
            let scope = Scope { module: module.path.clone(), imports: module.imports.clone() };
            let mut visitor = ReachVisitor {
                index: library,
                scope_index: &file_index,
                scope,
                outside: true,
                file: &module_file,
                source,
                test: test.name.clone(),
                findings: Vec::new(),
                locals: collect_locals(&test.body),
            };
            visitor.check_imports();
            visitor.visit_block(&test.body);
            if !visitor.findings.is_empty() {
                report.tests_flagged += 1;
            }
            report.findings.extend(visitor.findings);
        }
    }
}
