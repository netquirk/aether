# TASK-2B-343 (Part 3 of 4) — Brief `transcript` lines and the exact hit count in `.prairie/BRIEF.md`

Read-only evidence pass. No source file is edited by this task; only this
one new markdown file is added to the project. Verified before writing:

```
$ git rev-parse HEAD
69d5d07c230cbc351bb5b8b920162c87a14393ea      # TASK-2B-397 (Part 4 of 4 of TASK-2B-343)
$ git status --porcelain
                                                # clean (only the file added by this task is new)
```

The task's three asks — quote the five lines verbatim, fix the exact
`transcript` count, and state explicitly that line 25 (reads-not-changed,
TASK-26-275/276/277) does **not** contain the word — are all answered
below. Each line number in the quote list is re-derived with `sed` from
the source at the base commit, not trusted from the task body.

---

## 1. The five lines from `.prairie/BRIEF.md`, quoted verbatim

Reproduced by `sed -n '25p;26p;27p;73p;92p' .prairie/BRIEF.md` and pasted
here unchanged (preserving the leading `- `, the bold `**Transcript**`,
the backticks around `ToolPolicy` / `crates/aether-sessions/migrations/*.sql`,
and the em dash `—`). A `diff -u` between the `sed` output and the five
quote blocks below prints nothing (byte-match confirmed).

**Line 25**
```
- Observable and explainable runs: the run log names the config file and profile loaded (TASK-26-858, TASK-27-16/17/18), the model and provider used (TASK-25-119), the files read but not changed (TASK-26-275, TASK-26-276, TASK-26-277), the token usage, cost and wall-clock time (TASK-25-7, TASK-24-278, TASK-25-148), and the refusal names the offending path, flag, or reason (TASK-26-394, TASK-26-110, TASK-26-472).
```

**Line 26**
```
- Safety and sandboxing: tool calls reaching outside the working directory are refused, naming the path (TASK-26-394); a path-confining `ToolPolicy` is installed in every runtime the CLI spawns (TASK-26-614, TASK-26-615); credential-looking values are redacted before they are written to the log or transcript (TASK-26-22).
```

**Line 27**
```
- Resilience: stream cuts without a finish reason are retried and a cut call resumes from its partial reply (TASK-23-191); repeated turns are stopped, not looped (TASK-23-227); a run can be resumed from a saved transcript (TASK-26-20).
```

**Line 73**
```
- **Transcript** — unknown (not defined in README.md or `crates/aether-sessions/migrations/*.sql`).
```

**Line 92**
```
- Define "Transcript" for the `## Glossary` entry that currently reads `unknown`, or confirm it stays undefined.
```

---

## 2. The exact `transcript` hit lines and count in the brief

`rg -n -i "transcript" .prairie/BRIEF.md` returns exactly four lines:

```
26:- Safety and sandboxing: tool calls reaching outside the working directory are refused, naming the path (TASK-26-394); a path-confining `ToolPolicy` is installed in every runtime the CLI spawns (TASK-26-614, TASK-26-615); credential-looking values are redacted before they are written to the log or transcript (TASK-26-22).
27:- Resilience: stream cuts without a finish reason are retried and a cut call resumes from its partial reply (TASK-23-191); repeated turns are stopped, not looped (TASK-23-227); a run can be resumed from a saved transcript (TASK-26-20).
73:- **Transcript** — unknown (not defined in README.md or `crates/aether-sessions/migrations/*.sql`).
92:- Define "Transcript" for the `## Glossary` entry that currently reads `unknown`, or confirm it stays undefined.
```

`rg -i -c "transcript" .prairie/BRIEF.md` returns:

```
4
```

**Confirmed hit-line list: `{26, 27, 73, 92}`. Confirmed count: 4.**

The case-insensitive match yields one occurrence per line (each line
contains the substring `transcript` exactly once), and `-c` counts
matching lines — so the literal-string count in the brief is **four**,
not five.

---

## 3. Line 25 does **NOT** contain the word `transcript`

`sed -n '25p' .prairie/BRIEF.md | rg -i -c transcript` prints `0` and
`rg` exits 1 (no match). The full line is shown in §1 above: it
references `TASK-26-275`, `TASK-26-276`, `TASK-26-277` (the
reads-not-changed evidence) but never spells the word `transcript`.
This is why the parent task's "five places the brief leans on the word"
collapses to **four** brief lines: line 25 is paired with the transcript
concept by the parent task's mapping, but the word itself is absent from
that line. The other two parent-task references — the killed shell
command `TASK-26-395` and `model-per-turn` `TASK-23-339` — do **not**
appear in `.prairie/BRIEF.md` at all (verified with
`rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md`, which prints
nothing; the only `TASK-26-*` evidence on line 25 is the
reads-not-changed triple).

---

## 4. Other occurrences across the repo tree

`rg -i "transcript" . | wc -l` reports **1160** matching lines across
**101** files (`rg -l -i "transcript" . | wc -l` → 101). Case-insensitive
matching catches capitalized identifiers (`Transcript`,
`TranscriptRecord`, `TranscriptError`, `TranscriptHeader`) and prose
alongside the lowercase word, so all counts below are case-insensitive.

Top per-file counts (`rg -i -c "transcript" . | sort -t: -k2 -rn`,
truncated to the top 20 files):

| # | File | Lines |
|---|------|------:|
| 1 | `crates/aether-cli/src/transcript.rs` | 167 |
| 2 | `aether-transcript-survey-readme-migrations-conclusion.md` | 96 |
| 3 | `aether-transcript-survey-headless-session.md` | 77 |
| 4 | `crates/aether-evals/src/agents/transcript.rs` | 72 |
| 5 | `aether-transcript-survey-eval-sdk.md` | 64 |
| 6 | `crates/aether-cli/tests/integration/resume.rs` | 59 |
| 7 | `aether-transcript-survey-tui-docs.md` | 56 |
| 8 | `crates/aether-cli/tests/integration/transcript_jsonl.rs` | 47 |
| 9 | `aether-transcript-survey-cli.md` | 39 |
| 10 | `aether-transcript-survey-acp.md` | 36 |
| 11 | `crates/aether-cli/src/headless/mod.rs` | 34 |
| 12 | `crates/aether-cli/src/headless/run.rs` | 29 |
| 13 | `crates/aether-cli/tests/integration/shell_command_timeout.rs` | 28 |
| 14 | `crates/aether-evals/README.md` | 27 |
| 15 | `crates/aether-evals/src/judge.rs` | 26 |
| 16 | `aether-transcript-survey-persistence.md` | 21 |
| 17 | `packages/aether-evals/test/task.test.ts` | 18 |
| 18 | `packages/aether-evals/test/sessionUsage.test.ts` | 14 |
| 19 | `packages/aether-evals/src/transcript.ts` | 12 |
| 20 | `crates/aether-evals/src/assertions.rs` | 12 |

The other 81 files each contain ≤11 transcript occurrences. The
`aether-transcript-survey-*.md` files are the prior parts of the same
parent survey (TASK-2B-343 / TASK-2B-344); the Rust source, test, and
TypeScript files are the *artifacts* the other parts of TASK-2B-343
inventory. They are listed here as the broader tree; they are not
quoted and not changed by this part.

---

## 5. Mapping the four brief lines to the artifacts each leans on

The parent task names five brief claims (credential redaction,
run resume, reads-not-changed, killed shell command, model-per-turn).
After the §2 / §3 correction, only **three** of those five claims map
to a brief line that literally contains the word `transcript` (lines 26
and 27); lines 73 and 92 are the glossary entries themselves, not
"claims about an artifact" but the question being decided.

| Brief line | Claim | Artifact it leans on | Path:line |
|------------|-------|----------------------|-----------|
| **26** | credential redaction (TASK-26-22) — *"credential-looking values are redacted before they are written to the log or transcript"* | `redactor` field + `redact_bytes` call sites + headless wiring | `crates/aether-cli/src/transcript.rs:114`, `:146`, `:160`, `:187`, `:218`; wired in `crates/aether-cli/src/headless/run.rs:236` |
| **27** | run resume (TASK-26-20) — *"a run can be resumed from a saved transcript"* | `load_resume_messages` reader + `--resume` call site | `crates/aether-cli/src/transcript.rs:316`; called from `crates/aether-cli/src/headless/run.rs:181` |
| **25** | reads-not-changed (TASK-26-275/276/277) — *"the files read but not changed"* | the headless output stream that ends with `Files changed: N ...` and reports `0` for a read-only run | the prose at `packages/website/src/content/docs/aether/running/headless.mdx:87` describes this stream as *"the transcript"*; no path:line in `crates/` defines the read-only tally (it's a derived display) |
| **73** | glossary entry — *"Transcript — unknown (not defined in README.md or `crates/aether-sessions/migrations/*.sql`)"* | none (this is the question the glossary leaves open) | the three-artifact conclusion at `aether-transcript-survey-readme-migrations-conclusion.md:283` is the candidate answer |
| **92** | open question — *"Define 'Transcript' for the `## Glossary` entry that currently reads `unknown`, or confirm it stays undefined."* | none (decision item, owned by TASK-2B-270) | filed under TASK-2B-270 / `prairie decide`; not a source claim |

The two parent-task claims that **do not** appear in the brief at all
(verified with `rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md`,
empty output) are deferred to the other parts of the survey:

- **Killed shell command (TASK-26-395)** — belongs to the headless /
  shell-timeout side; its evidence lives in
  `crates/aether-cli/tests/integration/shell_command_timeout.rs:28`
  (the integration test that exercises the killed-shell path). Not a
  brief line.
- **Model-per-turn (TASK-23-339)** — belongs to the persisted /
  per-turn row side; the artifact it leans on is `TurnEntry` at
  `crates/aether-sessions/src/transcript.rs:20` (which carries
  `provider: Option<String>` and `model_id: Option<String>` per turn).
  Not a brief line.

The conclusion's "three-artifact statement" at
`aether-transcript-survey-readme-migrations-conclusion.md:283` covers
both — they don't need brief lines of their own.

---

## 6. Correction to the CLI survey's "five places" footnote

`aether-transcript-survey-cli.md:133–138` reads:

> `reads-not-changed` (TASK-26-275/276/277), the killed shell command
> (TASK-26-395) and `model-per-turn` (TASK-23-339) are part of the broader
> TASK-2B-343 survey but are not the subject of this CLI-side part …

That footnote is correct: those three claims are *not* covered by the
CLI part. But the parent task's wording "the five places the brief
leans on the word" implies a count of five brief lines. That count is
**four** (lines 26, 27, 73, 92), and the three claims above are not in
the brief at all — they live in task records, not in
`.prairie/BRIEF.md`. The CLI survey's footnote is **not modified by
this task** (read-only constraint); the correction lives here in §3 and
§5 of this report. The CLI survey was committed in `96a0ba71` and
remains byte-identical at this base commit.

---

## 7. Method note

- `sed -n '25p;26p;27p;73p;92p' .prairie/BRIEF.md` was used to read
  the five brief lines; `diff -u` against the §1 quotes prints
  nothing.
- `rg -n -i "transcript" .prairie/BRIEF.md` and
  `rg -i -c "transcript" .prairie/BRIEF.md` were used to derive the
  hit list and the count of 4.
- `sed -n '25p' .prairie/BRIEF.md | rg -i -c transcript` (exit 1,
  count 0) was used to confirm line 25 has no `transcript` substring.
- `rg -n "TASK-26-395|TASK-23-339" .prairie/BRIEF.md` (empty output)
  was used to confirm the two non-claim references are not in the
  brief.
- `rg -l -i "transcript" . | wc -l` (101) and
  `rg -i "transcript" . | wc -l` (~1160) give the repo-wide totals.
- `git rev-parse HEAD` and `git status --porcelain` were checked
  before writing; the working tree is clean apart from the new file.
- No source, brief, migration, or test file is edited.
