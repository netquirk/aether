//! Cap the size of tool results before they reach the model context.
//!
//! The cap keeps the head and tail of the text the model would otherwise see,
//! writes the full output to disk so the model can read it back in ranges, and
//! embeds a marker in the returned text naming the elided byte and line counts
//! and the on-disk path. The on-disk filename is a hex content hash, so the
//! same `(full, max_bytes)` pair produces the same capped text on every call —
//! including across runs — which keeps prompt caches warm.
//!
//! Disable the cap for a session by setting [`ToolOutputCap::max_bytes`] to
//! `0` (the per-agent off switch in `settings.json` or the
//! `AETHER_TOOL_OUTPUT_MAX_BYTES` env override both map to that value).

use base16ct::lower;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The default cap for tool result text. 16 KiB keeps the model context small
/// while leaving room for tens of thousands of typical log lines.
pub const DEFAULT_MAX_BYTES: usize = 16 * 1024;

/// Default output directory the cap writes full truncated tool outputs to
/// when no settings or env override is set. Resolved relative to the project
/// root (so `<root>/.prairie/out`).
pub const DEFAULT_TOOL_OUTPUT_DIR: &str = ".prairie/out";

/// A cap smaller than this still works, but the marker is longer than the
/// remaining room for any preview, so the function falls back to the
/// uncapped text. Caps below this threshold are not useful.
const MIN_USABLE_MAX_BYTES: usize = 64;

/// The on-disk directory the cap writes the full output to when truncation
/// fires. When unset, defaults to `<workspace>/.prairie/out`, or the directory
/// named by the `PRAIRIE_TOOL_OUTPUT_DIR` env var when it is set.
#[derive(Debug)]
pub struct ToolOutputCap {
    /// `0` disables the cap and the on-disk spill. Any other value is the
    /// maximum byte length of the text the model sees.
    max_bytes: usize,
    /// Directory the saved file is written under.
    output_dir: PathBuf,
}

impl ToolOutputCap {
    /// Build a cap from the already-resolved byte budget and output directory.
    /// `max_bytes == 0` disables the cap.
    pub fn new(max_bytes: usize, output_dir: PathBuf) -> Self {
        Self { max_bytes, output_dir }
    }

    /// The byte budget the cap enforces on the model-visible text.
    /// `0` means the cap is disabled.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// The directory the cap writes the full output under.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }
}

/// Per-call tool result cap settings. Every field is optional so a partial
/// override inherits the missing piece from lower-precedence layers; the
/// resolution function in `aether-project` walks layers to produce the final
/// [`ToolOutputCap`].
///
/// Used both in the top-level `AetherSettings` and as a per-agent override on
/// `AgentConfig`. Env vars (`AETHER_TOOL_OUTPUT_MAX_BYTES`,
/// `PRAIRIE_TOOL_OUTPUT_DIR`) always win over settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolOutputSettings {
    /// Maximum byte length of the model-visible tool result text. Results over
    /// this size are truncated to a head+tail preview with a marker naming the
    /// on-disk file. `0` disables the cap (the full result is shown as-is);
    /// unset inherits the value from the top-level `toolOutput` block, the
    /// `AETHER_TOOL_OUTPUT_MAX_BYTES` env var (wins over settings), or the
    /// 16 KiB default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0))]
    pub max_bytes: Option<usize>,
    /// Directory the cap writes the full truncated tool result to. The
    /// directory must be writable. Unset inherits from the top-level
    /// `toolOutput` block, the `PRAIRIE_TOOL_OUTPUT_DIR` env var (wins over
    /// settings), or `<project_root>/.prairie/out`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<PathBuf>,
}

impl ToolOutputSettings {
    /// Replace every unset field of `self` with the corresponding field of
    /// `next`. `next` wins on a per-field basis; `None` in `next` is a no-op.
    pub fn merge(&mut self, next: Self) {
        if next.max_bytes.is_some() {
            self.max_bytes = next.max_bytes;
        }
        if next.output_dir.is_some() {
            self.output_dir = next.output_dir;
        }
    }

    /// Resolved byte budget: `Some(n)` returns `n` (including `0` to disable),
    /// `None` falls through to the [`DEFAULT_MAX_BYTES`] constant.
    pub fn resolved_max_bytes(&self) -> usize {
        self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES)
    }

    /// Resolved output directory: `Some(path)` returns `path`; `None` returns
    /// `<project_root>/.prairie/out`.
    pub fn resolved_output_dir(&self, project_root: &Path) -> PathBuf {
        self.output_dir.clone().unwrap_or_else(|| project_root.join(DEFAULT_TOOL_OUTPUT_DIR))
    }
}

/// Result of capping a single tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CappedOutput {
    /// The text the model sees. Either the original `full` string (no cap
    /// applied) or the head+tail preview with the truncation marker.
    pub text: String,
    /// The on-disk file that contains the full output, if the cap fired and
    /// the write succeeded. `None` otherwise.
    pub saved_path: Option<PathBuf>,
}

/// Truncate `full` to at most `cap.max_bytes` bytes.
///
/// The cap is disabled (`full` is returned unchanged and no file is written)
/// when the cap is `0`, smaller than `MIN_USABLE_MAX_BYTES`, or
/// `full.len() <= cap.max_bytes()`. Otherwise the function writes `full` to
/// `<output_dir>/<sha256(full)>.log` first, then builds a head+tail preview
/// with a marker. On any write failure the function falls back to the
/// uncapped text and never loses the original output.
///
/// The on-disk filename is a content hash, so the same `(full, cap)` pair
/// yields byte-identical text on every call — including across runs — which
/// keeps prompt caches warm.
pub fn cap_tool_output(cap: &ToolOutputCap, full: &str) -> CappedOutput {
    if cap.max_bytes == 0 || cap.max_bytes < MIN_USABLE_MAX_BYTES || full.len() <= cap.max_bytes {
        return CappedOutput { text: full.to_string(), saved_path: None };
    }

    let file_name = format!("{}.log", hash_hex(full.as_bytes()));
    let file_path = cap.output_dir.join(&file_name);

    if let Err(error) = write_full_output(cap.output_dir(), full, &file_path) {
        tracing::warn!(%error, path = %file_path.display(), "failed to persist tool output; returning uncapped text");
        return CappedOutput { text: full.to_string(), saved_path: None };
    }

    let total_bytes = full.len();
    let total_lines = count_lines(full);

    // First pass: build a marker with the worst-case (largest) numeric width
    // so the head/tail cut points are conservative. The real marker, built
    // after the cuts are known, is shorter or equal — so the assembled text
    // always fits within `cap.max_bytes`.
    let worst_marker = marker_text(total_bytes, total_lines, total_bytes, total_lines, &file_path);
    let (head_end, tail_start) = head_tail_offsets(full, cap.max_bytes, &worst_marker);
    let head_end = head_end.min(full.len());
    let tail_start = tail_start.min(full.len());

    let head = &full[..head_end];
    let tail = &full[tail_start..];
    let kept_lines = count_lines(head) + count_lines(tail);
    let elided_lines = total_lines.saturating_sub(kept_lines);
    let elided_bytes = total_bytes - head_end - (full.len() - tail_start);
    let marker = marker_text(total_bytes, total_lines, elided_bytes, elided_lines, &file_path);

    let mut text = String::with_capacity(marker.len() + head.len() + tail.len() + 1);
    text.push_str(&marker);
    text.push_str(head);
    text.push('\n');
    text.push_str(tail);

    debug_assert!(text.len() <= cap.max_bytes, "capped text {} exceeds cap {}", text.len(), cap.max_bytes);
    CappedOutput { text, saved_path: Some(file_path) }
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    lower::encode_string(&hasher.finalize())
}
fn write_full_output(dir: &Path, full: &str, file_path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(file_path, full.as_bytes())
}

fn marker_text(
    total_bytes: usize,
    total_lines: usize,
    elided_bytes: usize,
    elided_lines: usize,
    file_path: &Path,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        &mut out,
        "[aether: output truncated; elided {elided_bytes}/{total_bytes} bytes and {elided_lines}/{total_lines} lines. Full: {path}.]",
        path = file_path.display(),
    );
    out
}

/// Compute the head and tail cut points so the assembled `marker + head +
/// tail` text fits within `max_bytes`. Both ends are placed at valid char
/// boundaries so multibyte input never panics. If the marker is too big to
/// leave room for any preview the cap is bypassed (`head_end = 0`,
/// `tail_start = full.len()`) and the caller receives the full text under
/// the marker — the file on disk still holds the complete output.
fn head_tail_offsets(full: &str, max_bytes: usize, marker: &str) -> (usize, usize) {
    let available = max_bytes.saturating_sub(marker.len()).saturating_sub(1);
    if available == 0 {
        return (0, full.len());
    }
    let half = available / 2;
    let head_end = full.floor_char_boundary(half.min(full.len()));
    let tail_budget = available - head_end;
    if tail_budget == 0 {
        return (head_end, full.len());
    }
    let tail_start = full.floor_char_boundary(full.len().saturating_sub(tail_budget));
    (head_end, tail_start)
}

fn count_lines(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(max_bytes: usize, dir: &Path) -> ToolOutputCap {
        ToolOutputCap::new(max_bytes, dir.to_path_buf())
    }

    #[test]
    fn cap_bypassed_when_max_bytes_is_below_minimum() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(MIN_USABLE_MAX_BYTES - 1, dir.path());
        let full = "q".repeat(10_000);
        let out = cap_tool_output(&cap, &full);
        // A cap smaller than the marker is not useful: the function bypasses
        // it and returns the full text, since the cap cannot fit even a
        // marker describing the truncation.
        assert_eq!(out.text, full);
        assert!(out.saved_path.is_none());
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn cap_disabled_when_max_bytes_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(0, dir.path());
        let full = "x".repeat(10_000);
        let out = cap_tool_output(&cap, &full);
        assert_eq!(out.text, full);
        assert!(out.saved_path.is_none());
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn cap_passthrough_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(1024, dir.path());
        let full = "hello world";
        let out = cap_tool_output(&cap, full);
        assert_eq!(out.text, "hello world");
        assert!(out.saved_path.is_none());
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn cap_at_exact_limit_is_passthrough() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(8, dir.path());
        let full = "12345678";
        let out = cap_tool_output(&cap, full);
        assert_eq!(out.text, full);
        assert!(out.saved_path.is_none());
    }

    #[test]
    fn cap_keeps_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(256, dir.path());
        let full = format!("HEAD_SENTINEL\n{}\nTAIL_SENTINEL", "filler\n".repeat(2000));
        let out = cap_tool_output(&cap, &full);
        assert!(out.text.contains("HEAD_SENTINEL"), "missing head: {}", out.text);
        assert!(out.text.contains("TAIL_SENTINEL"), "missing tail: {}", out.text);
        // The marker advertises the elided byte and line counts.
        assert!(out.text.contains("elided "), "missing elided marker: {}", out.text);
        let elided_marker = out
            .text
            .split("elided ")
            .nth(1)
            .and_then(|tail| tail.split('/').next())
            .and_then(|n| n.parse::<usize>().ok())
            .expect("marker should begin with elided <N>/");
        let total_bytes = full.len();
        assert!(
            elided_marker > 0 && elided_marker < total_bytes,
            "elided bytes {elided_marker} should be in (0, {total_bytes})",
        );
        assert!(out.text.len() <= 256, "capped text {} exceeds cap 256", out.text.len());
        // Saved file holds the complete input.
        let saved = out.saved_path.as_ref().expect("cap wrote the full output to disk");
        assert_eq!(std::fs::read_to_string(saved).unwrap(), full);
    }

    #[test]
    fn cap_marker_carries_byte_and_line_counts_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(200, dir.path());
        let full = format!("{}\n{}\n{}", "alpha", "beta".repeat(1000), "omega");
        let out = cap_tool_output(&cap, &full);
        assert!(out.text.starts_with("[aether: output truncated;"));
        let saved = out.saved_path.expect("cap wrote the full output to disk");
        let on_disk = std::fs::read_to_string(&saved).unwrap();
        assert_eq!(on_disk, full);

        for needle in
            ["[aether: output truncated;", "elided ", "/", " bytes and ", "/", " lines. ", "Full: ", "alpha", "omega"]
        {
            assert!(out.text.contains(needle), "missing '{needle}' in: {}", out.text);
        }
        assert!(out.text.contains(saved.file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn cap_saves_full_output_to_disk_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(256, dir.path());
        let full = "x".repeat(10_000);
        let out = cap_tool_output(&cap, &full);
        let saved = out.saved_path.expect("cap wrote the full output to disk");
        let on_disk = std::fs::read(&saved).unwrap();
        assert_eq!(on_disk.len(), full.len());
        assert!(on_disk.iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn cap_does_not_panic_on_multibyte_input() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(256, dir.path());
        let emoji = "🦀";
        let full = format!("HEAD_{}{}TAIL", emoji.repeat(2000), "y".repeat(2000));
        let out = cap_tool_output(&cap, &full);
        assert!(std::str::from_utf8(out.text.as_bytes()).is_ok());
        assert!(out.text.contains("HEAD_"));
        assert!(out.text.contains("TAIL"));
        assert!(out.text.len() <= 256, "capped text {} exceeds cap 256", out.text.len());
    }

    #[test]
    fn cap_is_deterministic_for_same_input() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(200, dir.path());
        let full = "deterministic payload\n".repeat(500);
        let first = cap_tool_output(&cap, &full);
        let second = cap_tool_output(&cap, &full);
        // Filenames are per-session so the on-disk paths differ, but the
        // capped text the model sees is byte-identical (prompt cache stable).
        assert_eq!(first.text, second.text);
        assert_eq!(first.text.len(), second.text.len());
        let first_disk = std::fs::read_to_string(first.saved_path.as_ref().unwrap()).unwrap();
        let second_disk = std::fs::read_to_string(second.saved_path.as_ref().unwrap()).unwrap();
        assert_eq!(first_disk, full);
        assert_eq!(second_disk, full);
    }

    #[test]
    fn cap_marker_path_matches_saved_file() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(256, dir.path());
        let full = "z".repeat(5_000);
        let out = cap_tool_output(&cap, &full);
        let saved = out.saved_path.expect("cap wrote the full output to disk");
        let on_disk = std::fs::read_to_string(&saved).unwrap();
        assert_eq!(on_disk, full);
        let displayed = saved.display().to_string();
        assert!(out.text.contains(&displayed), "marker should name the saved file, got: {}", out.text);
    }

    #[test]
    fn cap_returns_uncapped_text_when_dir_is_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, "x").unwrap();
        // The output dir is an existing regular file, so create_dir_all
        // succeeds (it does not truncate the file) but writing the spillover
        // fails because the path is a file, not a directory.
        let cap = cap(256, &blocker);
        let full = "y".repeat(1_000);
        let out = cap_tool_output(&cap, &full);
        assert_eq!(out.text, full, "fallback to uncapped text when write fails");
        assert!(out.saved_path.is_none());
    }

    #[test]
    fn cap_filename_is_deterministic_per_input() {
        let dir = tempfile::tempdir().unwrap();
        let cap = cap(256, dir.path());
        let first = cap_tool_output(&cap, &"a".repeat(1_000));
        let second = cap_tool_output(&cap, &"b".repeat(1_000));
        // Same input, same file name (content-addressed); different input, different name.
        let again = cap_tool_output(&cap, &"a".repeat(1_000));
        assert_eq!(first.saved_path, again.saved_path, "same input yields same on-disk path");
        assert_ne!(first.saved_path, second.saved_path);
        assert!(
            std::path::Path::new(first.saved_path.as_deref().unwrap().file_name().unwrap())
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        );
    }

    #[test]
    fn cap_default_max_bytes_constant_matches_specification() {
        assert_eq!(DEFAULT_MAX_BYTES, 16 * 1024);
    }

    #[test]
    fn count_lines_handles_empty_and_trailing_newline() {
        // count_lines matches `wc -l`: the number of `\n` characters in the text.
        assert_eq!(count_lines(""), 0);
        assert_eq!(count_lines("a"), 0);
        assert_eq!(count_lines("a\n"), 1);
        assert_eq!(count_lines("a\nb"), 1);
        assert_eq!(count_lines("a\nb\n"), 2);
        assert_eq!(count_lines("\n"), 1);
        assert_eq!(count_lines("\n\n"), 2);
    }
}
