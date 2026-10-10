use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{TokenUsage, ToolCallRequest};

#[doc = include_str!("docs/stop_reason.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
    Unknown(String),
}

#[doc = include_str!("docs/llm_response.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LlmResponse {
    Start,
    Text {
        chunk: String,
    },
    Reasoning {
        chunk: String,
    },
    EncryptedReasoning {
        id: String,
        content: String,
    },
    ToolRequestStart {
        id: String,
        name: String,
    },
    ToolRequestArg {
        id: String,
        chunk: String,
    },
    ToolRequestComplete {
        tool_call: ToolCallRequest,
    },
    Done {
        stop_reason: Option<StopReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_request_id: Option<String>,
    },
    Error {
        message: String,
    },
    Usage {
        #[serde(flatten)]
        tokens: TokenUsage,
    },
}

impl LlmResponse {
    pub fn text(chunk: &str) -> Self {
        Self::Text { chunk: chunk.to_string() }
    }

    pub fn reasoning(chunk: &str) -> Self {
        Self::Reasoning { chunk: chunk.to_string() }
    }

    pub fn encrypted_reasoning(id: &str, encrypted: &str) -> Self {
        Self::EncryptedReasoning { id: id.to_string(), content: encrypted.to_string() }
    }

    pub fn tool_request_start(id: &str, name: &str) -> Self {
        Self::ToolRequestStart { id: id.to_string(), name: name.to_string() }
    }

    pub fn tool_request_arg(id: &str, chunk: &str) -> Self {
        Self::ToolRequestArg { id: id.to_string(), chunk: chunk.to_string() }
    }

    pub fn tool_request_complete(id: &str, name: &str, arguments: &str) -> Self {
        Self::ToolRequestComplete {
            tool_call: ToolCallRequest { id: id.to_string(), name: name.to_string(), arguments: arguments.to_string() },
        }
    }

    pub fn usage(input_tokens: u64, output_tokens: u64) -> Self {
        Self::Usage { tokens: TokenUsage::new(input_tokens, output_tokens) }
    }

    pub fn done() -> Self {
        Self::Done { stop_reason: None, provider_request_id: None }
    }

    pub fn done_with_stop_reason(stop_reason: StopReason) -> Self {
        Self::Done { stop_reason: Some(stop_reason), provider_request_id: None }
    }

    /// Construct a terminal `Done` event that carries the provider-side request id
    /// (taken from the response body or from response headers by the parser that
    /// observed it). When the provider does not return an id, callers should still
    /// surface `None` — there is no synthetic placeholder.
    pub fn done_with_request_id(stop_reason: Option<StopReason>, provider_request_id: Option<String>) -> Self {
        Self::Done { stop_reason, provider_request_id }
    }
}
