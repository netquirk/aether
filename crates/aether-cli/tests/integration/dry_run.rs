//! Integration tests for `aether headless --dry-run`.
//!
//! `aether headless --dry-run` must print the resolved model/profile/endpoint
//! to stdout, exit 0, and never start a session or contact the provider.
//! A local `TcpListener` claims a port that we point the dry-run invocation
//! at; if any provider request were attempted, the listener would see it.

use std::error::Error;
use std::io;
use std::net::TcpListener;
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// `aether headless --dry-run` exits 0, prints the resolved model + profile +
/// endpoint lines, and never tries to connect to the provider that the
/// `--provider` flag points at. The `TcpListener` claim is the negative
/// assertion: after the child exits we still see `WouldBlock` on `accept()`,
/// proving the dry-run path bypassed both session construction and the
/// provider HTTP client.
#[test]
fn dry_run_prints_summary_and_does_not_call_provider() -> TestResult {
    let dir = tempfile::tempdir()?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let endpoint = format!("http://{addr}");

    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("headless")
        .arg("--dry-run")
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("ollama:llama3.2")
        .arg("--provider")
        .arg(format!("ollama.url={endpoint}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "dry-run must exit 0, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );

    assert!(stdout.contains("model: ollama:llama3.2"), "stdout must name the resolved model; got:\n{stdout}");
    assert!(stdout.contains("profile: "), "stdout must name the resolved profile; got:\n{stdout}");
    assert!(
        stdout.contains(&format!("endpoint: {endpoint}")),
        "stdout must name the resolved endpoint; got:\n{stdout}",
    );

    // The dry-run path must not make any TCP connection. With the listener
    // set non-blocking and no connection having been attempted, `accept`
    // returns `WouldBlock` immediately.
    match listener.accept() {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(Box::new(error)),
        Ok((_socket, peer)) => {
            panic!("dry-run should never connect to the provider, but got a connection from {peer}")
        }
    }

    Ok(())
}

/// Sanity-check the same flow against a generic `--provider` URL override:
/// the printed endpoint must reflect the URL exactly, not the provider's
/// default. This is the same `resolved_summary` path with a real override.
#[test]
fn dry_run_prints_provider_url_override() -> TestResult {
    let dir = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let endpoint = format!("http://{addr}");

    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("headless")
        .arg("--dry-run")
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--model")
        .arg("anthropic:claude-sonnet-4-5")
        .arg("--provider")
        .arg(format!("anthropic.url={endpoint}"))
        .stdin(Stdio::null())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "dry-run must exit 0 with a URL override, got {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code(),
    );
    assert!(
        stdout.contains(&format!("endpoint: {endpoint}")),
        "endpoint must match the --provider URL override; got:\n{stdout}",
    );
    assert!(stdout.contains("model: anthropic:claude-sonnet-4-5"), "got:\n{stdout}");

    // Still no provider call.
    match listener.accept() {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Ok((_socket, peer)) => {
            panic!("dry-run should never connect to the provider, but got a connection from {peer}")
        }
        Err(error) => return Err(Box::new(error)),
    }

    Ok(())
}
