//! An integration test that reaches a private item of the crate under test.
//!
//! The lint must report it: a test that lives outside the crate may use the
//! public API of the crate only.

#[test]
fn calls_the_private_function_from_outside() {
    assert_eq!(private_crate_fixture::private_helper(), 42);
}
