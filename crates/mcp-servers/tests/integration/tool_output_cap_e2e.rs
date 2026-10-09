//! End-to-end test for the tool output cap.
//!
//! ACC-2: A `bash` tool call that produces 1 MiB of output must come back to
//! the model as a truncated result containing the marker text, and the full
//! 1 MiB must be readable from the on-disk file the cap wrote.
//!
//! This is the production path end-to-end: the test spins up a real
//! `CodingMcp` server, calls the real `bash` tool over the MCP wire, then
//! runs the response through the production `convert_tool_result` bridge
//! with a small cap, exactly the way the agent loop does at runtime.
//!
//! The bridge serializes the tool's structured result to YAML before capping
//! it; for a payload with non-empty lines (e.g. `y\ny\n...`) the YAML
//! round-trips through `serde_yml` and the file on disk is that YAML, bigger
//! than the raw bash text (YAML's per-line indent inflates it ~3x) but
//! still recoverable byte-for-byte via `serde_yml::from_str`.

use crate::common::{CodingWorkspace, TestResult, test_error};
use aether_core::mcp::tool_bridge::convert_tool_result;
use aether_core::mcp::tool_output::ToolOutputCap;
use llm::ToolCallRequest;
use mcp_servers::coding::tools::bash::BashInput;

/// Cap large enough to fit the truncation marker but small enough to force
/// the head+tail cut on a multi-MiB payload.
const CAP_BYTES: usize = 4096;

/// One mebibyte. The cap must fire on a payload noticeably larger than the
/// cap itself, so 1 MiB is the natural size for the acceptance criterion.
const PAYLOAD_BYTES: usize = 1024 * 1024;

#[tokio::test]
async fn bash_tool_one_mib_output_gets_capped_and_full_bytes_recoverable() -> TestResult {
    let workspace = CodingWorkspace::new().await?;

    let spillover_dir = tempfile::tempdir()?;
    let cap = ToolOutputCap::new(CAP_BYTES, spillover_dir.path().to_path_buf());

    // 1. Drive the real `bash` tool over the MCP wire, producing 1 MiB of
    //    stdout. `yes "y"` outputs `y\n` lines; piped into `head -c` it
    //    terminates cleanly at exactly PAYLOAD_BYTES. Using a non-empty
    //    argument keeps every line non-blank, which is what the bridge's
    //    YAML round-trip filter requires to pick YAML over the JSON fallback.
    let bash_input = BashInput {
        command: format!("yes y | head -c {PAYLOAD_BYTES}"),
        ..Default::default()
    };
    let raw = workspace
        .client
        .call_raw("bash", &bash_input)
        .await
        .map_err(|e| test_error(format!("bash call failed: {e}")))?;

    // 2. Sanity: the bash tool's structured result carries the full 1 MiB
    //    of stdout in its `output` field, before any cap is applied. This
    //    is what the on-disk file must round-trip to. (The MCP framework
    //    mirrors structured_content into `content[0].text` as JSON, so
    //    `content[0].text` itself is the JSON wrapper and is ~1.5x the raw
    //    output — the real bash output lives in `output` inside the JSON.)
    let raw_bash_output = raw
        .structured_content
        .as_ref()
        .and_then(|value| value.get("output"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| test_error("bash structured_content should carry an `output` string field"))?
        .to_owned();
    assert_eq!(
        raw_bash_output.len(),
        PAYLOAD_BYTES,
        "raw bash output should be exactly 1 MiB ({}), got {} bytes",
        PAYLOAD_BYTES,
        raw_bash_output.len(),
    );

    // 3. Run the response through the production bridge with the cap. This
    //    is the exact entry point the agent loop uses at runtime.
    let request = ToolCallRequest {
        id: "bash_cap_e2e".into(),
        name: "bash".into(),
        arguments: serde_json::to_string(&bash_input)?,
    };
    let (capped, _meta) = convert_tool_result(&request, Ok(raw), &cap)
        .map_err(|e| test_error(format!("convert_tool_result failed: {e:?}")))?;

    // 4. The model-visible result must be at most the cap, must start with
    //    the truncation marker, and must contain the on-disk path so the
    //    model can read the file back.
    assert!(
        capped.result.len() <= CAP_BYTES,
        "capped result {} bytes exceeds cap {}",
        capped.result.len(),
        CAP_BYTES,
    );
    assert!(
        capped.result.starts_with("[aether: output truncated;"),
        "capped result missing truncation marker: {}",
        &capped.result[..capped.result.len().min(80)],
    );
    assert!(
        capped.result.contains(" elided "),
        "capped result missing elided byte/line counts: {}",
        &capped.result[..capped.result.len().min(200)],
    );
    assert!(
        capped.result.contains("Full: "),
        "capped result missing on-disk path: {}",
        &capped.result[..capped.result.len().min(200)],
    );

    // 5. The on-disk file the marker points to must hold the full 1 MiB of
    //    bash output, recoverable by parsing the YAML the bridge wrote.
    //    The bridge passes a YAML-serialized structured result
    //    (`{output, exit_code, killed}`) to the cap, so the file on disk is
    //    that YAML — bigger than the raw text (YAML's per-line indent and
    //    the extra fields inflate it) but round-trippable to the exact bash
    //    output via `serde_yml::from_str`.
    let saved_path = spillover_dir
        .path()
        .read_dir()
        .map_err(|e| test_error(format!("read spillover dir: {e}")))?
        .filter_map(Result::ok)
        .next()
        .ok_or_else(|| test_error("cap should have written the full bash output to disk"))?
        .path();
    let on_disk = std::fs::read(&saved_path).map_err(|e| test_error(format!("read saved file: {e}")))?;
    assert!(
        on_disk.len() > CAP_BYTES,
        "saved file should hold more than the cap ({} bytes), got {}",
        CAP_BYTES,
        on_disk.len(),
    );
    let decoded: serde_yml::Value =
        serde_yml::from_slice(&on_disk).map_err(|e| test_error(format!("saved file is not YAML: {e}")))?;
    let recovered = decoded
        .get("output")
        .and_then(|v| v.as_str())
        .ok_or_else(|| test_error("saved YAML should carry an `output` string field"))?;
    assert_eq!(
        recovered.len(),
        PAYLOAD_BYTES,
        "saved YAML's output field should be the full 1 MiB bash output, got {} bytes",
        recovered.len(),
    );
    assert_eq!(recovered, raw_bash_output, "saved YAML's output field should round-trip to the exact bash output");
    // The model-visible path must name the same file the on-disk entry holds,
    // so the model can read it back via grep/read/tail.
    assert!(
        capped.result.contains(saved_path.file_name().unwrap().to_str().unwrap()),
        "capped result should name the saved file {}: {}",
        saved_path.display(),
        &capped.result[..capped.result.len().min(200)],
    );

    // 6. The elided byte count in the marker is sensible: it should be in
    //    the open interval (0, on_disk.len()), since the cap cut some but
    //    not all of the YAML the bridge passed in. (Comparing against
    //    PAYLOAD_BYTES would be wrong — the cap operates on the bridge's
    //    YAML output, not on the raw bash bytes.)
    let elided_marker = capped
        .result
        .split("elided ")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or_else(|| test_error("elided byte count should parse from the marker"))?;
    assert!(
        elided_marker > 0 && elided_marker < on_disk.len(),
        "elided byte count {} should be in (0, {})",
        elided_marker,
        on_disk.len(),
    );

    Ok(())
}