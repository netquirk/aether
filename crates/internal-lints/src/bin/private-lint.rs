//! Command line entry point of the private-item lint.
//!
//! `private-lint [PATH]` scans the test functions of the workspace at `PATH` (the repository root
//! by default), prints every private item of the crate under test a test reached with the file,
//! the test function and the offending expression, prints how many test functions it scanned and
//! how many it flagged, and exits with a non-zero status when it found one.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use internal_lints::{PrivateItemKind, PrivateReport, scan_workspace_private};

const USAGE: &str = "usage: private-lint [PATH]";

fn main() -> ExitCode {
    let mut root: Option<PathBuf> = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => root = Some(PathBuf::from(argument)),
        }
    }
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    run(&root)
}

fn run(root: &Path) -> ExitCode {
    let report = scan_workspace_private(root);
    print_report(root, &report);
    if report.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn print_report(root: &Path, report: &PrivateReport) {
    println!("private-lint: scanning {}", root.display());
    println!("private-lint: scanned {} test functions in {} files", report.tests_scanned, report.files.len());
    println!("private-lint: flagged {} test functions", report.tests_flagged);
    for finding in &report.findings {
        eprintln!(
            "private-lint: {}:{}:{}: {}: {}: {}: {}",
            display_path(root, &finding.path),
            finding.line,
            finding.column,
            finding.test,
            finding.kind.label(),
            finding.item,
            finding.detail
        );
    }
    for problem in &report.problems {
        eprintln!("private-lint: {problem}");
    }
    let counts: Vec<String> =
        PrivateItemKind::all().iter().map(|kind| format!("{}: {}", kind.label(), report.count(*kind))).collect();
    println!("private-lint: found {}", counts.join(", "));
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
