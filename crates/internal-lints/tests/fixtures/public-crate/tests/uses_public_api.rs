//! An integration test that uses the public API of the crate under test.
//!
//! The lint must leave it alone.

use public_crate_fixture::public_api;

#[test]
fn calls_the_public_function_from_outside() {
    assert_eq!(public_api(), 42);
}
