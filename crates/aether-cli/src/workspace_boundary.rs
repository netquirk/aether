//! Path-confining [`ToolPolicy`] rooted at the working directory (TASK-26-394).
//!
//! A run rooted at `DIR` may legitimately touch anything inside `DIR`; a
//! file-tool call that resolves outside `DIR` (for example `../secret` or
//! `/etc/passwd`) is refused before execution. The refusal is routed through
//! the existing [`ToolPolicy`] consult point in
//! `aether-core/src/core/agent.rs`, which records a
//! [`ToolEvent::Refused`](aether_core::events::ToolEvent::Refused) carrying
//! the offending request and a human-readable reason. The CLI renders that
//! reason as `"Tool refused [name]: {reason}"`.
//!
//! Scope is intentionally narrow: only the file-system-touching coding tools
//! (`read_file`, `write_file`, `edit_file`, `list_files`, `find`, `grep`)
//! are subject to the boundary. Tools like `bash` are untouched because
//! they have their own command-level checks (and their behaviour belongs to
//! the shell sandbox, not to this policy).
//!
//! The policy **fails open**: a tool call whose `arguments` are not valid
//! JSON or that omits every recognised path key is allowed through, never
//! over-refused. Over-refusing would silently break unrelated tools; failing
//! open keeps the surface of "what this policy catches" equal to the
//! surface of "what this policy can statically prove".
//!
//! The check is also **lexical by default** — `.` and `..` segments are
//! resolved without touching the filesystem, so a non-existent
//! `root/../secret` is still caught. When the path (or any of its
//! ancestors) exists, the existing portion is canonicalized to catch
//! symlink escapes, mirroring the behaviour the rest of the agent expects
//! from `Path::canonicalize`.

use std::path::{Component, Path, PathBuf};

use aether_core::core::ToolPolicy;
use llm::ToolCallRequest;

/// File tools that take a path argument and are subject to the boundary
/// check. Other tools (`bash`, network tools, etc.) are out of scope; the
/// policy lets them through unchanged.
const FILE_TOOLS: &[&str] = &["read_file", "write_file", "edit_file", "list_files", "find", "grep"];

/// Argument keys that carry the path. The first key present wins, in this
/// order. The order matches how the existing `read_path_from_arguments`
/// helper in `headless/run.rs` reads paths, plus `path` for tools that
/// already use that name (notably `find` / `grep`).
const PATH_KEYS: &[&str] = &["filePath", "file_path", "path"];

/// A [`ToolPolicy`] that refuses file-tool calls whose resolved path
/// escapes the run's root directory.
///
/// The root is canonicalized once at construction time so the `starts_with`
/// comparison in [`ToolPolicy::refuse`] is against the same physical path
/// `canonicalize_existing_ancestor` produces later; that pair is what
/// defeats the macOS `/tmp` -> `/private/tmp` symlink on either side of
/// the boundary.
pub(crate) struct WorkspaceBoundaryPolicy {
    root: PathBuf,
}

impl WorkspaceBoundaryPolicy {
    /// Build a policy rooted at `root`. The path is canonicalized once so
    /// later comparisons are stable across symlinks; if canonicalize fails
    /// (the path does not exist yet, or the OS rejected the lookup), the
    /// call falls back to the supplied path verbatim. Tests pass a
    /// pre-canonicalized tempdir so the fallback branch is not exercised.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        let given = root.into();
        let root = given.canonicalize().unwrap_or(given);
        Self { root }
    }

    /// The canonical root path. Exposed for tests and integration call
    /// sites that need to reason about what the policy will accept.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

impl ToolPolicy for WorkspaceBoundaryPolicy {
    fn refuse(&self, request: &ToolCallRequest) -> Option<String> {
        let tool = base_tool_name(&request.name);
        if !FILE_TOOLS.contains(&tool) {
            return None;
        }

        // Fail open: a malformed `arguments` blob is the model's problem,
        // not the boundary's. The downstream tool will surface its own
        // parse error; over-refusing here would mask real failures.
        let arguments: serde_json::Value = serde_json::from_str(&request.arguments).ok()?;
        let raw = path_argument(&arguments)?;

        let resolved = resolve_path(&self.root, raw);
        if resolved.starts_with(&self.root) {
            None
        } else {
            Some(format!(
                "refusing `{tool}`: path `{raw}` resolves outside the working directory `{}`",
                self.root.display()
            ))
        }
    }
}

/// Strip an MCP namespace from a tool name. MCP tools arrive as
/// `<server>__<tool>` (for example `coding__read_file`); the boundary only
/// cares about the base tool. Bare tool names without a namespace (e.g.
/// `read_file`) are returned unchanged.
fn base_tool_name(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

/// Pull the first present string path out of a parsed JSON argument
/// object. Returns `None` if every recognised key is missing or non-string
/// (e.g. the model passed a JSON object instead of a plain path).
fn path_argument(arguments: &serde_json::Value) -> Option<&str> {
    PATH_KEYS.iter().find_map(|k| arguments.get(*k).and_then(serde_json::Value::as_str))
}

/// Lexically resolve `.` and `..` without touching the filesystem.
///
/// A leading `..` that has nothing to pop is preserved verbatim so the
/// caller can still see that the path tried to escape: silently collapsing
/// it into the root would let `root/../secret` look like a legitimate
/// relative path under root.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push(component.as_os_str());
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// Canonicalize the longest existing prefix of `path`, leaving any
/// non-existing suffix verbatim.
///
/// Used to catch symlink escapes without requiring the exact target to
/// exist yet — a request for `root/new-dir/file` (where `root/new-dir/`
/// does not exist) is still resolved through the canonical `root` so a
/// symlinked root is treated consistently. If nothing on the path
/// canonicalizes at all, the lexical form is returned unchanged.
fn canonicalize_existing_ancestor(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }

    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            tail.push(name.to_os_string());
        }
        if let Ok(p) = parent.canonicalize() {
            let mut result = p;
            for part in tail.iter().rev() {
                result.push(part);
            }
            return result;
        }
        current = parent;
    }

    path.to_path_buf()
}

/// Resolve a raw tool-argument path against the run root, applying
/// lexical normalization and (when the path exists) canonicalization of
/// the nearest existing ancestor.
fn resolve_path(root: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    let joined = if p.is_absolute() { p.to_path_buf() } else { root.join(p) };
    canonicalize_existing_ancestor(&normalize_lexically(&joined))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`ToolCallRequest`] with the name and arguments the model
    /// would produce. Only `name` and `arguments` matter for the policy;
    /// `id` is set to a fixed value so a regression in either field is
    /// visible.
    fn request(name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest { id: "test".into(), name: name.into(), arguments: arguments.into() }
    }

    /// Construct a policy rooted at a fresh tempdir, canonicalizing the
    /// root first so macOS `/tmp` -> `/private/tmp` symlinks line up with
    /// what `canonicalize_existing_ancestor` will produce during the test.
    /// Returns the policy and the canonical root so tests can include the
    /// rendered root in their reason-content assertions.
    fn policy_with_temp_root() -> (tempfile::TempDir, PathBuf, WorkspaceBoundaryPolicy) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonicalize tempdir");
        let policy = WorkspaceBoundaryPolicy::new(&root);
        (dir, root, policy)
    }

    /// A `..` escape attempt from a namespaced read_file call must be
    /// refused and the reason must name both the offending raw path and
    /// the root the policy was built against. This is the headline
    /// acceptance criterion from the task body: "a file tool call whose
    /// resolved path escapes the working directory is refused rather than
    /// executed".
    #[test]
    fn refuses_dotdot_escape_naming_the_path() {
        let (_dir, root, policy) = policy_with_temp_root();
        let reason = policy
            .refuse(&request("coding__read_file", r#"{"filePath":"../secret"}"#))
            .expect("a ../ escape must be refused");
        assert!(reason.contains("../secret"), "reason must name the offending raw path `../secret`; got: {reason}",);
        assert!(
            reason.contains(&*root.to_string_lossy()),
            "reason must name the root `{}`; got: {reason}",
            root.display(),
        );
    }

    /// The MCP namespace must be stripped before the file-tool check
    /// runs: any `<server>__read_file` name with an escaping relative
    /// argument must refuse the same way `coding__read_file` does.
    #[test]
    fn refuses_namespaced_read_file_escape() {
        let (_dir, _root, policy) = policy_with_temp_root();
        assert!(
            policy.refuse(&request("other__read_file", r#"{"filePath":"../escape"}"#)).is_some(),
            "a namespaced read_file with an escaping relative path must be refused",
        );
    }

    /// An absolute path that lives outside the canonical root must be
    /// refused, and the reason must name the raw absolute path the model
    /// asked for so the operator can see what the run tried to reach.
    #[test]
    fn refuses_absolute_path_outside_root_naming_the_path() {
        let (dir, root, policy) = policy_with_temp_root();
        // A path that lives next to the temp dir but is not inside it.
        // Using the tempdir's parent makes the test portable: on Linux
        // the parent is `/tmp` (or the workspace tmpdir), on macOS the
        // `/private/tmp` shim is reached through `canonicalize` after the
        // join. The picked name is guaranteed not to exist.
        let outside = dir.path().parent().expect("tempdir parent").join("definitely-not-in-root-aether-task-26-394");
        let raw = outside.to_string_lossy().into_owned();
        let args = serde_json::json!({ "filePath": &raw }).to_string();
        let reason = policy
            .refuse(&request("coding__read_file", &args))
            .expect("an absolute path outside the root must be refused");
        assert!(reason.contains(&raw), "reason must name the offending raw absolute path `{raw}`; got: {reason}",);
        assert!(
            reason.contains(&*root.to_string_lossy()),
            "reason must name the root `{}`; got: {reason}",
            root.display(),
        );
    }

    /// A relative path that joins cleanly into it must be allowed.
    /// The path doesn't have to exist — the boundary is a lexical /
    /// canonicalized-ancestor check, not an existence check.
    #[test]
    fn allows_relative_path_inside_root() {
        let (_dir, _root, policy) = policy_with_temp_root();
        assert!(
            policy.refuse(&request("coding__read_file", r#"{"filePath":"src/main.rs"}"#)).is_none(),
            "a relative path that joins into the root must be allowed",
        );
    }

    /// An absolute path that is inside the canonical root must be
    /// allowed. Built using the canonical root so the comparison is
    /// `starts_with`-stable across the `/private/tmp` shim.
    #[test]
    fn allows_absolute_path_inside_root() {
        let (_dir, root, policy) = policy_with_temp_root();
        let inner = root.join("notes.txt");
        let args = serde_json::json!({ "path": inner.to_string_lossy() }).to_string();
        assert!(
            policy.refuse(&request("coding__read_file", &args)).is_none(),
            "an absolute path under the canonical root must be allowed; args={args}",
        );
    }

    /// Non-file tools (anything not in `FILE_TOOLS`) are out of scope
    /// and must pass through unchanged. `bash` is the most common
    /// example; it is governed by the shell sandbox, not by this
    /// boundary.
    #[test]
    fn allows_non_file_tool() {
        let (_dir, _root, policy) = policy_with_temp_root();
        assert!(
            policy.refuse(&request("coding__bash", r#"{"command":"echo hi"}"#)).is_none(),
            "non-file tools must be passed through unchanged",
        );
    }

    /// When a request has no path argument at all — empty object,
    /// non-JSON, or a path field that isn't a string — the policy must
    /// fail open and let the call through. Over-refusing would silently
    /// break every tool that doesn't happen to use one of the recognised
    /// path keys.
    #[test]
    fn allows_missing_path_argument() {
        let (_dir, _root, policy) = policy_with_temp_root();
        assert!(
            policy.refuse(&request("coding__read_file", "{}")).is_none(),
            "an empty JSON object must be allowed through (no path to check)",
        );
        assert!(
            policy.refuse(&request("coding__read_file", "not json")).is_none(),
            "non-JSON arguments must be allowed through (fail open)",
        );
        assert!(
            policy.refuse(&request("coding__read_file", r#"{"filePath":123}"#)).is_none(),
            "a non-string path field must be allowed through (fail open)",
        );
    }
}
