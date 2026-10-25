//! A fixture crate for the private-item lint.
//!
//! Its tests reach the public API of the crate only, which the lint must leave
//! alone.

/// The public configuration of the crate.
pub struct Config {
    /// The public value of the configuration.
    pub value: u32,
}

/// The public API of the crate.
pub fn public_api() -> u32 {
    42
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_the_public_function() {
        assert_eq!(public_api(), 42);
    }

    #[test]
    fn reaches_the_public_field() {
        let config = Config { value: 7 };
        assert_eq!(config.value, 7);
    }
}
