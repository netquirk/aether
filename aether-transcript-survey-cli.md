# TASK-2B-343 (Part 1 of 4) / TASK-2B-270 (Part 1 of 3) — CLI-side `Transcript` artifact inventory in `crates/aether-cli/src/transcript.rs`

Read-only evidence pass. No source file is edited. Only this one file is
added by the task. The git tree before this change is the base commit
`96a0ba71c9de3ca69917738334e6976b6e3750bb`
(`TASK-2B-375: Record the README/migrations negatives and state the…`),
`git status --porcelain` is clean, and `git ls-files` confirms every line
cited below is unchanged at the new commit.

The task's three asks are all answered with a single artifact table (the
"ONE verbatim defining line" column) plus a thin redactor-wiring table and
a thin headless `run.rs` call-site table. Each entry gives the exact
1-based line number, so a reviewer can `sed -n 'N{p;Nq}'` the file at the
base commit and reproduce the quote. The numbering matches the line
column the previous survey parts used (TASK-2B-371..375).

---

## 1. Artifacts in `crates/aether-cli/src/transcript.rs`

For every artifact the task body names, the table records its exact file
path, 1-based line number, and the ONE verbatim line that defines it. The
"defining line" is the struct/enum/fn declaration line (signature line
plus opening `{`); the doc comment immediately above it states the
surrounding contract and is omitted from the table on purpose (the task
explicitly asked for the *one* defining line, not a block).

| # | Artifact | Path | Line | One verbatim defining line |
|---|----------|------|------|----------------------------|
| 1 | Module doc lead (the line that names the artifact) | `crates/aether-cli/src/transcript.rs` | 1 | `//! JSON Lines transcript writer for the \`aether headless\` event stream.` |
| 2 | Header-shape schema (doc-quoted JSON; the on-disk contract) | `crates/aether-cli/src/transcript.rs` | 23 | `//! \`{"type":"header","aetherVersion":<version>,"startedAt":<rfc3339 utc>}\`` |
| 3 | `TranscriptRecord` (one JSON object per emitted event) | `crates/aether-cli/src/transcript.rs` | 59 | `struct TranscriptRecord<'a> {` |
| 4 | `JsonlTranscript` (the streaming writer) | `crates/aether-cli/src/transcript.rs` | 96 | `pub struct JsonlTranscript {` |
| 5 | `JsonlTranscript::create_with_max_bytes` (rotation-aware open) | `crates/aether-cli/src/transcript.rs` | 137 | `    pub fn create_with_max_bytes(path: &Path, max_bytes: Option<u64>) -> io::Result<Self> {` |
| 6 | `JsonlTranscript::with_redactor` (redactor-wiring builder) | `crates/aether-cli/src/transcript.rs` | 160 | `    pub fn with_redactor(mut self, redactor: SecretRedactor) -> Self {` |
| 7 | `JsonlTranscript::write_header` (first-line header) | `crates/aether-cli/src/transcript.rs` | 179 | `    pub fn write_header(&mut self, aether_version: &str, started_at: &str) -> io::Result<()> {` |
| 8 | `JsonlTranscript::record` (per-event write) | `crates/aether-cli/src/transcript.rs` | 203 | `    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {` |
| 9 | `JsonlTranscript::rotated_path` (`<stem>.1` rename target) | `crates/aether-cli/src/transcript.rs` | 240 | `    fn rotated_path(&self) -> PathBuf {` |
| 10 | `JsonlTranscript::rotate` (rename + fresh file) | `crates/aether-cli/src/transcript.rs` | 255 | `    fn rotate(&mut self) -> io::Result<()> {` |
| 11 | `TranscriptError` (resume-load error enum) | `crates/aether-cli/src/transcript.rs` | 284 | `pub enum TranscriptError {` |
| 12 | `load_resume_messages` (`--resume PATH` reader) | `crates/aether-cli/src/transcript.rs` | 316 | `pub fn load_resume_messages(path: &Path) -> Result<Vec<ChatMessage>, TranscriptError> {` |
| 13 | `ResumeBuilder` (folds event lines into `ChatMessage`s) | `crates/aether-cli/src/transcript.rs` | 371 | `struct ResumeBuilder {` |

Reproduce the columns (line numbers confirmed against the source at base
commit `96a0ba71`):

```
$ git ls-files | grep 'crates/aether-cli/src/transcript.rs$'
crates/aether-cli/src/transcript.rs
$ sed -n '1p;23p;59p;96p;137p;160p;179p;203p;240p;255p;284p;316p;371p' \
    crates/aether-cli/src/transcript.rs
```

(The sed output reproduces the "One verbatim defining line" column
above.)

---

## 2. Redactor wiring (the TASK-26-22 credential-redaction claim)

The brief's TASK-26-22 quality goal — "credential-looking values are
redacted before they are written to the log or transcript" — is enforced
inside the JSONL writer by a `SecretRedactor` field that the constructor
defaults to `SecretRedactor::new()` and that callers override via the
`with_redactor` builder. Both `write_header` and `record` run the
redactor on the serialised line before it is written. The exact lines:

| # | What it does | Path | Line | One verbatim line |
|---|--------------|------|------|-------------------|
| R1 | `redactor` field on `JsonlTranscript` | `crates/aether-cli/src/transcript.rs` | 114 | `    redactor: SecretRedactor,` |
| R2 | `create_with_max_bytes` installs the default redactor | `crates/aether-cli/src/transcript.rs` | 146 | `            redactor: SecretRedactor::new(),` |
| R3 | `with_redactor` builder lets callers override the default | `crates/aether-cli/src/transcript.rs` | 160 | `    pub fn with_redactor(mut self, redactor: SecretRedactor) -> Self {` |
| R4 | `write_header` runs the redactor on the header bytes | `crates/aether-cli/src/transcript.rs` | 187 | `        let bytes = self.redactor.redact_bytes(&bytes);` |
| R5 | `record` runs the redactor on each event line | `crates/aether-cli/src/transcript.rs` | 218 | `        let bytes = self.redactor.redact_bytes(&bytes);` |

Reproduce (line numbers confirmed against the source at base commit
`96a0ba71`):

```
$ sed -n '114p;146p;160p;187p;218p' crates/aether-cli/src/transcript.rs
```

(The sed output reproduces the "One verbatim line" column above.)

The matching headless-side wire-up — i.e. the call from
`aether headless --transcript-jsonl` that installs the same redactor
the run log uses — is in §3, row H3.

---

## 3. `--resume PATH` and `--transcript-jsonl` call sites in `crates/aether-cli/src/headless/run.rs`

The task explicitly asks for confirmation that the headless command line
plumbs the writer and the reader. Both live in `run_agent`
(`crates/aether-cli/src/headless/run.rs`):

| # | What the line does | Path | Line | One verbatim line |
|---|--------------------|------|------|-------------------|
| H1 | `--resume PATH` calls `load_resume_messages` (TASK-26-20) | `crates/aether-cli/src/headless/run.rs` | 181 | `        Some(path) => crate::transcript::load_resume_messages(path)` |
| H2 | `--transcript-jsonl` opens the writer with `create_with_max_bytes` | `crates/aether-cli/src/headless/run.rs` | 234 | `            let mut writer = JsonlTranscript::create_with_max_bytes(path, config.transcript_max_bytes)` |
| H3 | Same call chain installs the redactor (TASK-26-22) | `crates/aether-cli/src/headless/run.rs` | 236 | `                .with_redactor(redactor);` |
| H4 | Header line written immediately after the writer is opened | `crates/aether-cli/src/headless/run.rs` | 237 | `            writer.write_header(crate::version::aether_version(), &started_at).map_err(CliError::IoError)?;` |

Reproduce (line numbers confirmed against the source at base commit
`96a0ba71`):

```
$ sed -n '181p;234p;236p;237p' crates/aether-cli/src/headless/run.rs
```

(The sed output reproduces the "One verbatim line" column above.)

The full call chain at H2–H4 is the body of the
`Some(path) => { … }` arm of `match &config.transcript_jsonl` (whose
`let` opens on the line above, `let mut transcript = match &config.transcript_jsonl {`).
H1 is the second of two physical lines that make up the
`Some(path) => …` arm of `match &config.resume` (the `let` opens on
`let resume_messages = match &config.resume {` at line 180). The line
table records the line where each named call actually appears; the
surrounding `match` is named in the prose above.

---

## 4. Cross-reference: which brief claim leans on which artifact

| Brief claim | Artifact it leans on | Path:line |
|-------------|----------------------|-----------|
| Credential redaction (TASK-26-22) | `redactor` field + `redact_bytes` call sites | `crates/aether-cli/src/transcript.rs:114`, `:187`, `:218`; builder `:160`; default `:146`; headless wiring `crates/aether-cli/src/headless/run.rs:236` |
| Run resume (TASK-26-20) | `load_resume_messages` reader | `crates/aether-cli/src/transcript.rs:316`; called from `crates/aether-cli/src/headless/run.rs:181` |
| Header shape (one `{"type":"header",...}` line + one event line per emitted `CliEventKind`) | doc-quoted schema + writer entry points | `crates/aether-cli/src/transcript.rs:23` (schema) and `:179` (`write_header`); per-event write at `:203` |
| Rotation (`max_bytes` -> `<stem>.1`) | `create_with_max_bytes` / `rotate` / `rotated_path` | `crates/aether-cli/src/transcript.rs:137`, `:240`, `:255`; opened at `crates/aether-cli/src/headless/run.rs:234` |

`reads-not-changed` (TASK-26-275/276/277), the killed shell command
(TASK-26-395) and `model-per-turn` (TASK-23-339) are part of the broader
TASK-2B-343 survey but are not the subject of this CLI-side part; the
persisted/session side and the eval SDK / ACP / TUI surfaces are owned
by the other parts (and the parallel TASK-2B-344 series that already
landed in commits `f955f53e` through `96a0ba71`).

The Glossary `Transcript` entry (`unknown` in the brief) is not added by
this part: TASK-2B-270 is explicitly "no brief change", and the answer
is written back by its own decision task.

---

## 5. Method

- Read `crates/aether-cli/src/transcript.rs` end-to-end (1199 lines) in
  50-line windows. Captured the module doc (lines 1-45), the
  `TranscriptRecord` / `JsonlTranscript` declarations, the constructor
  chain, the writer entry points, the rotation helpers, the resume
  reader and its error enum, and the resume builder.
- Read `crates/aether-cli/src/headless/run.rs` lines 1-240 (focus
  170-240) and captured both the `--resume` and `--transcript-jsonl`
  call sites plus the redactor wire-up.
- Picked each artifact's "defining line" as the single declaration /
  signature line; the doc block above the line (when present) is
  summarised in the prose around the table but is not quoted twice.
- Verified every line number against the source at base commit
  `96a0ba71` with `git ls-files` (file is unchanged at this commit)
  and `sed -n` so the line column can be re-derived by a reviewer.
- Did not edit any source file. `git status --porcelain` is clean
  after the commit that adds only this file.
