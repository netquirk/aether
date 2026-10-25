//! Redaction of credential-looking values in the run log and the transcript
//! (TASK-26-22).
//!
//! A run log (TASK-25-48) or a JSONL transcript (TASK-24-277) shared for
//! debugging must not leak an API key or token the operator set for the run.
//! This module owns two pieces:
//!
//! - [`SecretRedactor`]: an immutable, cheaply cloneable bag of known-secret
//!   strings plus a hand-rolled case-insensitive `bearer <token>` matcher.
//!   Its [`SecretRedactor::redact`] replaces every occurrence with the
//!   [`REDACTED`] marker so the leaked value never reaches the wire. The
//!   marker is `[REDACTED]` — no JSON-special characters, safe inside a
//!   serialized JSON line and inside a tracing text line.
//! - [`RedactingMakeWriter`]: a `tracing_subscriber::fmt::writer::MakeWriter`
//!   that buffers until a `\n`, redacts the completed line, then forwards it to
//!   the inner writer. Wrapping the base writer installed in
//!   [`crate::headless::run::setup_tracing`] covers the stderr / `--log-file`
//!   / `--log-max-bytes` cases in one place. Rotation (TASK-26-92) lives
//!   underneath, so rotation sees whole, already-redacted lines.
//!
//! No new external dependency is added: the redactor is hand-rolled so we
//! avoid pulling `regex` into the binary for one pattern.

use std::collections::BTreeMap;
use std::io;

use aether_core::agent_spec::AgentSpec;
use llm::LlmModel;
use tracing_subscriber::fmt::writer::MakeWriter;

/// Replacement marker emitted in place of every matched credential value.
///
/// The marker is intentionally plain ASCII with no JSON-special characters, so
/// it can be substituted into a serialized JSON line without further escaping
/// and still produce valid JSON Lines output. It is also safe to drop into a
/// human-readable `tracing` text line.
pub const REDACTED: &str = "[REDACTED]";

/// Threshold below which a candidate secret value is silently dropped from the
/// known-secrets list. Short strings would create false positives
/// (`Bearer a`, `Bearer 1`) without giving any meaningful protection, so we
/// require a minimum length and fall back to the bearer-pattern pass for very
/// short tokens. The threshold is intentionally low — four bytes matches the
/// shortest plausible credentials while skipping trivial inputs.
const MIN_SECRET_LEN: usize = 4;

/// Minimum length (in bytes) of the token after a `bearer ` prefix for the
/// bearer-pattern pass to fire. Common English words (`"is"`, `"of"`,
/// `"a"`, …) that happen to be 2–4 letters long are deliberately excluded
/// so a sentence like `"the bearer is here"` does not get redacted as a
/// credential. Real bearer tokens are routinely dozens of bytes (a JWT is
/// already three base64url segments of 8+ chars each); the seven-byte
/// floor still catches compact hand-rolled tokens (`Bearer abc.def`)
/// while keeping short prose words like `"is"`, `"of"`, `"the"`, `"bond"`
/// untouched when they happen to follow "bearer".
const MIN_BEARER_TOKEN_LEN: usize = 7;

/// Bundle of known-secret values plus a sender for the same redaction rule.
///
/// Cheap to clone (the inner `Vec` is small) so two sinks — the run log and
/// the transcript — can share a single configuration without a shared
/// `Arc`/`Mutex`. The redactor is otherwise stateless and readonly after
/// construction: the redact pass is pure-functional over the input text.
#[derive(Clone, Debug, Default)]
pub struct SecretRedactor {
    /// Known secret strings (longer than `MIN_SECRET_LEN`) the redactor should
    /// replace verbatim with [`REDACTED`] before scanning for bearer
    /// patterns. Empty strings are filtered out at insertion time.
    secrets: Vec<String>,
}

impl SecretRedactor {
    /// Build an empty redactor. Only the bearer-pattern pass (`bearer <token>`)
    /// runs; nothing else is substituted. Existing callers that never seed a
    /// secret keep working without further changes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style add: append a known secret value, ignoring empties and
    /// strings shorter than `MIN_SECRET_LEN` so a config bug (or a stray
    /// debug environment) does not silently over-redact ordinary short
    /// identifiers.
    ///
    /// Returns `self` for chaining.
    pub fn with_secret(mut self, value: impl Into<String>) -> Self {
        let raw = value.into();
        if raw.len() >= MIN_SECRET_LEN {
            // Deduplicate so a value that is set both via env var and inline
            // (or appears in two env vars) is not held twice. `Vec::contains`
            // is fine: `secrets` is small (single-digit items per run).
            if !self.secrets.iter().any(|existing| existing == &raw) {
                self.secrets.push(raw);
            }
        }
        self
    }

    /// Build a redactor seeded with every credential the process environment
    /// could supply at this point: each name in
    /// [`llm::LlmModel::ALL_REQUIRED_ENV_VARS`] that is set, plus the
    /// `CUSTOM_API_KEY` env var. Used before any resolved `AgentSpec` is
    /// available — typically for the early `setup_tracing` install in the
    /// dry-run code path, where inline `provider_connections` overrides
    /// cannot be known yet because spec has not been resolved.
    ///
    /// An empty string is filtered out at insertion by [`Self::with_secret`]
    /// so unset / empty env vars are no-ops. The bearer-pattern pass always
    /// fires regardless of the seeded list.
    pub fn from_env() -> Self {
        let mut redactor = Self::new();
        for name in LlmModel::ALL_REQUIRED_ENV_VARS {
            if let Ok(value) = std::env::var(name) {
                redactor = redactor.with_secret(value);
            }
        }
        if let Ok(value) = std::env::var("CUSTOM_API_KEY") {
            redactor = redactor.with_secret(value);
        }
        redactor
    }

    /// Build a redactor seeded with every credential the `aether` CLI would
    /// plausibly pass to a provider for `spec`:
    ///
    /// - The value of every name in
    ///   [`llm::LlmModel::ALL_REQUIRED_ENV_VARS`] that is set in the process
    ///   environment (matches the env-var list `sandbox.rs` already
    ///   consults), plus the `custom` provider's `CUSTOM_API_KEY`.
    /// - Each `api_key` configured inline on `spec.provider_connections`
    ///   (the override struct's per-provider key).
    /// - Each header **value** whose header **name** matches the Anthropic
    ///   provider's `should_redact_header` convention: `authorization`,
    ///   `x-api-key`, or contains `secret` or
    ///   `token` (case-insensitive). The Anthropic provider redacts those
    ///   header values from its own request-id line; we mirror the rule here
    ///   so the same headers the model sees redacted do not leak through a
    ///   `--log-file` write either.
    ///
    /// The redactor is constructed defensively: missing env vars, missing
    /// overrides, and missing headers are all no-ops. An empty string is
    /// filtered out at insertion by [`Self::with_secret`].
    pub fn from_env_and_spec(spec: &AgentSpec) -> Self {
        let mut redactor = Self::new();
        // Provider catalog env vars. Unset vars are skipped silently; running
        // the redaction logic on a missing var would be wasted work and would
        // confuse a reader of the seeded list.
        for name in LlmModel::ALL_REQUIRED_ENV_VARS {
            if let Ok(value) = std::env::var(name) {
                redactor = redactor.with_secret(value);
            }
        }
        // `custom` provider has no catalog entry so its env var isn't in
        // `ALL_REQUIRED_ENV_VARS`, but it's a recognised bearer for the
        // inline-key path (`ProviderConnectionOverride::api_key`).
        if let Ok(value) = std::env::var("CUSTOM_API_KEY") {
            redactor = redactor.with_secret(value);
        }

        // Per-provider inline overrides. Clone the merged map because
        // `into_inner()` takes ownership and `spec` is borrowed here;
        // the BTreeMap is a handful of entries per run, so the clone is
        // cheap relative to the rest of the run setup.
        for (_, override_value) in spec.provider_connections.clone().into_inner() {
            if let Some(api_key) = override_value.api_key {
                redactor = redactor.with_secret(api_key);
            }
            for (header_name, header_value) in override_value.headers {
                if should_redact_header(&header_name) {
                    redactor = redactor.with_secret(header_value);
                }
            }
        }

        redactor
    }

    /// Build a redactor seeded explicitly from an env lookup table and a
    /// `[name -> value]` map of headers. The split lets tests seed values
    /// without touching the process environment or constructing a real
    /// `AgentSpec`.
    ///
    /// `env_vars` carries the parsed env (`std::env::vars()`-shaped); `headers`
    /// carries the per-connection header pair. Empty values are dropped.
    pub fn from_sources<I, J, K>(env_vars: I, connection_headers: J) -> Self
    where
        I: IntoIterator<Item = (K, String)>,
        J: IntoIterator<Item = (String, BTreeMap<String, String>)>,
        K: AsRef<str>,
    {
        let mut redactor = Self::new();
        for (name, value) in env_vars {
            // Filter to the same names `from_env_and_spec` accepts so a test
            // can pass the result of `std::env::vars()` and still only seed
            // recognised names.
            if is_required_env_var(name.as_ref()) {
                redactor = redactor.with_secret(value);
            }
        }
        for headers in connection_headers {
            for (header_name, header_value) in headers.1 {
                if should_redact_header(&header_name) {
                    redactor = redactor.with_secret(header_value);
                }
            }
        }
        redactor
    }

    /// Number of known-secret strings currently held. Exposed for tests that
    /// want to assert dedup behaviour; not used by the redact path.
    pub fn known_secrets_len(&self) -> usize {
        self.secrets.len()
    }

    /// Apply redaction to a UTF-8 string. Returns the redacted string.
    ///
    /// Two passes run in order:
    ///
    /// 1. Every known secret is replaced with [`REDACTED`]. This catches exact
    ///    provider keys that don't appear next to a `bearer ` prefix (the
    ///    catalog env-var path).
    /// 2. A hand-rolled case-insensitive `bearer <token>` scan replaces the
    ///    match with [`REDACTED`]. The token is the run of characters in
    ///    `[A-Za-z0-9._~+/=-]` that follows the literal "bearer" word and at
    ///    least one ASCII whitespace character.
    ///
    /// Benign strings (`"hello world"`, `"bearer"` with no token, `"bearer: x"` —
    /// a colon, not whitespace) pass through byte-for-byte.
    pub fn redact(&self, text: &str) -> String {
        let mut output = text.to_string();
        // 1) Known-secret pass. The order of iterations does not matter
        // because every replacement replaces the exact same span with the
        // same marker; running them on the output of the previous iteration
        // would still leave the bytes correct but would invalidate the
        // starts-at-zero invariant of the next pass.
        for secret in &self.secrets {
            if output.contains(secret.as_str()) {
                output = output.replace(secret.as_str(), REDACTED);
            }
        }
        // 2) Bearer-pattern pass. Hand-rolled because pulling in `regex`
        // for one case-insensitive pattern would dominate the binary's
        // compile time and dead-code weight for negligible gain.
        redact_bearer_tokens(&mut output);
        output
    }

    /// Byte-level variant of [`Self::redact`]. Returns a fresh vector so the
    /// internal `String` never escapes via a borrowed reference. `serde_json`
    /// output is guaranteed UTF-8 (the `serde_json` crate rejects non-UTF-8
    /// bytes at the boundary), so the lossy round-trip on a non-UTF-8 input
    /// would never be hit in practice; using `from_utf8_lossy` keeps the
    /// public surface simple for the rare non-UTF-8 line.
    pub fn redact_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(bytes);
        self.redact(&text).into_bytes()
    }
}

/// Whether `name` is one of the env vars the redactor should scrape from the
/// process environment. Mirrors the catalog-driven list in
/// [`crate::sandbox`] plus the `custom` provider's `CUSTOM_API_KEY`.
fn is_required_env_var(name: &str) -> bool {
    LlmModel::ALL_REQUIRED_ENV_VARS.contains(&name) || name == "CUSTOM_API_KEY"
}

/// Whether `header_name` should be redacted when its value is picked up from a
/// `provider_connections.headers` entry. Mirrors the convention already in
/// use at `crates/llm/src/providers/anthropic/provider.rs::should_redact_header`:
/// `authorization` / `x-api-key` (case-insensitive), or any name whose lowercase
/// form contains the substring `secret` or `token`. Header-name lookups are
/// case-insensitive per RFC 7230.
fn should_redact_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization" || lower == "x-api-key" || lower.contains("secret") || lower.contains("token")
}

/// Scan `output` for `bearer <token>` (case-insensitive on the literal
/// `bearer`) and replace each match with [`REDACTED`]. Writes back into the
/// owned `String` so callers don't pay an allocation per match.
///
/// The token class is `[A-Za-z0-9._~+/=-]`, the RFC 6750 §2.1
/// `b64token` characters that survive unquoted use. A non-empty token is
/// required so the literal word "bearer" in plain prose is not replaced.
fn redact_bearer_tokens(output: &mut String) {
    // Walk the haystack twice: collect the (start, end) replacement ranges in
    // a single scan over the bytes, then apply them in a second pass with
    // `replace_range`. Splitting the passes avoids holding a borrow of
    // `output`'s bytes while we mutate the `String`.
    let mut replacements: Vec<(usize, usize)> = Vec::new();
    {
        let bytes = output.as_bytes();
        let len = bytes.len();
        let mut cursor = 0;

        while cursor < len {
            let Some(start) = find_bearer(bytes, cursor, len) else { break };
            let token_start = start + "bearer".len();
            if token_start >= len || !bytes[token_start].is_ascii_whitespace() {
                cursor = start + 1;
                continue;
            }
            let mut token_run_start = token_start + 1;
            // Skip whitespace runs between "bearer" and the token; we accept one
            // space, multiple spaces, or a tab.
            while token_run_start < len && bytes[token_run_start].is_ascii_whitespace() {
                token_run_start += 1;
            }
            if token_run_start >= len || !is_token_byte(bytes[token_run_start]) {
                cursor = start + 1;
                continue;
            }
            let mut token_end_run = token_run_start;
            while token_end_run < len && is_token_byte(bytes[token_end_run]) {
                token_end_run += 1;
            }
            // Require the token to be at least [`MIN_BEARER_TOKEN_LEN`] bytes so
            // that bare common-prose phrases like "the bearer is here" or
            // "bearer has" do not get redacted. Real bearer tokens (random
            // b64, JWTs, opaque session ids) are always at least 16 bytes;
            // the threshold sits at eight as a safety floor that still
            // catches compact hand-rolled tokens like `Bearer abc.def`.
            let token_len = token_end_run - token_run_start;
            if token_len < MIN_BEARER_TOKEN_LEN {
                cursor = start + 1;
                continue;
            }
            // Replace the whole `"bearer"` + whitespace span AND the trailing
            // token so the marker occupies the original span. A non-token
            // occurrence of "bearer" never reaches this point because the
            // empty-token guard above rejects it.
            replacements.push((start, token_end_run));
            cursor = token_end_run;
        }
    }

    // Apply replacements in reverse so earlier ranges stay valid as the
    // string shrinks. UTF-8 boundaries hold because every byte offset we
    // collected sits on an ASCII byte.
    for (start, end) in replacements.iter().rev() {
        output.replace_range(start..end, REDACTED);
    }
}

/// Find the next case-insensitive occurrence of the literal `bearer` starting
/// at `from` in `bytes`. Returns the byte offset of the first byte of the
/// match, or `None` when no further match exists. The match also has to sit
/// inside whitespace or a string boundary at the left, and at whitespace or
/// string boundary at the right (when followed by whitespace) — but this
/// helper does the raw find and lets [`redact_bearer_tokens`] apply the
/// boundary checks so the same scan can anchor on whitespace.
fn find_bearer(bytes: &[u8], from: usize, len: usize) -> Option<usize> {
    // ASCII byte literals; `bearer` is seven bytes. Pre-allocating the
    // lowercase search target keeps the inner loop branch-free.
    const TARGET: &[u8; 6] = b"bearer";
    let mut cursor = from;
    while cursor + TARGET.len() <= len {
        // Case-fold by uppercasing the byte and comparing to the uppercased
        // target. ASCII upper/lower for `[a-z]` is a single-bit XOR with
        // `0x20`, so adding `0x20` to an already-uppercase byte yields the
        // wrong letter; use an explicit equality test instead.
        let mut matched = true;
        for offset in 0..TARGET.len() {
            let left = bytes[cursor + offset];
            if !left.eq_ignore_ascii_case(&TARGET[offset]) {
                matched = false;
                break;
            }
        }
        if matched {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

/// True when `byte` is one of the characters allowed inside an RFC 6750
/// `b64token` (matching the same set as RFC 7235 §2.1 `token68`).
fn is_token_byte(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

// ---------------------------------------------------------------------------
// RedactingMakeWriter
// ---------------------------------------------------------------------------

/// `tracing_subscriber::fmt::writer::MakeWriter` implementation that buffers
/// each write, splits on newlines, and forwards only redacted, complete lines
/// to `inner`. The inner writer is wrapped as-is so stderr, plain file, and
/// rotating file writers all work without modification.
///
/// Lifetime note: `tracing_subscriber` constructs a fresh `Writer` for every
/// event via `make_writer`, so this type holds `RedactingMakeWriter` cloned by
/// `Arc` (via the `BoxMakeWriter`/`tracing-subscriber` shared-writer
/// pattern). The per-write guard is itself cheap to construct.
pub struct RedactingMakeWriter<W> {
    /// Inner writer factory, moved into the per-write guard by clone.
    inner: W,
    redactor: SecretRedactor,
}

/// Per-write handle produced by [`RedactingMakeWriter`]. Owns the inner
/// writer, a redactor clone, and a small `Vec<u8>` line buffer. `Drop` is
/// implemented so a trailing unterminated write still flushes the buffer
/// before the guard is discarded; without that, a `tracing::info!` at run
/// end without a trailing newline would silently drop its tail.
pub struct RedactingWriter<W: io::Write> {
    inner: W,
    redactor: SecretRedactor,
    buffer: Vec<u8>,
}

impl<'a, M> MakeWriter<'a> for RedactingMakeWriter<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter { inner: self.inner.make_writer(), redactor: self.redactor.clone(), buffer: Vec::new() }
    }
}

impl<W> RedactingMakeWriter<W> {
    /// Convenience constructor so callers do not have to spell out the generic
    /// parameters when wrapping a typed writer (the `BoxMakeWriter` path
    /// collapses both type parameters into one).
    #[allow(dead_code)]
    pub fn new(inner: W, redactor: SecretRedactor) -> Self {
        Self { inner, redactor }
    }
}

impl<W: io::Write> io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Always advance the buffer so a write that overlaps a newline still
        // reports the requested byte count even if some bytes are flushed
        // through. `tracing-subscriber` does not inspect the returned
        // length, but the contract is "best-effort" — it must not be zero
        // for a non-empty buf unless the inner write fails.
        self.buffer.extend_from_slice(buf);
        // Flush every completed line so partial lines can survive until the
        // next write (or until `flush`/`drop` for the trailing one).
        while let Some(offset) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=offset).collect();
            let redacted = self.redactor.redact_bytes(&line);
            self.inner.write_all(&redacted)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // Forward any buffered bytes (redacted) before flushing the inner
        // writer. The dropped-into-guard writer still gets a chance to
        // persist a trailing partial line on every explicit `flush` the
        // `fmt` layer issues.
        if !self.buffer.is_empty() {
            let drained = std::mem::take(&mut self.buffer);
            let redacted = self.redactor.redact_bytes(&drained);
            self.inner.write_all(&redacted)?;
        }
        self.inner.flush()
    }
}

impl<W: io::Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        // A trailing unterminated line lives in `buffer`; if the event
        // lost its `\n` between the last `write` and `drop`, persist it
        // now. Errors are swallowed: by `Drop` time the writer is leaving
        // scope and the run is already winding down — printing an error
        // would race the process's shutdown and could panic.
        if !self.buffer.is_empty() {
            let drained = std::mem::take(&mut self.buffer);
            let redacted = self.redactor.redact_bytes(&drained);
            let _ = self.inner.write_all(&redacted);
            let _ = self.inner.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn empty_redactor_does_not_change_text() {
        let redactor = SecretRedactor::new();
        // Empty redactor: no known secrets. Bearer-pattern redaction still
        // fires on a real `bearer <token>` line, so here we only assert on
        // a truly benign line that has no secret-shaped substring at all.
        assert_eq!(redactor.redact("hello world"), "hello world");
    }

    #[test]
    fn known_secret_is_replaced() {
        let redactor = SecretRedactor::new().with_secret("sk-known-secret-123");
        let redacted = redactor.redact("header x-api-key: sk-known-secret-123 used");
        assert!(!redacted.contains("sk-known-secret-123"), "redactor must scrub the known secret; got {redacted:?}");
        assert!(redacted.contains(REDACTED), "marker must appear; got {redacted:?}");
        assert!(redacted.contains("header x-api-key:"), "unrelated context must be preserved; got {redacted:?}");
    }

    #[test]
    fn known_secret_with_json_special_chars_is_still_safe() {
        // Marker is plain ASCII; surrounding JSON delimiters stay intact.
        let redactor = SecretRedactor::new().with_secret("quoted-secret");
        let out = redactor.redact(r#"{"api_key": "quoted-secret"}"#);
        assert!(out.contains(REDACTED));
        assert_eq!(out, r#"{"api_key": "[REDACTED]"}"#.to_string());
    }

    #[test]
    fn bearer_token_is_redacted_case_insensitive() {
        let redactor = SecretRedactor::new();
        for prefix in ["Bearer", "bearer", "BEARER", "bEaReR"] {
            let redacted = redactor.redact(&format!("{prefix} abc.def.ghi"));
            assert!(!redacted.contains("abc.def.ghi"), "{prefix:?} must scrub the token; got {redacted:?}");
            assert!(redacted.contains(REDACTED), "{prefix:?} must insert the marker; got {redacted:?}");
        }
    }

    #[test]
    fn bearer_token_with_trailing_punctuation_is_redacted() {
        let redactor = SecretRedactor::new();
        let redacted = redactor.redact("Bearer xyz123abc, and Bearer abc.def.");
        assert!(redacted.contains(REDACTED));
        assert!(!redacted.contains("xyz123abc"));
        assert!(!redacted.contains("abc.def"));
        assert!(redacted.contains(", and "));
    }

    #[test]
    fn bare_word_bearer_without_token_is_left_alone() {
        let redactor = SecretRedactor::new();
        assert_eq!(redactor.redact("the bearer is here"), "the bearer is here");
        assert_eq!(redactor.redact("bearer:"), "bearer:");
        assert_eq!(redactor.redact("bearer"), "bearer");
    }

    #[test]
    fn non_bearer_strings_are_not_touched() {
        let redactor = SecretRedactor::new();
        let benign = "hello world, this is a totally ordinary log line";
        assert_eq!(redactor.redact(benign), benign);
    }

    #[test]
    fn dedupes_repeated_secrets() {
        let redactor = SecretRedactor::new().with_secret("once").with_secret("once");
        assert_eq!(redactor.known_secrets_len(), 1);
    }

    #[test]
    fn short_strings_are_not_added_as_secrets() {
        let redactor = SecretRedactor::new().with_secret("abc");
        assert_eq!(redactor.known_secrets_len(), 0);
        // A sub-floor bearer-shaped token must NOT be redacted: short
        // secret is rejected at insertion; bearer-pattern requires
        // `MIN_BEARER_TOKEN_LEN` bytes.
        assert_eq!(redactor.redact("bearer abc"), "bearer abc");
        let out_long = redactor.redact("bearer abcdefgh");
        assert!(out_long.contains(REDACTED));
        assert!(!out_long.contains("abcdefgh"));
    }

    #[test]
    fn from_sources_picks_up_required_env_var_values() {
        let env_vars = vec![
            ("ANTHROPIC_API_KEY".to_string(), "real-anthropic-key-xyz".to_string()),
            ("NOPE_NOT_A_REAL_KEY".to_string(), "should-not-be-redacted".to_string()),
        ];
        let redactor = SecretRedactor::from_sources(env_vars, std::iter::empty());
        let out = redactor.redact("prefix real-anthropic-key-xyz suffix");
        assert!(!out.contains("real-anthropic-key-xyz"));
        assert!(out.contains(REDACTED));
        let out_unrelated = redactor.redact("prefix should-not-be-redacted suffix");
        assert!(
            out_unrelated.contains("should-not-be-redacted"),
            "unrelated env values must not be seeded; got {out_unrelated:?}"
        );
    }

    #[test]
    fn from_sources_picks_up_secret_and_token_headers() {
        let mut headers = BTreeMap::new();
        headers.insert("X-Client-Secret".to_string(), "secret-here".to_string());
        headers.insert("Authorization".to_string(), "Bearer secret-token-123".to_string());
        headers.insert("X-Custom-Header".to_string(), "harmless".to_string());
        let redactor = SecretRedactor::from_sources(
            std::iter::empty::<(String, String)>(),
            vec![("provider".to_string(), headers)],
        );
        let out = redactor.redact("Authorization: Bearer secret-token-123");
        assert!(!out.contains("secret-token-123"), "redaction must scrub the bearer token; got {out:?}");
        assert!(!out.contains("Authorization: Bearer"), "the value half is replaced; got {out:?}");
        let benign = redactor.redact("X-Custom-Header: harmless");
        assert!(benign.contains("harmless"), "non-credential headers must not be redacted; got {benign:?}");
    }

    #[test]
    fn redact_bytes_round_trips_utf8() {
        let redactor = SecretRedactor::new().with_secret("sk-bad");
        let bytes = b"text sk-bad text";
        let out = redactor.redact_bytes(bytes);
        assert_eq!(out, b"text [REDACTED] text");
    }

    // ---- RedactingMakeWriter ----

    /// In-memory writer so we can capture what `RedactingMakeWriter`
    /// forwards to the inner writer. Holds a shared `Vec<u8>` behind a
    /// `Mutex` so a `Write` impl can append and a test can read the
    /// snapshot after the writer is dropped.
    #[derive(Clone, Default)]
    struct CaptureWriter {
        buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn snapshot(&self) -> Vec<u8> {
            self.buffer.lock().expect("capture lock").clone()
        }
    }

    impl io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer.lock().expect("capture lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn make_writer_redacts_seeded_secret_on_every_completed_line() {
        let capture = CaptureWriter::default();
        let redactor = SecretRedactor::new().with_secret("sk-shared-secret-1").with_secret("Bearer payload-bb");
        let factory = RedactingMakeWriter::new(capture.clone(), redactor);

        let mut writer = factory.make_writer();
        writer.write_all(b"run start sk-shared-secret-1 and Bearer payload-bb\n").expect("first line");
        writer.write_all(b"second line sk-shared-secret-1\n").expect("second line");
        writer.flush().expect("flush");

        let captured = String::from_utf8_lossy(&capture.snapshot()).into_owned();
        assert!(!captured.contains("sk-shared-secret-1"), "writer must scrub the secret; got:\n{captured}");
        assert!(!captured.contains("payload-bb"), "writer must scrub the bearer token; got:\n{captured}");
        assert!(captured.contains(REDACTED), "marker must appear in the captured bytes; got:\n{captured}");
        assert!(captured.contains("run start"));
        assert!(captured.contains("second line"));
    }

    #[test]
    fn make_writer_drops_unterminated_tail_on_flush() {
        let capture = CaptureWriter::default();
        let redactor = SecretRedactor::new().with_secret("tail-secret-9");
        let factory = RedactingMakeWriter::new(capture.clone(), redactor);

        let mut writer = factory.make_writer();
        writer.write_all(b"unterminated tail-secret-9").expect("write tail");
        writer.flush().expect("flush");

        let captured = String::from_utf8_lossy(&capture.snapshot()).into_owned();
        assert!(
            !captured.contains("tail-secret-9"),
            "flush must persist a trailing partial line, redacted; got {captured:?}"
        );
        assert!(captured.contains(REDACTED));
    }
}
