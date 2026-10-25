//! A fixture crate for the private-item lint.
//!
//! Its unit tests reach the private items of the crate root, and its integration
//! test reaches a private item from outside the crate. The lint must report both.

/// The configuration the crate keeps to itself.
struct Config {
    /// The value the crate keeps to itself.
    secret: u32,
}

/// The helper the crate keeps to itself.
fn private_helper() -> u32 {
    42
}

/// The public API of the crate.
pub fn public_api() -> u32 {
    private_helper()
}

impl Config {
    /// The method the crate keeps to itself.
    fn private_method(&self) -> u32 {
        self.secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_the_private_function() {
        assert_eq!(private_helper(), 42);
    }

    #[test]
    fn reaches_the_private_field() {
        let config = Config { secret: 7 };
        assert_eq!(config.secret, 7);
    }

    #[test]
    fn calls_the_private_method() {
        let config = Config { secret: 7 };
        assert_eq!(config.private_method(), 7);
    }
}
