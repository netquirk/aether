//! CLI startup identifiers — `aether`'s own version and the ACP protocol version it speaks.
//!
//! A single source of truth for the values printed to stderr at run start and
//! returned in the ACP `initialize` response, so a transcript can record which
//! build produced it.

use agent_client_protocol::schema::ProtocolVersion;

/// ACP protocol version that the `aether` CLI speaks.
pub const ACP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V2;

/// Returns the crate version (`CARGO_PKG_VERSION`) of the `aether-agent-cli` crate.
#[must_use]
pub const fn aether_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns the banner printed on stderr when the ACP server starts.
///
/// The banner names both the aether version and the ACP protocol version so a
/// transcript of the run can record which build produced it.
#[must_use]
pub fn startup_line() -> String {
    format!("aether {} (ACP protocol version {})", aether_version(), ACP_PROTOCOL_VERSION.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aether_version_matches_cargo_pkg_version() {
        assert_eq!(aether_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn acp_protocol_version_is_two() {
        assert_eq!(ACP_PROTOCOL_VERSION.as_u16(), 2);
    }

    #[test]
    fn startup_line_names_aether_version_and_protocol_version() {
        let line = startup_line();
        assert!(line.contains(env!("CARGO_PKG_VERSION")), "{line}");
        assert!(line.contains("ACP protocol version 2"), "{line}");
    }
}
