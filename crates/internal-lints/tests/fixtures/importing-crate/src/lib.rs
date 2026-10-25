//! A fixture crate for the private-item lint.
//!
//! It keeps a helper to itself, and its public API reaches the helper.

/// The helper the crate keeps to itself.
fn private_helper() -> u32 {
    42
}

/// The public API of the crate.
pub fn public_api() -> u32 {
    private_helper()
}

#[cfg(test)]
mod tests {
    use crate::private_helper;

    #[test]
    fn imports_the_private_function_from_inside() {
        let _ = private_helper;
    }
}