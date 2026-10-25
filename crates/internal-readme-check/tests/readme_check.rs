//! Tests for the README provider check. They drive the public API and the check binary only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use internal_readme_check::{check_providers, harness_providers, readme_providers};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root")
}

fn current_readme() -> String {
    fs::read_to_string(repo_root().join("README.md")).expect("the README reads")
}

/// The current README with one more provider in its provider list.
fn readme_with_provider(name: &str) -> String {
    let readme = current_readme();
    let marker = "- **Your Model**:";
    let position = readme.find(marker).expect("the README names its providers");
    let mut with_provider = readme;
    with_provider.insert_str(position + marker.len(), &format!(" {name},"));
    with_provider
}

fn run_check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_readme-check")).arg(root).output().expect("the check runs")
}

#[test]
fn current_readme_names_only_providers_the_harness_registers() {
    let registered = harness_providers();

    let report = check_providers(&current_readme(), &registered).expect("the README has a provider list");

    assert!(report.is_clean(), "the README names providers the harness does not register: {:?}", report.unregistered);
    assert!(!report.listed.is_empty(), "the README names at least one provider");
    assert!(registered.len() > report.listed.len(), "the harness registers more providers than the README names");
}

#[test]
fn readme_naming_an_unknown_provider_is_reported() {
    let report = check_providers(&readme_with_provider("Nonexistent Provider"), &harness_providers())
        .expect("the README has a provider list");

    assert_eq!(report.unregistered.len(), 1, "{:?}", report.unregistered);
    assert_eq!(report.unregistered[0].name, "Nonexistent Provider");
    assert!(
        report.unregistered[0].to_string().contains("Nonexistent Provider"),
        "the failure must name the provider: {}",
        report.unregistered[0]
    );
}

#[test]
fn readme_providers_are_the_names_the_provider_list_holds() {
    let providers = readme_providers(&current_readme()).expect("the README has a provider list");

    assert!(providers.iter().any(|provider| provider == "Anthropic"), "{providers:?}");
    assert!(providers.iter().any(|provider| provider == "Ollama"), "{providers:?}");
    assert!(!providers.iter().any(|provider| provider.starts_with("and ")), "{providers:?}");
}

#[test]
fn the_check_fails_on_a_readme_naming_an_unknown_provider() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    fs::write(directory.path().join("README.md"), readme_with_provider("Nonexistent Provider"))
        .expect("the README writes");

    let output = run_check(directory.path());

    assert!(!output.status.success(), "the check must fail on a README naming an unknown provider");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Nonexistent Provider"), "the failure must name the provider: {stderr}");
}

#[test]
fn the_check_passes_on_the_current_readme() {
    let output = run_check(&repo_root());

    assert!(
        output.status.success(),
        "the check must pass on the current README: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
