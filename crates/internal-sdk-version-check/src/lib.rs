//! The check that keeps the README's SDK version in step with the published package.
//!
//! The README tells its reader which version of the `@aether-agent/sdk` package the docs describe.
//! This crate compares that version with the version the package publishes, so a README cannot
//! describe a version that was never published. It only reads files: it writes nothing, opens no
//! socket and needs no credential.
//!
//! - [`published_version`] is the version the package publishes: the `version` of the package
//!   manifest, `packages/aether-sdk/package.json`.
//! - [`stated_version`] is the version a README states.
//! - [`check_sdk_version`] compares the two and reports the difference.

use std::fmt;

use serde_json::Value;

/// The npm name of the SDK package the check reads.
pub const SDK_PACKAGE: &str = "@aether-agent/sdk";

/// A version a README states that the package does not publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMismatch {
    /// The version the package publishes.
    pub published: String,
    /// The version the README states.
    pub stated: String,
}

impl fmt::Display for VersionMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the README states SDK version '{}', but the package publishes version '{}'",
            self.stated, self.published
        )
    }
}

/// Why the check could not compare the versions.
#[derive(Debug)]
pub enum SdkVersionError {
    /// The README states no SDK version.
    MissingStatedVersion,
    /// The package manifest holds no version.
    MissingPublishedVersion,
    /// The package manifest is not the JSON the check reads.
    InvalidManifest(serde_json::Error),
    /// A file could not be read.
    Io(std::io::Error),
}

impl fmt::Display for SdkVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingStatedVersion => write!(f, "the README states no version of {SDK_PACKAGE}"),
            Self::MissingPublishedVersion => write!(f, "the package manifest holds no version"),
            Self::InvalidManifest(error) => write!(f, "the package manifest is not valid JSON: {error}"),
            Self::Io(error) => write!(f, "the file could not be read: {error}"),
        }
    }
}

impl std::error::Error for SdkVersionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingStatedVersion | Self::MissingPublishedVersion => None,
            Self::InvalidManifest(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReport {
    /// The version the package publishes.
    pub published: String,
    /// The version the README states.
    pub stated: String,
}

impl VersionReport {
    /// The difference between the two versions, when they differ.
    pub fn mismatch(&self) -> Option<VersionMismatch> {
        if self.published == self.stated {
            return None;
        }
        Some(VersionMismatch { published: self.published.clone(), stated: self.stated.clone() })
    }

    /// Whether the README states the version the package publishes.
    pub fn is_clean(&self) -> bool {
        self.published == self.stated
    }
}

/// The version the SDK package publishes, read from the package manifest.
pub fn published_version(manifest: &str) -> Result<String, SdkVersionError> {
    let manifest: Value = serde_json::from_str(manifest).map_err(SdkVersionError::InvalidManifest)?;
    manifest
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|version| !version.is_empty())
        .ok_or(SdkVersionError::MissingPublishedVersion)
}

/// The version the README states.
///
/// The README states it on the line that names the package and a version, for example
/// "The `@aether-agent/sdk` package is published at version `0.7.1`."
pub fn stated_version(readme: &str) -> Result<String, SdkVersionError> {
    readme.lines().find_map(stated_version_of_line).ok_or(SdkVersionError::MissingStatedVersion)
}

/// Compare the published version with the version the README states.
pub fn check_sdk_version(manifest: &str, readme: &str) -> Result<VersionReport, SdkVersionError> {
    Ok(VersionReport { published: published_version(manifest)?, stated: stated_version(readme)? })
}

/// The version the line states, when the line names the package and a version.
fn stated_version_of_line(line: &str) -> Option<String> {
    let package_at = line.find(SDK_PACKAGE)?;
    let after_package = &line[package_at + SDK_PACKAGE.len()..];
    let version_at = find_word(after_package, "version")?;
    let after_version = after_package[version_at + "version".len()..].trim_start();
    let after_version = after_version.strip_prefix(':').unwrap_or(after_version).trim_start();
    let version = after_version.split_whitespace().next()?;
    let version = version.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    let version = version.trim_matches('`');
    if version.is_empty() { None } else { Some(version.to_string()) }
}

/// The byte offset of the first occurrence of `word` in `text`, ignoring ASCII case.
fn find_word(text: &str, word: &str) -> Option<usize> {
    text.as_bytes().windows(word.len()).position(|window| window.eq_ignore_ascii_case(word.as_bytes()))
}
