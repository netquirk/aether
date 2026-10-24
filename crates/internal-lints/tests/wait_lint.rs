//! Tests for the fixed-wait lint. They drive the public API and the lint binary only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use internal_lints::{ExemptionReason, WaitKind, scan_workspace_waits};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root")
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/waiting-crate")
}

fn lint() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wait-lint"));
    command.current_dir(repo_root());
    command
}

fn run_lint(path: &Path) -> Output {
    lint().arg(path).output().expect("the lint runs")
}

/// The number of test attributes a file holds, ignoring the ones a comment mentions.
fn count_test_attributes(path: &Path) -> usize {
    let source = fs::read_to_string(path).expect("the file reads");
    let mut count = 0;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let mut rest = trimmed;
        while let Some(start) = rest.find("#[") {
            let after = &rest[start + 2..];
            let end = after.find([']', '(', ' ']).unwrap_or(after.len());
            let attribute = &after[..end];
            if attribute.rsplit("::").next().is_some_and(|last| last == "test") {
                count += 1;
            }
            rest = after;
        }
    }
    count
}

#[test]
fn workspace_scan_counts_every_test_function() {
    let report = scan_workspace_waits(&repo_root());
    assert!(report.problems.is_empty(), "every file of the workspace must parse: {:?}", report.problems);
    let expected: usize = report.files.iter().map(|file| count_test_attributes(file)).sum();
    assert_eq!(report.tests_scanned, expected, "the scan must read every test function of the workspace");
    assert!(
        report.tests_scanned > 1000,
        "the workspace holds thousands of test functions, got {}",
        report.tests_scanned
    );
}

#[test]
fn cli_fails_naming_the_test_function_the_expression_and_the_counts() {
    let output = run_lint(&fixture_root());
    assert!(!output.status.success(), "the lint must fail when it finds a fixed wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("scanned 4 test functions"),
        "the lint must print how many test functions it scanned: {stdout}"
    );
    assert!(stdout.contains("flagged 2 test functions"), "the lint must print how many it flagged: {stdout}");
    assert!(stderr.contains("src/lib.rs:"), "the finding must name the file: {stderr}");
    assert!(stderr.contains("sleeps_for_a_fixed_duration"), "the finding must name the test function: {stderr}");
    assert!(
        stderr.contains("std::thread::sleep(Duration::from_millis(10))"),
        "the finding must name the offending expression: {stderr}"
    );
}

#[test]
fn sleeping_fixture_test_is_reported_by_name() {
    let report = scan_workspace_waits(&fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.test == "sleeps_for_a_fixed_duration")
        .expect("the sleeping test is reported");
    assert_eq!(finding.kind, WaitKind::Sleep);
    assert!(
        finding.detail.contains("thread::sleep"),
        "the finding must quote the offending expression: {}",
        finding.detail
    );
    assert!(finding.path.ends_with("src/lib.rs"), "the finding must name the file: {:?}", finding.path);
    assert_eq!(report.tests_flagged, 2, "the fixture holds two waits the lint reports");
}

#[test]
fn fake_clock_and_fake_provider_tests_are_not_flagged() {
    let report = scan_workspace_waits(&fixture_root());
    for name in ["sleeps_on_the_paused_clock", "drives_a_fake_provider"] {
        assert!(
            !report.findings.iter().any(|finding| finding.test == name),
            "the lint must not flag {name}: {:?}",
            report.findings
        );
    }
    assert!(
        report.exemptions.iter().any(|exemption| exemption.test == "sleeps_on_the_paused_clock"
            && exemption.reason == ExemptionReason::FakeClock),
        "the paused-clock test is exempt: {:?}",
        report.exemptions
    );
    assert!(
        report
            .exemptions
            .iter()
            .any(|exemption| exemption.test == "drives_a_fake_provider"
                && exemption.reason == ExemptionReason::FakeProvider),
        "the fake-provider test is exempt: {:?}",
        report.exemptions
    );
}

#[test]
fn wait_lint_runs_with_a_scrubbed_environment() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wait-lint"));
    command.current_dir(repo_root()).env_clear();
    let output = command.output().expect("the lint runs");
    assert!(
        output.status.success(),
        "the lint must run without a credential or a network: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("scanned"), "the lint must report what it scanned: {stdout}");
}
