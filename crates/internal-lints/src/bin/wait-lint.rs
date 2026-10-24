//! Command line entry point of the fixed-wait lint.
//!
//! `wait-lint [PATH]` scans the test functions of the workspace at `PATH` (the repository root by
//! default), prints every wall-clock wait it found with the file, the test function and the
//! offending expression, prints how many test functions it scanned and how many it flagged, and
//! exits with a non-zero status when it found one.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use internal_lints::{ExemptionReason, WaitKind, WaitReport, default_allowlist_path, scan_workspace_waits_allowing};

const USAGE: &str = "usage: wait-lint [--allowlist PATH] [PATH]";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let mut allowlist: Option<PathBuf> = None;
    let mut root: Option<PathBuf> = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--allowlist" => {
                let Some(path) = arguments.next() else {
                    eprintln!("wait-lint: --allowlist needs a path");
                    return ExitCode::FAILURE;
                };
                allowlist = Some(PathBuf::from(path));
            }
            _ => root = Some(PathBuf::from(argument)),
        }
    }
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let allowlist = allowlist.unwrap_or_else(|| default_allowlist_path(&root));
    run(&root, &allowlist)
}

fn run(root: &Path, allowlist: &Path) -> ExitCode {
    let report = scan_workspace_waits_allowing(root, allowlist);
    print_report(root, &report);
    if report.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn print_report(root: &Path, report: &WaitReport) {
    println!("wait-lint: scanning {}", root.display());
    println!("wait-lint: scanned {} test functions in {} files", report.tests_scanned, report.files.len());
    println!("wait-lint: flagged {} test functions", report.tests_flagged);
    let exemptions: Vec<String> = ExemptionReason::all()
        .iter()
        .map(|reason| format!("{}: {}", reason.label(), report.exempted(*reason)))
        .collect();
    println!("wait-lint: exempted {} test functions ({})", report.exemptions.len(), exemptions.join(", "));
    for finding in &report.findings {
        eprintln!(
            "wait-lint: {}:{}:{}: {}: {}: {}",
            display_path(root, &finding.path),
            finding.line,
            finding.column,
            finding.test,
            finding.kind.label(),
            finding.detail
        );
    }
    for problem in &report.problems {
        eprintln!("wait-lint: {problem}");
    }
    let counts: Vec<String> =
        WaitKind::all().iter().map(|kind| format!("{}: {}", kind.label(), report.count(*kind))).collect();
    println!("wait-lint: found {}", counts.join(", "));
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
