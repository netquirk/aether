//! Integration tests for the bind policy of the raw `aether server`.
//!
//! The server carries no authentication of its own: whoever reaches the
//! listener owns the agent session. It therefore listens on loopback by
//! default, refuses any wider address unless the operator acknowledges the
//! risk with `--allow-unauthenticated-bind`, and says so loudly when it does.

use aether_cli::acp::server::{check_unauthenticated_bind, unauthenticated_bind_warning};
use std::error::Error;
use std::net::SocketAddr;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

/// A loopback listener is the default and needs no acknowledgement.
#[test]
fn a_loopback_bind_needs_no_acknowledgement() -> TestResult {
    for address in ["127.0.0.1:8765", "[::1]:8765"] {
        let address: SocketAddr = address.parse()?;
        check_unauthenticated_bind(address, false)?;
    }
    Ok(())
}

/// A wider address exposes the unauthenticated session to the network, so it
/// is refused until the operator acknowledges the risk.
#[test]
fn a_non_loopback_bind_is_refused_without_acknowledgement() -> TestResult {
    let address: SocketAddr = "0.0.0.0:8765".parse()?;
    let error = check_unauthenticated_bind(address, false).expect_err("a non-loopback bind must be refused");
    let message = error.to_string();
    assert!(message.contains("--allow-unauthenticated-bind"), "the refusal must name the opt-in flag: {message}");
    assert!(message.contains("unauthenticated"), "the refusal must name the reason: {message}");
    Ok(())
}

/// The acknowledged bind is allowed, and the warning tells the operator what
/// an unauthenticated listener means.
#[test]
fn an_acknowledged_non_loopback_bind_is_allowed_and_warns() -> TestResult {
    let address: SocketAddr = "192.0.2.10:8765".parse()?;
    check_unauthenticated_bind(address, true)?;
    let warning = unauthenticated_bind_warning(address);
    assert!(warning.contains("WARNING"), "the warning must be loud: {warning}");
    assert!(warning.contains(&address.to_string()), "the warning must name the address: {warning}");
    assert!(warning.contains("tunnel"), "the warning must name the tunnel: {warning}");
    assert!(warning.contains("proxy"), "the warning must name the proxy: {warning}");
    Ok(())
}

/// The CLI refuses the bind before it opens a listener, and names the flag
/// that acknowledges it.
#[test]
fn the_cli_refuses_a_non_loopback_bind_without_the_acknowledgement() -> TestResult {
    let output = Command::new(env!("CARGO_BIN_EXE_aether"))
        .arg("server")
        .arg("--listen")
        .arg("0.0.0.0:8765")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    assert!(!output.status.success(), "the server must refuse to start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--allow-unauthenticated-bind"), "stderr must name the opt-in flag: {stderr}");
    assert!(stderr.contains("unauthenticated"), "stderr must name the reason: {stderr}");
    Ok(())
}
