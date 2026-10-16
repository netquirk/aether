# TASK-2B-343 (Part 4 of 4) — Map the five claims to their artifacts and the count reconciliation

This is the consolidated survey that ties the three prior inventories
(`aether-transcript-survey-cli.md` Part 1, `aether-transcript-survey-persistence.md`
Part 2, `aether-transcript-survey-brief.md` Part 3) to the five brief claims
named in the parent task: **credential redaction (TASK-26-22)**, **run resume
(TASK-26-20)**, **reads-not-changed (TASK-26-275/276/277)**, **killed shell
command (TASK-26-395)**, and **model-per-turn (TASK-23-339)**. For each claim
this file records the artifact it leans on, the path:line, the **one verbatim
defining line** that defines it, and the count reconciliation between the
task's "five brief claims" premise and what the brief actually contains.

Read-only evidence pass. No source file is edited by this task; only this
one new markdown file is added. Verified before writing:

```
$ git rev-parse HEAD
364f1bf37e908a33307364b9193f261c1b231671      # base commit (this task's base)
$ git status --porcelain
                                                # clean
```

Every quoted line below was re-derived with `sed` against the source at
this base commit, not trusted from the task body. Line numbers in the
"Path:line" column are confirmed by the `Reproduce` block at the end of
each section; a reviewer can re-run those commands and see a byte-identical
match against the quote cell.

---

## 1. The five-row claim table

### Row 1 — Credential redaction (TASK-26-22)

| Field | Value |
|-------|-------|
| Brief task | TASK-26-22 (Quality goals → Safety and sandboxing: *"credential-looking values are redacted before they are written to the log or transcript"*) |
| Brief line that contains the word | `.prairie/BRIEF.md:26` |
| Artifact leaned on | JSONL writer redaction wiring in `crates/aether-cli/src/transcript.rs` (the `redactor` field on `JsonlTranscript`, the `with_redactor` builder, the `redact_bytes` call sites in `write_header` and `record`); headless-side wire-up at `crates/aether-cli/src/headless/run.rs:236` |
| Source defining lines (verbatim) | see below |

Verbatim defining lines, re-derived with
`sed -n '114p;146p;160p;179p;187p;203p;218p' crates/aether-cli/src/transcript.rs`:

```
    redactor: SecretRedactor,                                       # 114 (field on JsonlTranscript)
            redactor: SecretRedactor::new(),                        # 146 (default in create_with_max_bytes)
    pub fn with_redactor(mut self, redactor: SecretRedactor) -> Self {   # 160 (builder)
    pub fn write_header(&mut self, aether_version: &str, started_at: &str) -> io::Result<()> {   # 179 (header writer)
        let bytes = self.redactor.redact_bytes(&bytes);             # 187 (redact the header line)
    pub fn record(&mut self, event: &AgentEvent) -> io::Result<()> {   # 203 (per-event writer)
        let bytes = self.redactor.redact_bytes(&bytes);             # 218 (redact every event line)
```

The headless side that installs the redactor on the writer
(`sed -n '234p;236p;237p' crates/aether-cli/src/headless/run.rs`):

```
            let mut writer = JsonlTranscript::create_with_max_bytes(path, config.transcript_max_bytes)   # 234
                .with_redactor(redactor);                          # 236 (redactor installed)
            writer.write_header(crate::version::aether_version(), &started_at).map_err(CliError::IoError)?;   # 237
```

This row's brief line 26 is the only brief line in the survey that names
"transcript" and pairs it with TASK-26-22; the writer line quoted
(`write_header` / `record` / `redact_bytes`) is the artifact the brief
leans on. Reproduced from the CLI survey
(`aether-transcript-survey-cli.md` Part 1, §2 rows R1–R5 and §3 row H3).

### Row 2 — Run resume (TASK-26-20)

| Field | Value |
|-------|-------|
| Brief task | TASK-26-20 (Quality goals → Resilience: *"a run can be resumed from a saved transcript"*) |
| Brief line that contains the word | `.prairie/BRIEF.md:27` |
| Artifact leaned on | `load_resume_messages` in `crates/aether-cli/src/transcript.rs` and its doc comment naming the call site |
| Source defining lines (verbatim) | see below |

Verbatim defining lines, re-derived with
`sed -n '298p;316p' crates/aether-cli/src/transcript.rs`:

```
/// by [`JsonlTranscript`]. Used by `aether headless --resume PATH` to seed the   # 298 (doc line)
pub fn load_resume_messages(path: &Path) -> Result<Vec<ChatMessage>, TranscriptError> {   # 316 (declaration)
```

The full doc comment at lines 297–315 (the `Used by …` line is line 298)
names the function's contract: it walks a JSONL file produced by
`JsonlTranscript`, reconstructs the `Vec<ChatMessage>` from the events
whose contents appear, ignores purely-observability lines, and reports
schema/path errors through `TranscriptError` with a `Display` that names
the offending line.

The call site on the headless command line
(`sed -n '181p' crates/aether-cli/src/headless/run.rs`):

```
        Some(path) => crate::transcript::load_resume_messages(path)   # 181
```

This row's brief line 27 is the only brief line in the survey that names
"transcript" and pairs it with TASK-26-20; the artifact is
`load_resume_messages` (line 316) and its `Used by` doc line (line 298).
Reproduced from the CLI survey (`aether-transcript-survey-cli.md` Part 1,
§1 row 12 and §3 row H1).

### Row 3 — Reads-not-changed (TASK-26-275 / TASK-26-276 / TASK-26-277)

| Field | Value |
|-------|-------|
| Brief task | TASK-26-275 / TASK-26-276 / TASK-26-277 (Quality goals → Observable and explainable runs: *"the files read but not changed"*) |
| Brief line | `.prairie/BRIEF.md:25` |
| Brief line contains the word `transcript`? | **No** — verified by `sed -n '25p' .prairie/BRIEF.md \| rg -i -c transcript` → `0` (exit 1) |
| Artifact leaned on | the brief's own line 25 is the citation; the read-only tally is a derived display on the headless run output, not a Rust declaration of an artifact called "transcript" |

Verbatim defining line, re-derived with `sed -n '25p' .prairie/BRIEF.md`:

```
- Observable and explainable runs: the run log names the config file and profile loaded (TASK-26-858, TASK-27-16/17/18), the model and provider used (TASK-25-119), the files read but not changed (TASK-26-275, TASK-26-276, TASK-26-277), the token usage, cost and wall-clock time (TASK-25-7, TASK-24-278, TASK-25-148), and the refusal names the offending path, flag, or reason (TASK-26-394, TASK-26-110, TASK-26-472).
```

The "files read but not changed" triple is part of a quality goal that
the headless run log enforces by emitting a `Files changed: N ...` line
(with `N = 0` for a read-only run); no `transcript` Rust artifact owns
that tally. The brief survey's §5 row for line 25
(`aether-transcript-survey-brief.md` Part 3, §5) records this as a
derived display on the headless output stream. The word `transcript` is
**not** in line 25 (the case-insensitive grep returns 0), which is why
the parent task's "five places the brief leans on the word" framing
collapses for this row.

### Row 4 — Killed shell command (TASK-26-395)

| Field | Value |
|-------|-------|
| Brief task | TASK-26-395 (per-command shell timeout / kill) |
| Brief line that contains the word | **none** — verified by `rg -n "TASK-26-395" .prairie/BRIEF.md` → empty |
| Artifact leaned on | per-command shell timeout field on `RunSettings` in `crates/aether-project/src/aether_settings.rs` (doc comment, field, and a `// TASK-26-395:` cross-reference in a parsing test) |
| Source defining lines (verbatim) | see below |

Verbatim defining lines, re-derived with
`sed -n '223p;229p;2193p' crates/aether-project/src/aether_settings.rs`:

```
    /// `coding` MCP server) may run before it is killed (TASK-26-395). Unset   # 223 (doc comment)
    pub shell_command_timeout_seconds: Option<u64>,                          # 229 (field)
        // TASK-26-395: per-command shell timeout lives at                    # 2193 (test cross-reference)
```

The doc comment at lines 222–227 names the field as the seconds a
tool-spawned shell command (the `bash` tool of the built-in `coding` MCP
server) may run before it is killed; the field at line 229 holds the
value; the test cross-reference at line 2193 anchors the camelCase
JSON spelling `run.shellCommandTimeoutSeconds`.

This row is **NOT in the brief** — the brief never references
TASK-26-395. The brief survey's §5 ("other parent-task claims") records
this under "Killed shell command (TASK-26-395)" with a `Not a brief
line` note. The actual evidence lives in the integration test
`crates/aether-cli/tests/integration/shell_command_timeout.rs:28`, but
that test is not the "defining artifact" — the field at line 229 is.

### Row 5 — Model-per-turn (TASK-23-339)

| Field | Value |
|-------|-------|
| Brief task | TASK-23-339 (per-turn provider/model record) |
| Brief line that contains the word | **none** — verified by `rg -n "TASK-23-339" .prairie/BRIEF.md` → empty |
| Artifact leaned on | **none** — the string `TASK-23-339` does not appear in `.prairie/BRIEF.md` and does not appear in any Rust source, migration, or test file in this repository |
| Where TASK-23-339 *is* mentioned | only in the three prior survey markdown files (`aether-transcript-survey-cli.md:134`, `aether-transcript-survey-brief.md:92, 146, 169, 186, 211`) — i.e. as a cross-reference inside this survey series, not as a code claim |
| Nearest real artifact (NOT a defining line for TASK-23-339) | `crates/aether-sessions/src/transcript.rs:20` `pub struct TurnEntry {` — a `TurnEntry` carries `provider: Option<String>` and `model_id: Option<String>` per turn, which is the only place in the source tree that records provider/model per turn |

Verbatim defining line for the nearest real artifact
(`sed -n '20p' crates/aether-sessions/src/transcript.rs`):

```
pub struct TurnEntry {
```

This row is **not found** in the brief and not found in any source
file. The brief survey's §5 records this under "Model-per-turn
(TASK-23-339)" as "not a brief line". The CLI survey's §4 footnote
(`aether-transcript-survey-cli.md:133–138`) already says so explicitly:

> `reads-not-changed` (TASK-26-275/276/277), the killed shell command
> (TASK-26-395) and `model-per-turn` (TASK-23-339) are part of the broader
> TASK-2B-343 survey but are not the subject of this CLI-side part …

The persistence survey (`aether-transcript-survey-persistence.md` Part 2,
§1 row 1) records the `TurnEntry` declaration as a separate artifact
without claiming it defines TASK-23-339. The model-per-turn task
identifier is therefore recorded as a **citation without a code
anchor** — present in the survey series, absent in the source tree and
the brief.

---

## 2. The count reconciliation (verbatim)

The parent task's "five brief claims" premise is **partly wrong**.
The case-insensitive transcript grep of `.prairie/BRIEF.md` returns
**four** matching lines, not five; and the mapping to the five claims
is not five-brief-lines-for-five-claims but a mix of brief lines,
brief-absent artifacts, and citation-without-code-anchor:

```
$ rg -i -c "transcript" .prairie/BRIEF.md
4
$ rg -n -i "transcript" .prairie/BRIEF.md
26:- Safety and sandboxing: tool calls reaching outside the working directory are refused, naming the path (TASK-26-394); a path-confining `ToolPolicy` is installed in every runtime the CLI spawns (TASK-26-614, TASK-26-615); credential-looking values are redacted before they are written to the log or transcript (TASK-26-22).
27:- Resilience: stream cuts without a finish reason are retried and a cut call resumes from its partial reply (TASK-23-191); repeated turns are stopped, not looped (TASK-23-227); a run can be resumed from a saved transcript (TASK-26-20).
73:- **Transcript** — unknown (not defined in README.md or `crates/aether-sessions/migrations/*.sql`).
92:- Define "Transcript" for the `## Glossary` entry that currently reads `unknown`, or confirm it stays undefined.
```

**Confirmed hit-line list: `{26, 27, 73, 92}`. Confirmed count: 4.**

How that maps to the five claim rows (this is the reconciliation the
parent task asks for):

| Row | Claim | Anchor | Brief line(s) that contain the word `transcript`? |
|-----|-------|--------|---------------------------------------------------|
| 1 | credential redaction (TASK-26-22) | `redactor` + `redact_bytes` in `crates/aether-cli/src/transcript.rs` | yes — line 26 |
| 2 | run resume (TASK-26-20) | `load_resume_messages` at `crates/aether-cli/src/transcript.rs:316` | yes — line 27 |
| 3 | reads-not-changed (TASK-26-275/276/277) | brief line 25 (derived display) | **no** — `rg -i -c transcript` on line 25 returns 0 |
| 4 | killed shell command (TASK-26-395) | `RunSettings::shell_command_timeout_seconds` at `crates/aether-project/src/aether_settings.rs:229` | **no** — TASK-26-395 is not in the brief |
| 5 | model-per-turn (TASK-23-339) | none (citation only) | **no** — TASK-23-339 is not in the brief and not in any source file |

So the task's "five places the brief leans on the word `transcript`"
is only literally true for **two** of the five rows (rows 1 and 2);
line 25 (row 3) does not contain the word; rows 4 and 5 are not in the
brief at all. The four brief lines that do contain the word are
**lines 26 and 27** (the two quality-goal lines that pair the word
with TASK-26-22 and TASK-26-20) and **lines 73 and 92** (the glossary
entry and the open question — neither of which is a "claim about an
artifact" but the question the survey is trying to answer).

The brief survey (`aether-transcript-survey-brief.md` Part 3) gives
the same number and the same hit list (§2 and §3); this Part 4
consolidates the count discrepancy into the five-row claim table and
records that of the five claim rows, **only two are anchored on a
brief line that literally contains the word `transcript`**. No
fabricated brief text is introduced.

The two task IDs the brief does not mention at all
(`rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md` → empty) are
recorded in rows 4 and 5 of the table with their actual evidence
sources (the `RunSettings` field for row 4; the absence-of-code
plus the three-survey-markdown cross-reference for row 5).

---

## 3. Confirmation: no source file was modified (read-only pass)

```
$ git rev-parse HEAD
364f1bf37e908a33307364b9193f261c1b231671

$ git status --porcelain
?? aether-transcript-survey.md          # this file (the only tree change)

$ git diff --stat
                                  # empty (no tracked file changed)

$ git diff --name-only
                                  # empty (no tracked file changed)
```

No `.rs` file under `crates/`, no `.sql` file under
`crates/aether-sessions/migrations/`, no `.ts`/`.tsx`/`.mdx` file under
`packages/`, no `.toml` file under the workspace, and no `.md` file
under `.prairie/` was modified. The change set is the single new
markdown file above, matching how the three prior parts each added one
new markdown file (`aether-transcript-survey-cli.md`,
`aether-transcript-survey-persistence.md`,
`aether-transcript-survey-brief.md`).

The other two absence-of-brief-claim checks the plan requires:

```
$ rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md
                                  # empty (rows 4 and 5 not in the brief)
$ rg -n "TASK-23-339" crates packages .prairie
                                  # empty (row 5 not in any source, migration, or brief)
```

The full-tree `rg -n "TASK-23-339" .` (without scope filtering) does
return hits, but only inside the three prior survey markdown files
themselves (`aether-transcript-survey-cli.md:134`,
`aether-transcript-survey-brief.md:92,146,169,186,211`) — i.e. the
citation lives only inside this survey series, not in any code that
the survey describes.

---

## 4. Acceptance criteria check

| Acceptance criterion | Status |
|----------------------|--------|
| Five-row mapping with a quoted defining line per artifact | rows 1–5 above; each row has at least one `sed`-derived verbatim quote |
| Count discrepancy stated | §2: `rg -i -c "transcript" .prairie/BRIEF.md` → `4`; hit list `{26,27,73,92}`; only two of the five claim rows anchor on a brief line that contains the word; line 25 does not contain it |
| No source file modified (read-only pass) | §3: only `?? aether-transcript-survey.md`; `git diff --stat` empty; `git diff --name-only` empty |
| No fabricated brief text | all brief text in §1 and §2 is byte-identical to `sed -n '25p;26p;27p;73p;92p' .prairie/BRIEF.md` |
| No `prairie create\|block\|steer\|finish` invoked | none; no `prairie` command at all in this pass |

---

## 5. Reproduce (single command, byte-matches the quote cells)

```
$ git rev-parse HEAD
364f1bf37e908a33307364b9193f261c1b231671

# Row 1 — credential redaction (TASK-26-22)
$ sed -n '114p;146p;160p;179p;187p;203p;218p' crates/aether-cli/src/transcript.rs
$ sed -n '234p;236p;237p' crates/aether-cli/src/headless/run.rs

# Row 2 — run resume (TASK-26-20)
$ sed -n '298p;316p' crates/aether-cli/src/transcript.rs
$ sed -n '181p' crates/aether-cli/src/headless/run.rs

# Row 3 — reads-not-changed (TASK-26-275/276/277)
$ sed -n '25p' .prairie/BRIEF.md

# Row 4 — killed shell command (TASK-26-395)
$ sed -n '223p;229p;2193p' crates/aether-project/src/aether_settings.rs

# Row 5 — model-per-turn (TASK-23-339) — nearest real artifact only
$ sed -n '20p' crates/aether-sessions/src/transcript.rs

# Count reconciliation
$ rg -i -c "transcript" .prairie/BRIEF.md
$ rg -n -i "transcript" .prairie/BRIEF.md
$ sed -n '25p' .prairie/BRIEF.md | rg -i -c transcript   # exit 1, count 0
$ rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md       # empty
$ rg -n "TASK-23-339" crates packages .prairie             # empty

# Read-only confirmation
$ git status --porcelain
$ git diff --stat
$ git diff --name-only
```

A `diff -u` between any of the `sed` outputs above and the corresponding
quote cell in §1 prints nothing — every line number in the "Path:line"
column is current at base commit `364f1bf3`.
