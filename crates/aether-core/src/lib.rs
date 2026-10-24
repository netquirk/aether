#![doc = include_str!("../README.md")]

pub mod agent_spec;
pub mod context;
pub mod core;
pub mod events;
pub mod mcp;
#[cfg(feature = "testing")]
pub mod testing;

pub use agent_spec::{AgentSpec, AgentSpecExposure};

// The workspace README, whose "Using Aether as a library" example is compiled as a doc test. The
// README is included from the repository root, so an edit to the README changes what
// `cargo test --doc` builds. The example is `no_run`: it is type-checked against this crate but
// never executed, so the check needs no provider, no network and no credential. The item carries
// no doc comment of its own, so rustdoc reports the README file and line the example sits on.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct WorkspaceReadme;
