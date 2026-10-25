//! A fixture crate for the private-item lint.
//!
//! Its unit tests reach the private items of the module they are written in, of the module they
//! are nested in, of a sibling module and of the crate root. The lint must report the sibling and
//! the crate root, and leave the other two alone: the private items of the module a test is written
//! in, and of the modules it is nested in, are the internals of the module under test.

/// The helper the crate root keeps to itself.
fn crate_root_helper() -> u32 {
    1
}

/// The module that keeps its helper to itself.
mod sibling {
    /// The helper the sibling module keeps to itself.
    fn sibling_helper() -> u32 {
        2
    }
}

/// The module under test.
mod module {
    /// The helper the module keeps to itself.
    fn module_helper() -> u32 {
        3
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The helper the test module keeps to itself.
        fn test_helper() -> u32 {
            4
        }

        #[test]
        fn reaches_the_private_helper_of_the_module_it_is_written_in() {
            assert_eq!(test_helper(), 4);
        }

        #[test]
        fn reaches_the_private_helper_of_the_module_it_is_nested_in() {
            assert_eq!(module_helper(), 3);
        }

        #[test]
        fn reaches_the_private_helper_of_the_crate_root() {
            assert_eq!(crate::crate_root_helper(), 1);
        }

        #[test]
        fn reaches_the_private_helper_of_a_sibling_module() {
            assert_eq!(crate::sibling::sibling_helper(), 2);
        }
    }
}
