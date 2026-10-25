use aether_cli::{MAX_TOOL_CALLS_EXIT_CODE, TIMEOUT_EXIT_CODE};

/// The exit codes the headless run returns for its two distinct stops are
/// part of the crate's public API: the public documentation of
/// `HeadlessArgs::timeout` and `HeadlessArgs::max_tool_calls` links to them,
/// and a caller that drives the headless run from the library has to compare
/// the returned `ExitCode` against them. This test pins the two values the
/// documentation names, so a rename or a value change cannot leave the docs
/// and the run disagreeing.
#[test]
fn exit_codes_are_public_and_match_the_documented_values() {
    assert_eq!(TIMEOUT_EXIT_CODE, 124, "the `--timeout` exit code is 124, like timeout(1)");
    assert_eq!(MAX_TOOL_CALLS_EXIT_CODE, 125, "the `--max-tool-calls` exit code is 125");
    assert_ne!(
        TIMEOUT_EXIT_CODE, MAX_TOOL_CALLS_EXIT_CODE,
        "a timed-out run and a capped run must be told apart by their exit code"
    );
}
