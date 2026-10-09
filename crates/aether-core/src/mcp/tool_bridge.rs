use crate::events::{TaskOutcome, TaskOutcomeState};
use crate::mcp::tool_output::{ToolOutputCap, cap_tool_output};
use mcp_utils::{
    client::{CallToolError, SERVERNAME_DELIMITER},
    display_meta::ToolResultMeta,
};
use rmcp::model::{CallToolRequestParams, CallToolResult, Task};
use serde_json;

use llm::{ToolCallError, ToolCallRequest, ToolCallResult};

/// Convert a `ToolCallRequest` to `rmcp::CallToolRequestParams`
pub fn tool_call_request_to_mcp(request: &ToolCallRequest) -> Result<CallToolRequestParams, String> {
    let tool_name = request
        .name
        .split_once(SERVERNAME_DELIMITER)
        .map_or_else(|| request.name.clone(), |(_, tool_name)| tool_name.to_string());

    // Parse arguments from JSON string
    let arguments = serde_json::from_str::<serde_json::Value>(&request.arguments)
        .map_err(|e| format!("Invalid tool arguments: {e}"))?
        .as_object()
        .cloned();

    let mut params = CallToolRequestParams::new(tool_name);
    if let Some(args) = arguments {
        params = params.with_arguments(args);
    }
    Ok(params)
}

/// Convert an rmcp `CallToolResult` and request to `ToolCallResult` or `ToolCallError`,
/// extracting any `_meta` metadata from structured content.
///
/// `cap` enforces the per-call result byte budget and writes the full output
/// to disk when the result is over the budget. Pass a disabled cap (`max_bytes
/// == 0`) to skip truncation.
pub fn mcp_result_to_tool_call_result(
    request: &ToolCallRequest,
    mcp_result: rmcp::model::CallToolResult,
    cap: &ToolOutputCap,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
    if mcp_result.is_error.unwrap_or(false) {
        let error_msg = mcp_result.content.first().map_or_else(
            || "Unknown error".to_string(),
            |content| {
                content.as_text().map_or_else(
                    || serde_json::to_string(content).unwrap_or_else(|_| "Unknown error".to_string()),
                    |text| text.text.clone(),
                )
            },
        );
        Err(ToolCallError {
            id: request.id.clone(),
            name: request.name.clone(),
            arguments: Some(request.arguments.clone()),
            error: format!("Tool execution error: {error_msg}"),
        })
    } else {
        let (result_value, result_meta) = extract_result_and_meta(mcp_result.structured_content, &mcp_result.content);
        // YAML is ~18% more token-efficient than JSON for LLM consumption
        let yaml = serde_yml::to_string(&result_value)
            .ok()
            .filter(|yaml| serde_yml::from_str::<serde_json::Value>(yaml).is_ok_and(|decoded| decoded == result_value))
            .unwrap_or_else(|| result_value.to_string());
        let capped = cap_tool_output(cap, &yaml);
        Ok((
            ToolCallResult {
                id: request.id.clone(),
                name: request.name.clone(),
                arguments: request.arguments.clone(),
                result: capped.text,
            },
            result_meta,
        ))
    }
}

pub fn map_task_result_to_outcome(
    request: ToolCallRequest,
    task: Task,
    outcome: Result<CallToolResult, CallToolError>,
    cap: &ToolOutputCap,
) -> TaskOutcome {
    let state = match convert_tool_result(&request, outcome, cap) {
        Ok((result, result_meta)) => TaskOutcomeState::Completed { result, result_meta },
        Err(error) => TaskOutcomeState::Failed { error },
    };
    TaskOutcome { request, task_id: task.task_id, state }
}

pub fn convert_tool_result(
    request: &ToolCallRequest,
    outcome: Result<CallToolResult, CallToolError>,
    cap: &ToolOutputCap,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
    outcome
        .map_err(|error| ToolCallError::from_request(request, error.to_string()))
        .and_then(|mcp_result| mcp_result_to_tool_call_result(request, mcp_result, cap))
}

fn extract_result_and_meta(
    structured_content: Option<serde_json::Value>,
    content: &[rmcp::model::ContentBlock],
) -> (serde_json::Value, Option<ToolResultMeta>) {
    if let Some(mut val) = structured_content {
        let result_meta = extract_result_meta(&mut val);
        (val, result_meta)
    } else {
        let fallback = content.first().map_or_else(
            || serde_json::Value::String("No result".to_string()),
            |c| serde_json::to_value(c).unwrap_or(serde_json::Value::String("Serialization error".to_string())),
        );
        (fallback, None)
    }
}

fn extract_result_meta(value: &mut serde_json::Value) -> Option<ToolResultMeta> {
    let obj = value.as_object_mut()?;
    let parsed: ToolResultMeta = {
        let meta = obj.get("_meta")?.as_object()?;
        serde_json::from_value(serde_json::Value::Object(meta.clone())).ok()?
    };

    let meta_empty = {
        let meta = obj.get_mut("_meta")?.as_object_mut()?;
        for key in ["display", "file_diff", "plan"] {
            meta.remove(key);
        }
        meta.is_empty()
    };

    if meta_empty {
        obj.remove("_meta");
    }

    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_utils::display_meta::PlanMetaStatus;
    use rmcp::model::{CallToolResult as McpCallToolResult, ContentBlock};
    use serde::Serialize;
    use serde_json::json;
    use std::sync::Arc;

    fn req() -> ToolCallRequest {
        ToolCallRequest { id: "call_123".into(), name: "test_tool".into(), arguments: "{}".into() }
    }

    /// Tests use a cap with the budget high enough that the cap is
    /// effectively a no-op; the cap's own behaviour is tested in
    /// `mcp::tool_output::tests`. These tests focus on the bridge between
    /// MCP results and `ToolCallResult`.
    fn no_op_cap() -> Arc<ToolOutputCap> {
        let dir = tempfile::tempdir().unwrap();
        Arc::new(ToolOutputCap::new(usize::MAX, dir.keep()))
    }

    fn call_structured(structured: serde_json::Value) -> (ToolCallResult, Option<ToolResultMeta>) {
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![];
        let cap = no_op_cap();
        mcp_result_to_tool_call_result(&req(), mcp, &cap).unwrap()
    }

    #[test]
    fn test_extracts_and_strips_meta() {
        let structured = json!({
            "status": "success", "file_path": "/test/file.rs",
            "_meta": { "display": { "title": "Read file", "value": "file.rs, 50 lines" } }
        });
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![ContentBlock::text("plain text fallback")];
        let cap = no_op_cap();
        let (result, meta) = mcp_result_to_tool_call_result(&req(), mcp, &cap).unwrap();

        assert!(!result.result.contains("_meta"));
        assert!(result.result.contains("success"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Read file");
        assert_eq!(rm.display.value, "file.rs, 50 lines");
        assert!(rm.file_diff.is_none());
    }

    #[test]
    fn test_extracts_meta_with_file_diff() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Edit file", "value": "main.rs" },
                "file_diff": { "path": "/tmp/main.rs", "old_text": "old content", "new_text": "new content" }
            }
        }));
        assert!(!result.result.contains("_meta"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Edit file");
        let fd = rm.file_diff.expect("file_diff should be present");
        assert_eq!(fd.path, "/tmp/main.rs");
        assert_eq!(fd.old_text.as_deref(), Some("old content"));
        assert_eq!(fd.new_text.as_deref(), Some("new content"));
    }

    #[test]
    fn test_extracts_known_meta_and_preserves_unknown_meta_keys() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Edit file", "value": "main.rs" },
                "file_diff": { "path": "/tmp/main.rs", "old_text": "old", "new_text": "new" },
                "trace_id": "trace-123", "duration_ms": 18
            }
        }));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Edit file");
        assert!(rm.file_diff.is_some());
        for absent in ["display:", "file_diff:"] {
            assert!(!result.result.contains(absent));
        }
        for present in ["trace_id:", "trace-123", "duration_ms:", "18"] {
            assert!(result.result.contains(present));
        }
    }

    #[test]
    fn test_malformed_meta_returns_none() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": { "display": "not a valid ToolDisplayMeta" }
        }));
        assert!(meta.is_none());
        assert!(result.result.contains("not a valid ToolDisplayMeta"));
    }

    #[test]
    fn structured_strings_preserve_trailing_whitespace() {
        for text in ["", "plain", "line\n", "line\n\n", "line\n\n\n", "\n", "\n\n", "  line\n\n", "line\n  "] {
            for value in
                [json!(text), json!({"first": text, "last": "end"}), json!({"nested": [text]}), json!({"last": text})]
            {
                let (result, _) = call_structured(value.clone());
                let decoded: serde_json::Value = serde_yml::from_str(&result.result).unwrap();
                assert_eq!(decoded, value, "encoded result: {:?}", result.result);
            }
        }
    }

    #[test]
    fn test_no_meta_passes_through_unchanged() {
        let (result, meta) = call_structured(json!({"status": "success", "data": "hello"}));
        assert!(result.result.contains("success"));
        assert!(result.result.contains("hello"));
        assert!(meta.is_none());
    }

    #[test]
    fn test_tool_call_result_falls_back_to_content() {
        let mcp = McpCallToolResult::success(vec![ContentBlock::text("plain text result")]);
        let cap = no_op_cap();
        let (result, meta) = mcp_result_to_tool_call_result(&req(), mcp, &cap).unwrap();
        assert!(result.result.contains("plain text result"));
        assert!(meta.is_none());
    }

    #[test]
    fn test_extracts_meta_with_plan() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Todo", "value": "Research AI agents" },
                "plan": { "entries": [
                    { "content": "Research AI agents", "status": "in_progress" },
                    { "content": "Write tests", "status": "pending" }
                ]}
            }
        }));
        assert!(!result.result.contains("_meta"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Todo");
        let plan = rm.plan.expect("plan should be present");
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].content, "Research AI agents");
        assert_eq!(plan.entries[0].status, PlanMetaStatus::InProgress);
        assert_eq!(plan.entries[1].status, PlanMetaStatus::Pending);
    }

    /// Regression: verifies `#[serde(rename = "_meta")]` preserves the key under camelCase,
    /// and that omitting the rename breaks extraction.
    #[test]
    fn test_meta_camel_case_serde_round_trip() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct GoodResult {
            file_path: String,
            total_lines: usize,
            #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
            _meta: Option<serde_json::Value>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct BrokenResult {
            file_path: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            _meta: Option<serde_json::Value>,
        }

        let display_meta = json!({
            "display": { "title": "Read file", "value": "file.rs, 50 lines" }
        });

        let good = serde_json::to_value(&GoodResult {
            file_path: "/test/file.rs".into(),
            total_lines: 50,
            _meta: Some(display_meta.clone()),
        })
        .unwrap();
        assert!(good.get("_meta").is_some(), "expected `_meta` key, got: {good}");
        let (stripped, meta) = extract_result_and_meta(Some(good), &[]);
        let rm = meta.expect("meta should be extracted");
        assert_eq!(rm.display.title, "Read file");
        assert_eq!(rm.display.value, "file.rs, 50 lines");
        assert!(stripped.get("_meta").is_none());

        let broken =
            serde_json::to_value(&BrokenResult { file_path: "/test/file.rs".into(), _meta: Some(display_meta) })
                .unwrap();
        assert!(broken.get("_meta").is_none(), "should be mangled by camelCase");
        assert!(broken.get("meta").is_some());
        let (_, meta) = extract_result_and_meta(Some(broken), &[]);
        assert!(meta.is_none(), "extraction should fail when _meta is mangled");
    }

    #[test]
    fn test_tool_call_result_handles_text_error_without_sdk_debug_output() {
        let mcp = McpCallToolResult::error(vec![ContentBlock::text("Error: file not found")]);
        let cap = no_op_cap();
        let err = mcp_result_to_tool_call_result(&req(), mcp, &cap).unwrap_err();
        assert_eq!(err.error, "Tool execution error: Error: file not found");
    }

    #[test]
    fn test_tool_call_result_serializes_non_text_error_content() {
        let image = serde_json::from_value(serde_json::json!({
            "type": "image",
            "data": "aW1hZ2U=",
            "mimeType": "image/png"
        }))
        .unwrap();
        let mcp = McpCallToolResult::error(vec![image]);
        let cap = no_op_cap();
        let err = mcp_result_to_tool_call_result(&req(), mcp, &cap).unwrap_err();
        assert_eq!(err.error, r#"Tool execution error: {"type":"image","data":"aW1hZ2U=","mimeType":"image/png"}"#);
    }

    #[test]
    fn test_result_is_yaml_format() {
        let (result, _) = call_structured(json!({
            "status": "success",
            "files": [{"name": "Cargo.toml", "path": "./Cargo.toml"}, {"name": "src", "path": "./src"}],
            "totalCount": 2
        }));
        let r = &result.result;
        for expected in ["status: success", "totalCount: 2", "- name:"] {
            assert!(r.contains(expected), "expected '{expected}' in YAML, got: {r}");
        }
        assert!(!r.starts_with('{'), "expected YAML, not JSON: {r}");
    }

    #[test]
    fn test_serde_yml_produces_yaml_not_json() {
        let yaml = serde_yml::to_string(&json!({"key": "value"})).unwrap();
        assert!(yaml.contains("key:") && yaml.contains("value") && !yaml.starts_with('{'));
    }

    /// Bridge-level integration: a structured result over the cap is truncated
    /// to the cap and the on-disk file holds the full YAML text. The
    /// cap's own head+tail logic is exercised in `mcp::tool_output::tests`.
    #[test]
    fn oversized_structured_result_truncates_and_saves_full_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let cap = Arc::new(ToolOutputCap::new(256, dir.path().to_path_buf()));
        let request = ToolCallRequest { id: "bridge_cap".into(), name: "big_tool".into(), arguments: "{}".into() };
        let payload = json!({
            "head": "HEAD_SENTINEL",
            "data": "x".repeat(64 * 1024),
            "tail": "TAIL_SENTINEL",
        });
        let mut mcp = McpCallToolResult::structured(payload);
        mcp.content = vec![];
        let (result, _) = mcp_result_to_tool_call_result(&request, mcp, &cap).unwrap();

        assert!(result.result.len() <= 256, "result length {} exceeds cap 256", result.result.len());
        assert!(result.result.starts_with("[aether: output truncated;"));
        // Both head and tail are preserved in the preview; only the middle of
        // the data blob is elided. The on-disk file still holds the full
        // text so the model can read it back in ranges.
        assert!(result.result.contains("HEAD_SENTINEL"));
        assert!(result.result.contains("TAIL_SENTINEL"));
        // The elided marker advertises how much of the data was dropped.
        assert!(result.result.contains(" elided "));
        let saved = std::fs::read_dir(dir.path()).unwrap().find_map(Result::ok).expect("cap wrote the full output to disk");
        let on_disk = std::fs::read_to_string(saved.path()).unwrap();
        // The file holds the full YAML-encoded result byte-for-byte.
        assert!(on_disk.contains("TAIL_SENTINEL"));
        assert!(on_disk.contains("HEAD_SENTINEL"));
        assert!(on_disk.len() > 256, "on-disk file should hold the full ~64 KiB payload, got {}", on_disk.len());
    }
}
