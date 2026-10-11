//! Size-based log rotation for the `--log-file` writer (TASK-26-92).
//!
//! The run's `tracing` log is normally appended to a single file
//! (TASK-25-48) so an existing log survives across runs. When a long-running
//! aether process generates enough records that one unbounded file would
//! fill the disk, the operator can pass `--log-max-bytes N` and the writer
//! here rotates the file on the operator-named path:
//!
//! - The current file at `PATH` is renamed to `<stem>.1` (overwriting any
//!   previous rotated sibling — exactly one previous file is kept, mirroring
//!   the `--transcript-max-bytes` contract from TASK-24-279).
//! - A fresh empty file is opened at the original `PATH` so subsequent
//!   writes resume where the rotation left off. The append-across-runs
//!   contract from TASK-25-48 still holds: the operator's pre-existing log
//!   at `PATH` survives any number of runs.
//! - One line is printed naming the new file. The line goes to **stderr**
//!   (never into the rotated file) so `--log-format json` keeps one valid
//!   JSON object per line and downstream log readers do not have to skip a
//!   non-record line at every threshold crossing.
//!
//! `tracing-appender` has no built-in size cap, so this is a local
//! `MakeWriter` that owns a `Mutex<RotatingFile>` plus the notice sink and
//! serialises every write. The writer never calls `tracing` itself
//! (re-entrancy would feed rotations back into the same `fmt` layer and
//! corrupt the file), and only emits the rotation notice via the injected
//! `notice` sink.
//!
//! Only applies when both `--log-file` and `--log-max-bytes N` with
//! `N > 0` are set; the `None`/`Some(0)` path in `setup_tracing` keeps the
//! pre-existing single-file append behaviour byte-for-byte.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::writer::MakeWriter;

/// Default stderr line printed when a rotation happens. Names the new file
/// (the freshly opened one at the original `--log-file` path) so an
/// operator tailing stderr can follow the file across rotations.
pub(crate) const ROTATION_NOTICE: &str = "aether: log rotated; writing to ";

/// Sink the rotation notice goes through. The default prints to stderr via
/// [`default_notice`]; tests inject their own closure to capture the line
/// deterministically without touching the process's stderr.
type NoticeSink = Arc<dyn Fn(&Path) + Send + Sync>;

fn default_notice(path: &Path) {
    eprintln!("{ROTATION_NOTICE}{}", path.display());
}

/// Shared state owned by both the `RotatingFileAppender` (for construction
/// and rotation) and every `RotatingFileWriter` (for the actual writes).
/// Held behind `Arc<Mutex<…>>` so the per-write `RotatingFileWriter`
/// instances are cheap to produce and writes are serialised.
struct Inner {
    state: Mutex<RotatingFile>,
    notice: NoticeSink,
}

struct RotatingFile {
    file: File,
    path: PathBuf,
    bytes: u64,
    max_bytes: u64,
}

/// `MakeWriter` implementation that owns a size-capped, self-rotating file.
/// Cheap to clone (the inner state is shared by `Arc`) and `Send + Sync` so
/// it can be wrapped in a [`tracing_subscriber::fmt::writer::BoxMakeWriter`]
/// exactly like the plain `File` writer `setup_tracing` used before.
pub(crate) struct RotatingFileAppender {
    inner: Arc<Inner>,
}

impl RotatingFileAppender {
    /// Open `path` for append (the same `OpenOptions::new().create(true).append(true)`
    /// mode `setup_tracing` has always used) and configure size-based
    /// rotation at `max_bytes` bytes. `max_bytes` of `0` is reserved for the
    /// "no rotation" sentinel — the caller in `setup_tracing` checks `> 0`
    /// before reaching this constructor, so passing `0` here would disable
    /// rotation but still seed `bytes` from the on-disk length.
    pub(crate) fn new(path: &Path, max_bytes: u64) -> io::Result<Self> {
        Self::new_with_notice(path, max_bytes, Arc::new(default_notice))
    }

    /// Test-only constructor that lets the caller substitute the
    /// `default_notice` sink so a unit test can capture the rotation line
    /// without intercepting the process's stderr.
    #[cfg(test)]
    fn new_with_notice_for_test(path: &Path, max_bytes: u64, notice: NoticeSink) -> io::Result<Self> {
        Self::new_with_notice(path, max_bytes, notice)
    }

    fn new_with_notice(path: &Path, max_bytes: u64, notice: NoticeSink) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        // Seed the byte counter from the on-disk length so an already-large
        // file (e.g. a fresh `aether` run that inherits a long-lived log)
        // rotates on the next write rather than waiting for `max_bytes`
        // additional bytes.
        let bytes = file.metadata()?.len();
        let inner = Arc::new(Inner {
            state: Mutex::new(RotatingFile { file, path: path.to_path_buf(), bytes, max_bytes }),
            notice,
        });
        Ok(Self { inner })
    }

    /// Path the current log is renamed to on rotation. Drops the extension
    /// so `aether.log` rotates to `aether.1`, mirroring the transcript's
    /// `<stem>.1` naming from
    /// [`crate::transcript::JsonlTranscript::rotated_path`].
    fn rotated_path(path: &Path) -> PathBuf {
        path.with_extension("1")
    }

    /// Trigger a rotation under the held lock. Caller has already locked
    /// `state`. Flushes, renames the live file to `<stem>.1` (after
    /// removing any previous sibling), reopens a fresh empty file at the
    /// original path, resets the byte counter, and fires the notice naming
    /// the newly opened file.
    fn rotate_locked(inner: &Inner, state: &mut RotatingFile) -> io::Result<()> {
        state.file.flush()?;
        let rotated = Self::rotated_path(&state.path);
        if rotated.exists() {
            std::fs::remove_file(&rotated)?;
        }
        std::fs::rename(&state.path, &rotated)?;
        state.file = OpenOptions::new().create(true).append(true).open(&state.path)?;
        state.bytes = 0;
        (inner.notice)(&state.path);
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RotatingFileAppender {
    type Writer = RotatingFileWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingFileWriter { inner: Arc::clone(&self.inner) }
    }
}

/// Per-write handle. Holds the shared state by `Arc<Mutex<…>>` so each
/// `fmt` event gets its own cheap writer instance while writes still happen
/// one at a time. `Drop` does nothing because the underlying `File` is
/// reused across events; closing it would tear down the appender.
pub(crate) struct RotatingFileWriter {
    inner: Arc<Inner>,
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.inner.state.lock().expect("rotating log mutex poisoned");
        let written = state.file.write(buf)?;
        state.bytes = state.bytes.saturating_add(written as u64);
        // Rotation fires after the write that crosses the threshold so a
        // multi-record event never splits across files. Matches the
        // transcript's "rotate after the line that crosses" contract.
        // Hold the lock through rotation so a concurrent writer cannot
        // append into the new file before the rename finishes. `?`
        // propagates a disk-full / permission error to the
        // `tracing-subscriber` layer, which surfaces it on the writer.
        if state.max_bytes > 0 && state.bytes >= state.max_bytes {
            RotatingFileAppender::rotate_locked(&self.inner, &mut state)?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.inner.state.lock().expect("rotating log mutex poisoned");
        state.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn open_with_captured_notice(path: &Path, max_bytes: u64) -> (RotatingFileAppender, Arc<StdMutex<Vec<PathBuf>>>) {
        let captured: Arc<StdMutex<Vec<PathBuf>>> = Arc::new(StdMutex::new(Vec::new()));
        let captured_for_closure = Arc::clone(&captured);
        let notice: NoticeSink = Arc::new(move |p: &Path| {
            captured_for_closure.lock().expect("capture lock").push(p.to_path_buf());
        });
        let appender = RotatingFileAppender::new_with_notice_for_test(path, max_bytes, notice).expect("open appender");
        (appender, captured)
    }

    #[test]
    fn rotated_path_drops_extension() {
        assert_eq!(RotatingFileAppender::rotated_path(Path::new("aether.log")), PathBuf::from("aether.1"));
        assert_eq!(RotatingFileAppender::rotated_path(Path::new("/tmp/run.log")), PathBuf::from("/tmp/run.1"));
        assert_eq!(RotatingFileAppender::rotated_path(Path::new("plain")), PathBuf::from("plain.1"));
    }

    #[test]
    fn single_write_below_threshold_does_not_rotate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        let (appender, captured) = open_with_captured_notice(&path, 1024);
        let mut w = appender.make_writer();
        w.write_all(b"hello").expect("write");
        w.flush().expect("flush");

        assert!(captured.lock().unwrap().is_empty(), "no notice must fire when below threshold");
        assert!(
            !RotatingFileAppender::rotated_path(&path).exists(),
            "no rotated sibling must exist when below threshold"
        );

        let live = std::fs::read_to_string(&path).expect("read live");
        assert_eq!(live, "hello", "live file must hold the writes when below threshold");
    }

    #[test]
    fn write_past_threshold_rotates_and_prints_notice_naming_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        // `max_bytes = 1` forces a rotation after the very first write:
        // every chunk crosses the threshold, so the rotated sibling holds
        // exactly the bytes from the trigger write and the live file is
        // freshly empty.
        let (appender, captured) = open_with_captured_notice(&path, 1);
        let mut w = appender.make_writer();
        w.write_all(b"first").expect("write 1");
        w.flush().expect("flush 1");

        let rotated = RotatingFileAppender::rotated_path(&path);
        assert!(rotated.exists(), "rotated sibling must exist after threshold crossing");
        let rotated_contents = std::fs::read_to_string(&rotated).expect("read rotated");
        assert_eq!(rotated_contents, "first", "rotated sibling must hold the pre-rotation bytes verbatim");

        let live_after_first = std::fs::read_to_string(&path).expect("read live");
        assert!(live_after_first.is_empty(), "live file must be empty between rotations; got {live_after_first:?}");

        let notices = captured.lock().unwrap().clone();
        assert_eq!(notices.len(), 1, "exactly one notice per rotation");
        assert_eq!(notices[0], path, "the notice must name the *new* file (the freshly-opened one)");

        // A second write triggers a second rotation: the rotated sibling
        // is overwritten with the new contents and the live file is empty
        // again.
        w.write_all(b"second").expect("write 2");
        w.flush().expect("flush 2");
        let rotated_now = std::fs::read_to_string(&rotated).expect("read rotated again");
        assert_eq!(rotated_now, "second", "rotated sibling must be overwritten on each rotation");
        let live_again = std::fs::read_to_string(&path).expect("read live again");
        assert!(live_again.is_empty(), "live file must stay empty across rotations; got {live_again:?}");

        let notices_after_second = captured.lock().unwrap().clone();
        assert_eq!(notices_after_second.len(), 2, "second rotation must emit a second notice");
        assert_eq!(notices_after_second[1], path);
    }

    #[test]
    fn zero_max_bytes_disables_rotation_even_after_large_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        // `Some(0)` is reserved for "no rotation" — the call site in
        // `setup_tracing` must branch on `> 0` so this constructor never
        // sees zero, but documenting the behaviour in a unit test makes
        // the contract explicit.
        let (appender, captured) = open_with_captured_notice(&path, 0);
        let mut w = appender.make_writer();
        w.write_all(b"first chunk").expect("write 1");
        w.write_all(b"second chunk").expect("write 2");
        w.flush().expect("flush");

        let rotated = RotatingFileAppender::rotated_path(&path);
        assert!(!rotated.exists(), "Some(0) must not rotate; got unexpected rotated sibling at {rotated:?}");

        let live = std::fs::read_to_string(&path).expect("read live");
        assert_eq!(live, "first chunksecond chunk", "live file must hold all writes when rotation is disabled");

        assert!(captured.lock().unwrap().is_empty(), "no notice must fire when rotation is disabled");
    }

    #[test]
    fn seed_bytes_from_existing_file_triggers_rotation_on_next_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("run.log");
        // Pre-seed the live file so the constructor sees a non-zero length.
        std::fs::write(&path, b"already-large").expect("seed file");

        let (appender, captured) = open_with_captured_notice(&path, 4);
        let mut w = appender.make_writer();
        w.write_all(b"!").expect("write trigger");
        w.flush().expect("flush");

        let rotated = RotatingFileAppender::rotated_path(&path);
        assert!(rotated.exists(), "an already-large file must rotate immediately on the next write");
        let rotated_contents = std::fs::read_to_string(&rotated).expect("read rotated");
        assert!(
            rotated_contents.contains("already-large!"),
            "rotated sibling must hold the pre-existing + just-written bytes; got {rotated_contents:?}"
        );

        let notices = captured.lock().unwrap().clone();
        assert_eq!(notices.len(), 1, "rotation must emit exactly one notice");
        assert_eq!(notices[0], path);
    }
}
