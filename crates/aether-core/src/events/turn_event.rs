use llm::{
    ContentBlock, LlmCallPurpose, LlmError, MessageId, ModelIdentity, ProviderErrorKind, StopReason, TokenUsage,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a turn reached its terminal state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed {
        error: String,
    },
    /// The configured per-run turn cap was reached. The run ends cleanly
    /// (no failure) and the cap is reported in the outcome payload so callers
    /// can surface it.
    MaxTurnsReached {
        max_turns: u32,
    },
}

/// How a single LLM call ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LlmCallOutcome {
    Completed {
        stop_reason: Option<StopReason>,
        usage: Option<TokenUsage>,
    },
    Failed {
        error: String,
        will_retry: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_error_code: Option<String>,
        /// Normalized provider failure classification carried over from the
        /// original [`LlmError`]. `None` for client-side errors (missing API
        /// key, OAuth flow, argument validation, etc.) and for the legacy
        /// `failed` constructor; the headless CLI uses
        /// `Some(ProviderErrorKind::Authentication)` to pick a distinct exit
        /// code when the model *rejected* the credential (HTTP 401/403).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ProviderErrorKind>,
    },
    Cancelled,
}

impl LlmCallOutcome {
    pub fn failed(error: impl Into<String>, will_retry: bool) -> Self {
        Self::Failed {
            error: error.into(),
            will_retry,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
            kind: None,
        }
    }

    pub fn from_llm_error(error: &LlmError, will_retry: bool) -> Self {
        let Some(provider) = error.provider() else {
            return Self::failed(error.to_string(), will_retry);
        };
        Self::Failed {
            error: provider.to_string(),
            will_retry,
            http_status: provider.http_status,
            provider_request_id: provider.request_id.clone(),
            provider_error_code: provider.code.clone(),
            kind: Some(provider.kind),
        }
    }
}

/// A retry of a failed LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: u64,
}

/// Turn lifecycle events.
///
/// A turn spans from a user message to a terminal [`TurnEvent::Ended`]. Within a
/// turn, each LLM call is bracketed by `LlmCallStarted`/`LlmCallEnded`; retries
/// surface as an `LlmCallStarted` with `attempt > 0`. Note that the completion
/// events for streamed message content
/// ([`MessageEvent`](crate::events::MessageEvent) with `is_complete: true`) are
/// emitted at turn completion, after the originating call's `LlmCallEnded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    /// A user message began a turn. Messages queued while a turn is active are
    /// folded into that turn and do not start a new one.
    Started {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
    },
    /// A retry is waiting for its backoff delay before the request starts.
    RetryScheduled { purpose: LlmCallPurpose, attempt: u32, max_attempts: u32, delay_ms: u64 },
    /// An LLM request was issued.
    LlmCallStarted {
        purpose: LlmCallPurpose,
        model: ModelIdentity,
        display_name: String,
        /// 0 for the initial call, incrementing per retry.
        attempt: u32,
        max_attempts: u32,
    },
    /// An LLM call reached a terminal state.
    LlmCallEnded { purpose: LlmCallPurpose, outcome: LlmCallOutcome },
    /// The agent is auto-continuing because the LLM stopped with a resumable
    /// stop reason.
    AutoContinue { attempt: u32, max_attempts: u32, message_id: MessageId, content: Vec<ContentBlock> },
    /// The turn reached a terminal state.
    Ended { outcome: TurnOutcome },
}

impl TurnEvent {
    pub fn retry_info(&self) -> Option<RetryInfo> {
        match self {
            Self::RetryScheduled { attempt, max_attempts, delay_ms, .. } => {
                Some(RetryInfo { attempt: *attempt, max_attempts: *max_attempts, delay_ms: *delay_ms })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use llm::ProviderError;

    use super::*;

    #[test]
    fn from_llm_error_carries_provider_kind_for_authorization_failures() {
        // `ProviderError::authentication(...)` is the canonical shape an HTTP
        // 401/403 surfaces through the reqwest/openai adapters: the runtime
        // uses it to gate the distinct auth exit code, so the mapping has to
        // land `kind = Some(ProviderErrorKind::Authentication)`.
        let outcome = LlmCallOutcome::from_llm_error(&LlmError::from(ProviderError::authentication("bad key")), false);
        match outcome {
            LlmCallOutcome::Failed { kind, will_retry, .. } => {
                assert_eq!(kind, Some(ProviderErrorKind::Authentication));
                assert!(!will_retry);
            }
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    #[test]
    fn from_llm_error_carries_provider_kind_for_generic_api_failures() {
        // Non-auth provider failures still carry their kind so callers can
        // classify `Timeout`/`RateLimit`/`Server`/etc. without string-matching
        // the error message.
        let outcome = LlmCallOutcome::from_llm_error(&LlmError::from(ProviderError::api("boom")), false);
        match outcome {
            LlmCallOutcome::Failed { kind, .. } => assert_eq!(kind, Some(ProviderErrorKind::Api)),
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }

    #[test]
    fn from_llm_error_leaves_kind_none_for_client_side_errors() {
        // `MissingApiKey` is a client-side "you forgot to configure it"
        // failure: it must NOT be mapped to the auth exit code, so the kind
        // is intentionally `None` and the headless CLI treats it as a
        // generic task failure.
        let outcome = LlmCallOutcome::from_llm_error(&LlmError::MissingApiKey("OPENAI_API_KEY".into()), false);
        match outcome {
            LlmCallOutcome::Failed { kind, .. } => assert_eq!(kind, None),
            other => panic!("expected Failed outcome, got {other:?}"),
        }
    }
}
