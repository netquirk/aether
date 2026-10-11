//! Integration tests for the refusal of oversized system prompts against the
//! configured context window (aether: a run whose system prompt exceeds the
//! model's configured context window is refused before any provider call).
//!
//! Each test points `aether headless` at an `Inspector` agent that pins a
//! small `contextWindow` and otherwise relies on offline defaults. The
//! `--agent` selector is used (rather than `--model`) so the agent's
//! `contextWindow` actually reaches `AgentSpec::context_window`; passing
//! `--model` would route through `catalog.default_spec` which builds a
//! bare spec with `context_window: None` and silently skip the check. The
//! refused run must exit non-zero and name both sizes in stderr; the
//! fitting run must proceed past the check (no refusal message). All
//! three tests bypass the provider with a TCP listener bound to
//! `--provider ollama.url=...` so the run neither reaches the network
//! nor hangs on retries; the listener also catches the connection
//! attempt the post-check code path makes, giving us a positive signal
//! that the check fired (no connection) or didn't (connection).

use std::error::Error;
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

type TestResult = std::result::Result<(), Box<dyn Error>>;

/// Settings block with one user-invocable agent whose `contextWindow` is
/// fixed at the supplied value. The prompt file path is required by
/// `AgentConfig` (an agent with zero prompts is rejected), so each test
/// creates one alongside the run directory. The `ollama:llama3.2` model
/// has no remote dependency and matches the rest of the offline
/// integration suite.
fn settings_with_context_window(context_window: u32) -> String {
    format!(
        r#"{{
            "credentialsStore": {{ "type": "memory" }},
            "agents": [
                {{
                    "name": "Inspector",
                    "description": "Inspect-only agent for the context-window integration test",
                    "model": "ollama:llama3.2",
                    "userInvocable": true,
                    "contextWindow": {context_window},
                    "prompts": [".aether/SYSTEM.md"]
                }}
            ]
        }}"#
    )
}

/// Settings block with one user-invocable agent that does NOT pin a
/// `contextWindow`. Used to exercise the "check is skipped when unset"
/// branch of the deliverable.
fn settings_without_context_window() -> String {
    r#"{
        "credentialsStore": { "type": "memory" },
        "agents": [
            {
                "name": "Inspector",
                "description": "Inspect-only agent for the context-window integration test",
                "model": "ollama:llama3.2",
                "userInvocable": true,
                "prompts": [".aether/SYSTEM.md"]
            }
        ]
    }"#
    .to_string()
}

/// A short-lived TCP listener used as a stand-in Ollama endpoint. The
/// listener is bound on `127.0.0.1:0`, runs `accept` on a background
/// thread, and forwards the first peer's address back over a channel.
/// The headless run is configured to point at this listener via
/// `--provider ollama.url=http://<addr>`; if the prompt-context-window
/// check fires the listener never sees one; if the check fired / was
/// skipped the provider client will reach the listener before the run
/// exits. The thread is detached (no join) so a check-fired test does
/// not stall on the listener thread waiting for a connection that will
/// never come.
struct CapturingListener {
    addr: SocketAddr,
    receiver: mpsc::Receiver<Option<SocketAddr>>,
    _join: thread::JoinHandle<()>,
}

impl CapturingListener {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind to ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            // `accept` blocks until either a connection arrives (good —
            // the check fired or was skipped and the provider client
            // reached us) or the run exits and the test process drops the
            // channel sender. The latter is the refused-run path: the
            // listener thread waits forever, but the test is satisfied
            // by `received_connection_within` returning `false` after a
            // bounded wait, so a stuck thread does not stall the suite.
            let peer = listener.accept().ok().map(|(_, peer)| peer);
            let _ = tx.send(peer);
        });
        Self { addr, receiver: rx, _join: join }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Block up to `timeout` for the listener thread to forward a peer
    /// address. Returns `true` if a connection arrived before the
    /// timeout (the check fired or was skipped and the provider client
    /// reached the listener) and `false` otherwise (no connection ever
    /// came, proving the check refused the run before any provider
    /// call).
    fn received_connection_within(&self, timeout: Duration) -> bool {
        matches!(self.receiver.recv_timeout(timeout), Ok(Some(_peer)))
    }
}

/// Spawn the headless run with the supplied settings and system prompt,
/// pointing the run's Ollama endpoint at `listener` when present. The
/// `--agent Inspector` selector is used (not `--model`) so the agent's
/// `contextWindow` actually reaches the spec. `--timeout 5s` bounds the
/// post-check retries so the run does not hang for ~6 seconds on every
/// refused-network-error cycle; the timeout is irrelevant to the
/// refused test (the check fires before the loop starts) and only
/// shapes the fitting / unset tests so they exit in bounded time.
fn run_headless(
    dir: &std::path::Path,
    settings: &str,
    system_prompt: &str,
    listener: Option<&CapturingListener>,
) -> std::process::Output {
    fs_write(&dir.join("settings.json"), settings);
    fs_write(&dir.join(".aether/SYSTEM.md"), BASE_PROMPT);
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("headless")
        .arg("--settings-json")
        .arg(settings)
        .arg("--cwd")
        .arg(dir)
        .arg("--agent")
        .arg("Inspector")
        .arg("--system-prompt")
        .arg(system_prompt)
        .arg("--log-level")
        .arg("error")
        .arg("--timeout")
        .arg("5s")
        .arg("hi")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(listener) = listener {
        command.arg("--provider").arg(format!("ollama.url={}", listener.endpoint()));
    }
    command.output().expect("spawning aether must succeed")
}

/// Contents of the agent's `prompts` array — kept in one `const` so the
/// refusal test can derive the expected estimated-token count from the
/// same string the run sees on disk.
const BASE_PROMPT: &str = "inline base prompt\n";

fn fs_write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir -p");
    }
    std::fs::write(path, contents).expect("write file");
}

/// Baseline (the one this whole file lives to prove): a run whose
/// `--system-prompt` plus the configured overhead exceeds the agent's
/// `contextWindow` is refused before any provider call, naming both the
/// prompt size (in tokens) and the window size. The refusal is the
/// `CliError::SystemPromptExceedsContextWindow` variant the headless run
/// returns when the check fires.
///
/// Concretely we set `contextWindow: 500` (500 tokens) and pass a
/// 4_000-character `--system-prompt`. The 4 bytes/token heuristic yields
/// an estimated prompt-token count comfortably above 500, so the run
/// must be refused and stderr must surface both numbers. The TCP
/// listener is unused here: the check fires before any provider work,
/// so no connection should ever arrive at the listener — that is the
/// negative assertion we use to prove the refusal happened upstream of
/// the provider call. The estimated token count is computed dynamically
/// (`base_prompt_chars + system_prompt_chars`) / 4 so a future tweak to
/// the base prompt does not silently break the assertion.
#[test]
fn headless_refuses_run_when_system_prompt_exceeds_context_window() -> TestResult {
    let dir = tempfile::tempdir()?;
    let settings = settings_with_context_window(500);
    let prompt = "x".repeat(4_000);
    let listener = CapturingListener::spawn();

    let output = run_headless(dir.path(), &settings, &prompt, Some(&listener));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "oversized prompt must be refused; got exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stderr.contains("system prompt"),
        "stderr must name the system prompt as the cause: {stderr}",
    );
    assert!(
        stderr.contains("context window"),
        "stderr must name the context window as the limit: {stderr}",
    );
    // The 4 bytes/token estimate includes the agent's configured base
    // prompt (`inline base prompt\n`, 19 chars) AND the inline
    // `--system-prompt` (4_000 chars). `Prompt::build_all` joins the
    // two parts with a `\n\n` separator (2 chars), so the total
    // assembled length is `19 + 2 + 4_000 = 4_021` and the reported
    // token count is `4_021 / 4 = 1_005`. Both numbers must appear in
    // the message so an operator can tell which size is which without
    // re-running.
    #[allow(clippy::cast_possible_truncation)]
    let expected_tokens = ((BASE_PROMPT.len() + "\n\n".len() + prompt.len()) / 4) as u32;
    assert!(
        stderr.contains(&expected_tokens.to_string()),
        "stderr must name the prompt-token count ({expected_tokens}): {stderr}",
    );
    assert!(stderr.contains("500"), "stderr must name the window size (500): {stderr}");
    // Negative signal: the check refused the run before any provider
    // call, so the listener never received a connection.
    assert!(
        !listener.received_connection_within(Duration::from_secs(2)),
        "the check must refuse before any provider call (no connection at the listener)",
    );

    Ok(())
}

/// Companion test (the "a run that fits starts as before" half of the
/// deliverable): the same agent with the same `contextWindow: 500` is
/// passed a system prompt small enough to fit (40 chars -> ~10
/// estimated tokens). The check passes, so the run proceeds to the
/// headless loop, which then makes a provider call against the
/// listener. The listener thread recording the incoming connection is
/// the positive signal that the check fired / was skipped and the run
/// actually attempted the model call (i.e. did not refuse).
#[test]
fn headless_starts_run_when_system_prompt_fits_context_window() -> TestResult {
    let dir = tempfile::tempdir()?;
    let settings = settings_with_context_window(500);
    let prompt = "x".repeat(40); // ~10 estimated tokens, well under 500
    let listener = CapturingListener::spawn();

    let output = run_headless(dir.path(), &settings, &prompt, Some(&listener));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("exceeds the model's configured context window"),
        "fitting run must not surface the refusal message: {stderr}",
    );
    assert!(
        listener.received_connection_within(Duration::from_secs(8)),
        "the fitting run must attempt the provider call (listener got a connection), proving the check fired / was skipped; stdout: {stdout}; stderr: {stderr}",
    );

    Ok(())
}

/// The check is gated on `contextWindow` being set; an agent without one
/// proceeds exactly as it did before this change. We exercise that default
/// with a deliberately oversized prompt and confirm the run is NOT
/// refused at the size-check phase (it instead proceeds to provider
/// work, which here hits the TCP listener).
///
/// This pins the "if none is set, the check is skipped and the run
/// proceeds" half of the task's "Default taken where the ask forks"
/// clause. Without this guard a future change that always runs the
/// check would silently break agents whose provider reports a window
/// the configured agent doesn't override.
#[test]
fn headless_skips_context_window_check_when_unset() -> TestResult {
    let dir = tempfile::tempdir()?;
    let settings = settings_without_context_window();
    let prompt = "x".repeat(4_000); // comfortably past any default
    let listener = CapturingListener::spawn();

    let output = run_headless(dir.path(), &settings, &prompt, Some(&listener));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("system prompt exceeds"),
        "no-window run must not surface the refusal: {stderr}",
    );
    assert!(
        listener.received_connection_within(Duration::from_secs(8)),
        "the no-window run must attempt the provider call (listener got a connection), proving the check was skipped; stdout: {stdout}; stderr: {stderr}",
    );

    Ok(())
}

