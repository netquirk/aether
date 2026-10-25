//! The check that keeps the README's provider list in step with the harness.
//!
//! The README promises a set of LLM providers "supported out of the box". This crate compares
//! the providers the README names with the providers the harness registers, so a provider that
//! does not exist cannot be advertised. It only reads the README: it writes nothing, opens no
//! socket and needs no credential.
//!
//! - [`check_providers`] is the check: it takes the README's text and the registered providers
//!   and reports the providers the README names that the harness does not register.
//! - [`harness_providers`] is the registry the check compares against: the providers the
//!   harness registers.

use std::fmt;

use llm::catalog::Provider;
use llm::parser::ModelProviderParser;

/// The marker that introduces the README's provider list.
const PROVIDER_LIST_MARKER: &str = "**Your Model**";

/// The words that end the README's provider list.
const LIST_ENDINGS: [&str; 3] = [" are ", " is ", ". "];

/// A provider the harness registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredProvider {
    /// The name a user writes in a `provider:model` string, e.g. `azure-foundry`.
    pub parser: String,
    /// The human-readable name, e.g. `Microsoft Foundry`.
    pub display: String,
}

/// A provider the README names that the harness does not register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnregisteredProvider {
    /// The name the README uses.
    pub name: String,
}

impl fmt::Display for UnregisteredProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the README names provider '{}', which the harness does not register", self.name)
    }
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderReport {
    /// The providers the README names.
    pub listed: Vec<String>,
    /// The providers the README names that the harness does not register.
    pub unregistered: Vec<UnregisteredProvider>,
}

impl ProviderReport {
    /// Whether every provider the README names is one the harness registers.
    pub fn is_clean(&self) -> bool {
        self.unregistered.is_empty()
    }
}

/// Why the check could not read the README's provider list.
#[derive(Debug)]
pub enum ReadmeError {
    /// The README has no provider list.
    MissingProviderList,
    /// The README could not be read.
    Io(std::io::Error),
}

impl fmt::Display for ReadmeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingProviderList => {
                write!(f, "the README has no '{PROVIDER_LIST_MARKER}' provider list")
            }
            Self::Io(error) => write!(f, "the README could not be read: {error}"),
        }
    }
}

impl std::error::Error for ReadmeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingProviderList => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// The providers the harness registers.
///
/// A provider counts as registered when the harness's provider parser holds a factory for it, or
/// when the harness registers it together with a credential store (the OAuth providers).
pub fn harness_providers() -> Vec<RegisteredProvider> {
    let parser = ModelProviderParser::default();
    let registered = parser.registered_providers();
    Provider::ALL
        .iter()
        .filter(|provider| registered.contains(&provider.parser_name()) || provider.oauth_provider_id().is_some())
        .map(|provider| RegisteredProvider {
            parser: provider.parser_name().to_string(),
            display: provider.display_name().to_string(),
        })
        .collect()
}

/// The providers the README names in its provider list.
pub fn readme_providers(readme: &str) -> Result<Vec<String>, ReadmeError> {
    Ok(provider_list(readme)?
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.strip_prefix("and ").unwrap_or(item).trim().to_string())
        .collect())
}

/// Compare the providers the README names with the providers the harness registers.
///
/// The comparison is one-way: a provider the harness registers but the README does not name is
/// fine, a provider the README names that the harness does not register is reported.
pub fn check_providers(readme: &str, registered: &[RegisteredProvider]) -> Result<ProviderReport, ReadmeError> {
    let listed = readme_providers(readme)?;
    let unregistered = listed
        .iter()
        .filter(|name| !is_registered(name, registered))
        .map(|name| UnregisteredProvider { name: name.clone() })
        .collect();
    Ok(ProviderReport { listed, unregistered })
}

/// The text of the README's provider list.
fn provider_list(readme: &str) -> Result<&str, ReadmeError> {
    let line =
        readme.lines().find(|line| line.contains(PROVIDER_LIST_MARKER)).ok_or(ReadmeError::MissingProviderList)?;
    let after_marker = line.split_once(PROVIDER_LIST_MARKER).map_or(line, |(_, rest)| rest);
    let list = after_marker.trim_start().trim_start_matches(':').trim_start();
    let end = LIST_ENDINGS.iter().filter_map(|ending| list.find(ending)).min().unwrap_or(list.len());
    Ok(&list[..end])
}

/// Whether the harness registers the provider the README names.
///
/// A name matches a provider when it is the provider's display name or the name a user writes in
/// a `provider:model` string, compared without regard to case and punctuation: `Llama.cpp` is the
/// `LlamaCpp` provider and `Microsoft Foundry` is the `azure-foundry` provider.
fn is_registered(name: &str, registered: &[RegisteredProvider]) -> bool {
    let wanted = normalize(name);
    registered.iter().any(|provider| normalize(&provider.display) == wanted || normalize(&provider.parser) == wanted)
}

/// The name a provider is known by: lower case, without punctuation.
fn normalize(name: &str) -> String {
    name.chars().filter(|character| character.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}
