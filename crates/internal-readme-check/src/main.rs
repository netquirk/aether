//! Command line entry point of the README provider check.
//!
//! `readme-check [PATH]` reads the README of the repository at `PATH` (the repository root by
//! default), compares the providers it names with the providers the harness registers, prints
//! every provider the README names that the harness does not register, and exits with a
//! non-zero status when it found one.

use std::path::Path;
use std::process::ExitCode;

use internal_readme_check::{check_providers, harness_providers};

const USAGE: &str = "usage: readme-check [PATH]";

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
    let readme_path = root.join("README.md");
    let readme = match std::fs::read_to_string(&readme_path) {
        Ok(readme) => readme,
        Err(error) => {
            eprintln!("readme-check: {}: {error}", readme_path.display());
            return ExitCode::FAILURE;
        }
    };
    let registered = harness_providers();
    let report = match check_providers(&readme, &registered) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("readme-check: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("readme-check: the README names {} providers", report.listed.len());
    println!("readme-check: the harness registers {} providers", registered.len());
    for provider in &report.unregistered {
        eprintln!("readme-check: {provider}");
    }
    if report.is_clean() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
