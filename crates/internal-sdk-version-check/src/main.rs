//! Command line entry point of the SDK version check.
//!
//! `sdk-version-check [PATH]` reads the repository at `PATH` (the repository root by default),
//! compares the version the SDK package publishes with the version each README states, prints the
//! versions it compared, and exits with a non-zero status when a README states another version.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use internal_sdk_version_check::{check_sdk_version, published_version};

const USAGE: &str = "usage: sdk-version-check [PATH]";

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
    let manifest_path = root.join("packages/aether-sdk/package.json");
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("sdk-version-check: {}: {error}", manifest_path.display());
            return ExitCode::FAILURE;
        }
    };
    let published = match published_version(&manifest) {
        Ok(version) => version,
        Err(error) => {
            eprintln!("sdk-version-check: {}: {error}", manifest_path.display());
            return ExitCode::FAILURE;
        }
    };
    println!("sdk-version-check: the package publishes SDK version {published}");

    let mut clean = true;
    for readme_path in readme_paths(root) {
        let readme = match std::fs::read_to_string(&readme_path) {
            Ok(readme) => readme,
            Err(error) => {
                eprintln!("sdk-version-check: {}: {error}", readme_path.display());
                clean = false;
                continue;
            }
        };
        let report = match check_sdk_version(&manifest, &readme) {
            Ok(report) => report,
            Err(error) => {
                eprintln!("sdk-version-check: {}: {error}", readme_path.display());
                clean = false;
                continue;
            }
        };
        match report.mismatch() {
            Some(mismatch) => {
                eprintln!("sdk-version-check: {}: {mismatch}", readme_path.display());
                clean = false;
            }
            None => println!("sdk-version-check: {} states SDK version {}", readme_path.display(), report.stated),
        }
    }

    if clean { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// The READMEs that state the version of the SDK package.
fn readme_paths(root: &Path) -> Vec<PathBuf> {
    vec![root.join("README.md"), root.join("packages/aether-sdk/README.md")]
}
