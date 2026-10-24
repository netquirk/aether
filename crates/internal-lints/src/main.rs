//! Command line entry point of the error-handling lint.
//!
//! `internal-lints [PATH]` scans `PATH` (the repository root by default), prints every path it
//! read, every forbidden construct it found and the number of findings per kind, and exits with a
//! non-zero status when it found something.

use std::path::Path;
use std::process::ExitCode;

use internal_lints::{ForbiddenKind, Report, scan};

const USAGE: &str = "usage: internal-lints [PATH]";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(argument) = arguments.next() else {
        return run(Path::new("."));
    };
    if argument == "--help" || argument == "-h" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    run(Path::new(&argument))
}

fn run(root: &Path) -> ExitCode {
    let report = scan(root);
    print_report(root, &report);
    if report.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn print_report(root: &Path, report: &Report) {
    println!("error-lint: scanning {}", root.display());
    for path in &report.scanned {
        println!("scanned {}", display_path(root, path));
    }
    println!("error-lint: scanned {} paths", report.scanned.len());
    for finding in &report.findings {
        eprintln!(
            "error-lint: {}:{}:{}: {}: {}",
            display_path(root, &finding.path),
            finding.line,
            finding.column,
            finding.kind.label(),
            finding.detail
        );
    }
    for problem in &report.problems {
        eprintln!("error-lint: {problem}");
    }
    let counts: Vec<String> =
        ForbiddenKind::all().iter().map(|kind| format!("{}: {}", kind.label(), report.count(*kind))).collect();
    println!("error-lint: found {}", counts.join(", "));
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
