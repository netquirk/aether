//! Hooks that decide whether a tool call the model requested should be executed.
//!
//! [`ToolPolicy`] is consulted before an [`Agent`](crate::core::Agent) starts executing a tool
//! call. Returning a reason refuses the call: it never executes, a
//! [`ToolEvent::Refused`](crate::events::ToolEvent::Refused) is emitted on the run's transcript,
//! and a refusal message is appended to the model's context so it can react.

use llm::ToolCallRequest;

/// Decides whether to refuse executing a tool call before it is dispatched.
pub trait ToolPolicy: Send + Sync {
    /// Return `Some(reason)` to refuse executing `request`; `None` allows the call to proceed.
    fn refuse(&self, request: &ToolCallRequest) -> Option<String>;
}

/// Default policy that refuses nothing; the agent executes every tool call the model requests.
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllTools;

impl ToolPolicy for AllowAllTools {
    fn refuse(&self, _request: &ToolCallRequest) -> Option<String> {
        None
    }
}
