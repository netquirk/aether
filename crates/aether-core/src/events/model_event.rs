use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Model configuration events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    /// The model was successfully switched.
    Switched { previous: String, new: String },
    /// The primary provider failed with a server (5xx) error and the agent
    /// retried the turn on a configured secondary provider. `from` is the
    /// primary's display name, `to` is the secondary's display name, and
    /// `reason` is the textual description of the 5xx that triggered the
    /// fallback. Emitted exactly once per run, on the first server error.
    Fallback { from: String, to: String, reason: String },
}
