//! Aggregate the file changes a headless run performed.
//!
//! Coding MCP tools attach a [`FileDiff`] to their results via
//! [`ToolResultMeta`]. The headless run streams every tool event past the
//! CLI, so the CLI can tally those diffs here and print a one-line summary
//! at the end of the transcript.

use std::collections::BTreeMap;

use mcp_utils::display_meta::ToolResultMeta;

/// The kind of change a tool result applied to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

/// Tally of every file changed by a single run, keyed by absolute path.
#[derive(Debug, Default, Clone)]
pub(crate) struct FileChanges {
    files: BTreeMap<String, ChangeKind>,
}

impl FileChanges {
    /// Record a tool-result metadata block, if it carries a `FileDiff`.
    ///
    /// Classification is driven by which of `old_text` / `new_text` are set:
    ///
    /// - `old_text = None`, `new_text = Some(_)` → `Created`
    /// - `old_text = Some(_)`, `new_text = Some(_)` → `Modified`
    /// - `new_text = None` → `Deleted` (regardless of `old_text`)
    ///
    /// Subsequent updates for the same path follow a "latest wins, except
    /// `Created` is sticky" rule so a create-then-edit is still reported
    /// as created.
    pub(crate) fn record(&mut self, meta: &ToolResultMeta) {
        let Some(diff) = meta.file_diff.as_ref() else {
            return;
        };
        let kind = match (diff.old_text.as_ref(), diff.new_text.as_ref()) {
            (_, None) => ChangeKind::Deleted,
            (None, Some(_)) => ChangeKind::Created,
            (Some(_), Some(_)) => ChangeKind::Modified,
        };
        self.files
            .entry(diff.path.clone())
            .and_modify(|existing| {
                if *existing == ChangeKind::Created {
                    return;
                }
                *existing = kind;
            })
            .or_insert(kind);
    }

    pub(crate) fn total(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn created(&self) -> usize {
        self.files.values().filter(|kind| **kind == ChangeKind::Created).count()
    }

    pub(crate) fn modified(&self) -> usize {
        self.files.values().filter(|kind| **kind == ChangeKind::Modified).count()
    }

    pub(crate) fn deleted(&self) -> usize {
        self.files.values().filter(|kind| **kind == ChangeKind::Deleted).count()
    }

    /// One-line summary suitable for printing at the end of a transcript.
    pub(crate) fn summary(&self) -> String {
        format!(
            "Files changed: {} ({} created, {} modified, {} deleted)",
            self.total(),
            self.created(),
            self.modified(),
            self.deleted(),
        )
    }
}

#[cfg(test)]
mod tests {
    use mcp_utils::display_meta::{FileDiff, ToolDisplayMeta, ToolResultMeta};

    use super::{ChangeKind, FileChanges};

    fn display(title: &str, value: &str) -> ToolDisplayMeta {
        ToolDisplayMeta::new(title, value)
    }

    fn diff(path: &str, old: Option<&str>, new: Option<&str>) -> FileDiff {
        FileDiff { path: path.to_string(), old_text: old.map(str::to_string), new_text: new.map(str::to_string) }
    }

    fn meta(diff: FileDiff) -> ToolResultMeta {
        ToolResultMeta::with_file_diff(display("Edit", &diff.path), diff)
    }

    #[test]
    fn empty_tally_reports_zero() {
        let changes = FileChanges::default();
        assert_eq!(changes.total(), 0);
        assert_eq!(changes.created(), 0);
        assert_eq!(changes.modified(), 0);
        assert_eq!(changes.deleted(), 0);
        assert_eq!(changes.summary(), "Files changed: 0 (0 created, 0 modified, 0 deleted)");
    }

    #[test]
    fn classifies_created_modified_and_deleted() {
        let mut changes = FileChanges::default();
        changes.record(&meta(diff("a/new.rs", None, Some("hello"))));
        changes.record(&meta(diff("a/existing.rs", Some("old"), Some("new"))));
        changes.record(&meta(diff("a/gone.rs", Some("old"), None)));

        assert_eq!(changes.total(), 3);
        assert_eq!(changes.created(), 1);
        assert_eq!(changes.modified(), 1);
        assert_eq!(changes.deleted(), 1);
        assert_eq!(changes.summary(), "Files changed: 3 (1 created, 1 modified, 1 deleted)");
    }

    #[test]
    fn dedupes_repeated_paths() {
        let mut changes = FileChanges::default();
        let path = "a/duplicated.rs";
        changes.record(&meta(diff(path, Some("old"), Some("new1"))));
        changes.record(&meta(diff(path, Some("new1"), Some("new2"))));
        changes.record(&meta(diff(path, Some("new2"), Some("new3"))));

        assert_eq!(changes.total(), 1);
        assert_eq!(changes.modified(), 1);
        assert_eq!(changes.created(), 0);
        assert_eq!(changes.deleted(), 0);
    }

    #[test]
    fn created_is_sticky_under_subsequent_edits() {
        let mut changes = FileChanges::default();
        let path = "a/file.rs";
        changes.record(&meta(diff(path, None, Some("v1"))));
        changes.record(&meta(diff(path, Some("v1"), Some("v2"))));

        assert_eq!(changes.total(), 1);
        assert_eq!(changes.created(), 1);
        assert_eq!(changes.modified(), 0);
        assert_eq!(changes.deleted(), 0);
    }

    #[test]
    fn edit_then_delete_becomes_deleted() {
        let mut changes = FileChanges::default();
        let path = "a/file.rs";
        changes.record(&meta(diff(path, Some("v1"), Some("v2"))));
        changes.record(&meta(diff(path, Some("v2"), None)));

        assert_eq!(changes.total(), 1);
        assert_eq!(changes.created(), 0);
        assert_eq!(changes.modified(), 0);
        assert_eq!(changes.deleted(), 1);
    }

    #[test]
    fn metadata_without_file_diff_is_ignored() {
        let mut changes = FileChanges::default();
        changes.record(&ToolResultMeta::new(display("Read file", "a.rs")));

        assert_eq!(changes.total(), 0);
        assert_eq!(changes.summary(), "Files changed: 0 (0 created, 0 modified, 0 deleted)");
    }

    #[test]
    fn change_kind_distinguishes_categories() {
        assert_ne!(ChangeKind::Created, ChangeKind::Modified);
        assert_ne!(ChangeKind::Modified, ChangeKind::Deleted);
        assert_ne!(ChangeKind::Created, ChangeKind::Deleted);
    }
}
