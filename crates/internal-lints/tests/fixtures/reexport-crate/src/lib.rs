//! A fixture crate for the private-item lint.
//!
//! The crate declares the `impl` block of its type in a module it reads before
//! the module that declares the type, and it re-exports the type under its
//! root. The lint must leave the crate alone: the type is public, and the
//! module that holds the `impl` block says nothing about the visibility of the
//! type.

/// The module that holds the `impl` block of the type.
mod schema {
    use crate::widget::Widget;

    impl Widget {
        /// The schema of the widget.
        pub fn schema(&self) -> String {
            self.name.clone()
        }
    }
}

/// The module that declares the type.
mod widget {
    /// The widget the crate declares.
    pub struct Widget {
        /// The name of the widget.
        pub name: String,
    }
}

pub use widget::*;
