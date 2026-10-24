//! Proves the README example check bites.
//!
//! `src/lib.rs` compiles the example in the workspace README as a doc test. This test points the
//! same rustdoc doc-test machinery at a fixture README whose example names a type
//! `aether-agent-core` does not export, and asserts that the compile fails and names that type.
//! It compiles markdown only: no provider, no network, no credential and no deployed service.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{fs, time::SystemTime};

/// The type the fixture README names and `aether-agent-core` does not export.
const MISSING_TYPE: &str = "NotExportedByAetherCore";

#[test]
fn fixture_readme_example_fails_naming_the_type_the_core_does_not_export() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme_missing_type.md");
    let output = compile_markdown(&fixture);
    let report = report(&output);

    assert!(!output.status.success(), "the fixture README must not compile:\n{report}");
    assert!(report.contains(MISSING_TYPE), "the failure must name {MISSING_TYPE}:\n{report}");
    assert!(report.contains("readme_missing_type.md"), "the failure must name the fixture README:\n{report}");
}

/// Compiles the Rust examples of a markdown file the way rustdoc compiles a doc test.
fn compile_markdown(markdown: &Path) -> Output {
    let dependencies = dependency_directory();
    Command::new("rustdoc")
        .arg("--test")
        .arg(markdown)
        .args(["--edition", &edition()])
        .arg("-L")
        .arg(format!("dependency={}", dependencies.display()))
        .arg("--extern")
        .arg(format!("aether_core={}", aether_core_rlib().display()))
        .output()
        .expect("run rustdoc")
}

/// The compiler's report: rustdoc prints the compile errors of a failed doc test on stdout.
fn report(output: &Output) -> String {
    let mut report = String::from_utf8_lossy(&output.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&output.stderr));
    report
}

/// The directory cargo put this test's dependencies in.
fn dependency_directory() -> PathBuf {
    let executable = std::env::current_exe().expect("the test executable path");
    executable.parent().expect("the target directory").to_path_buf()
}

/// The `aether_core` rlib cargo built for this test, the most recently built one.
fn aether_core_rlib() -> PathBuf {
    let dependencies = dependency_directory();
    let mut rlibs: Vec<(SystemTime, PathBuf)> = fs::read_dir(&dependencies)
        .expect("read the target directory")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let modified = fs::metadata(&path).and_then(|metadata| metadata.modified()).ok()?;
            let is_rlib = name.starts_with("libaether_core-")
                && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("rlib"));
            is_rlib.then_some((modified, path))
        })
        .collect();
    rlibs.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let Some((_, rlib)) = rlibs.into_iter().next() else {
        panic!("no aether_core rlib in {}", dependencies.display());
    };
    rlib
}

/// The edition `aether-agent-core` is built with, read from its manifest.
fn edition() -> String {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).expect("read Cargo.toml");
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("edition = \""))
        .and_then(|value| value.split('"').next())
        .expect("the edition in Cargo.toml")
        .to_string()
}
