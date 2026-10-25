//! An integration test that imports a private item of the crate under test.
//!
//! The import is a reach of the private item, even though the test never calls
//! it: the lint must report it, because a test that lives outside the crate may
//! use the public API of the crate only.

use importing_crate_fixture::private_helper;

#[test]
fn imports_the_private_function() {
    let _ = private_helper;
}
