//! Per-line trace logging for the stdio ACP transport (TASK-25-467).
//!
//! The ACP server installs a one-line debug callback on its `Stdio`
//! transport only when `--log-level trace` is set, so a trace run records
//! every JSON-RPC line the server reads and writes as a single
//! `tracing::trace!` event. The recorded payload intentionally carries
//! only direction and method (or the literal `response` for replies), so
//! request parameters and result data stay out of the log file.
//!
//! Lower log levels never enter this module: `AcpArgs::run_acp` only
//! attaches the callback when `args.log_level == Some(LogLevel::Trace)`,
//! so the function below is never installed for non-trace runs.

use agent_client_protocol::LineDirection;

/// `Stdio::with_debug` callback shape. The vendored ACP crate calls this
/// once per incoming (`Stdin`) or outgoing (`Stdout`) line of framed
/// JSON-RPC text. `Stderr` frames are also delivered; the function
/// silently ignores them because they originate from the subprocess's
/// own stderr (captured by `AcpAgent`) and carry no protocol
/// information.
pub(crate) fn log_acp_message(line: &str, direction: LineDirection) {
    let direction_label = match direction {
        LineDirection::Stdin => "recv",
        LineDirection::Stdout => "send",
        LineDirection::Stderr => return,
    };

    match acp_message_method(line) {
        Some(method) => {
            tracing::trace!(target: "aether::acp", "acp {direction_label} {method}");
        }
        None => {
            tracing::trace!(target: "aether::acp", "acp {direction_label} response");
        }
    }
}

/// Extract the JSON-RPC `method` string from a single frame.
///
/// Returns `None` for parsed objects that have no `method` (i.e.
/// responses and error envelopes) and for frames that are not valid JSON
/// at all. JSON-RPC batches are uncommon in practice but handled: the
/// first entry with a `method` string wins. The function is shared by
/// the unit tests below to keep the wire-format quirks covered without
/// having to spin up the ACP server.
fn acp_message_method(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    extract_method(&value)
}

fn extract_method(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map.get("method").and_then(serde_json::Value::as_str).map(str::to_string),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(method) = extract_method(item) {
                    return Some(method);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_method_returns_method_for_single_request() {
        let line = r#"{"jsonrpc":"2.0","method":"initialize","params":{},"id":1}"#;
        assert_eq!(acp_message_method(line).as_deref(), Some("initialize"));
    }

    #[test]
    fn extract_method_returns_none_for_response_without_method() {
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert_eq!(acp_message_method(line), None);
    }

    #[test]
    fn extract_method_returns_none_for_error_envelope() {
        let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        assert_eq!(acp_message_method(line), None);
    }

    #[test]
    fn extract_method_handles_batch_with_leading_request() {
        let line = r#"[{"jsonrpc":"2.0","method":"foo","id":1},{"jsonrpc":"2.0","id":2,"result":null}]"#;
        assert_eq!(acp_message_method(line).as_deref(), Some("foo"));
    }

    #[test]
    fn extract_method_handles_batch_with_leading_response() {
        // Batches may legitimately have a response before a request:
        // the array is read in order and the first `method` key wins.
        let line = r#"[{"jsonrpc":"2.0","id":2,"result":null},{"jsonrpc":"2.0","method":"bar","id":3}]"#;
        assert_eq!(acp_message_method(line).as_deref(), Some("bar"));
    }

    #[test]
    fn extract_method_returns_none_for_batch_without_any_method() {
        let line = r#"[{"jsonrpc":"2.0","id":2,"result":null}]"#;
        assert_eq!(acp_message_method(line), None);
    }

    #[test]
    fn extract_method_returns_none_for_garbage_input() {
        assert!(acp_message_method("not json at all").is_none());
    }

    #[test]
    fn extract_method_returns_none_for_top_level_non_object_array() {
        // Top-level non-object, non-array JSON has no `method` to extract.
        assert!(acp_message_method("42").is_none());
        assert!(acp_message_method("null").is_none());
    }
}
