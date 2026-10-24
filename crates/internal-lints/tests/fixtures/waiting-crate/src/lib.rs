//! A fixture crate for the fixed-wait lint.
//!
//! Its tests hold the waits the lint must report (a fixed sleep, a duration
//! bound) and the two it must leave alone (a sleep on the paused clock, a
//! sleep next to a fake provider).

use std::time::Duration;

/// The answer a provider gives, without any clock involved.
pub fn answer() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake provider that answers without a clock.
    struct FakeProvider;

    impl FakeProvider {
        /// The answer the fake gives.
        fn answer(&self) -> String {
            String::new()
        }
    }

    /// Wait for a fixed duration, the way a bounded wait would.
    fn wait_for(duration: Duration) {
        std::thread::sleep(duration);
    }

    #[test]
    fn sleeps_for_a_fixed_duration() {
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(answer(), "");
    }

    #[tokio::test(start_paused = true)]
    async fn sleeps_on_the_paused_clock() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    #[test]
    fn drives_a_fake_provider() {
        let provider = FakeProvider;
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(provider.answer(), "");
    }

    #[test]
    fn waits_with_a_duration_bound() {
        wait_for(Duration::from_millis(10));
    }
}
