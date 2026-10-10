//! Controls whether the CLI emits ANSI colour / cursor-control escape sequences.
//!
//! The CLI honours the environment variable [`NO_COLOR`] in addition to any
//! richer TTY-detection mechanisms callers may add later. The
//! [no-color.org](https://no-color.org/) convention is: the variable's presence,
//! irrespective of its value, signals a preference for colour-free output; an
//! empty value is treated as "not set" so a single `NO_COLOR=` in the
//! environment does not disable colour.
//!
//! [`NO_COLOR`]: https://no-color.org/

/// Returns `true` when the CLI must not emit ANSI / cursor-control escape
/// sequences. Honours the `NO_COLOR` convention: the variable must be present
/// (case-sensitive — `env::var_os("NO_COLOR")` does not match `no_color` or
/// `No_Color`) **and** non-empty to take effect. Per the [no-color.org]
/// standard the variable name is documented in upper case, so the exact-name
/// match is the intended behaviour.
pub(crate) fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// Inverse of [`no_color`]: colour codes may be emitted when no `NO_COLOR`
/// signal is set. Centralised so callers do not need to remember to invert the
/// gate.
pub(crate) fn color_enabled() -> bool {
    !no_color()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::*;

    /// Process-global env mutations are not safe to run in parallel: tests
    /// that touch `NO_COLOR` must hold this lock and restore the prior value
    /// on the way out. Exposed for use across the crate so every test that
    /// mutates `NO_COLOR` shares the same mutex.
    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock_env() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn no_color_disables_when_variable_is_present_and_non_empty() {
        let _guard = lock_env();
        // `NO_COLOR=1` is the example from the task; any non-empty value
        // disables colour.
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        assert!(no_color());
        assert!(!color_enabled());
    }

    #[test]
    fn no_color_ignores_empty_value() {
        let _guard = lock_env();
        // `env::set_var` requires non-empty on Unix for safety in edition
        // 2024 but `var_os` distinguishes empty from unset, so we set then
        // clear via the OS path. Setting to an empty string on platforms that
        // accept it is the documented `NO_COLOR=` opt-out case.
        let prior = std::env::var_os("NO_COLOR");
        unsafe {
            std::env::set_var("NO_COLOR", "");
        }
        // Empty means "not set" per the no-color.org spec, so colour stays on.
        assert!(!no_color());
        assert!(color_enabled());
        match prior {
            Some(v) => unsafe {
                std::env::set_var("NO_COLOR", v);
            },
            None => unsafe {
                std::env::remove_var("NO_COLOR");
            },
        }
    }

    #[test]
    fn no_color_absent_means_color_enabled() {
        let _guard = lock_env();
        let prior = std::env::var_os("NO_COLOR");
        unsafe {
            std::env::remove_var("NO_COLOR");
        }
        assert!(!no_color());
        assert!(color_enabled());
        if let Some(v) = prior {
            unsafe {
                std::env::set_var("NO_COLOR", v);
            }
        }
    }
}
