use std::error::Error;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;

/// The first line of an ACP run names the aether version and the ACP protocol version.
///
/// Stdout stays reserved for the JSON-RPC channel; the banner must go to stderr.
#[test]
fn first_stderr_line_names_aether_version_and_protocol_version() -> TestResult {
    let log_dir = tempfile::tempdir()?;
    let mut child =
        acp_command(log_dir.path()).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?;

    // Close stdin immediately so the server sees EOF before the client speaks.
    child.stdin.take().ok_or_else(|| std::io::Error::other("child stdin"))?;
    let stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("child stderr"))?;
    let mut reader = BufReader::new(stderr);

    let mut line = String::new();
    reader.read_line(&mut line)?;

    let first = line.trim_end_matches(&['\r', '\n'][..]);
    assert!(
        first.contains(env!("CARGO_PKG_VERSION")),
        "first stderr line should name the aether build version, got {first:?}",
    );
    assert!(first.contains("ACP"), "first stderr line should reference ACP, got {first:?}",);
    assert!(first.contains('2'), "first stderr line should name the ACP protocol version, got {first:?}",);

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn acp_command(log_dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .arg("acp")
        .arg("--log-dir")
        .arg(log_dir)
        .arg("--settings-json")
        .arg(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    command
}
