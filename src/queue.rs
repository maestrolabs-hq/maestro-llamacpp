//! How long a request waits for room before it is refused.
//!
//! The router evicts the coldest idle model to make room. When every model
//! that could go is *busy* -- something is reading from it -- there is nothing
//! to evict without truncating somebody's answer, and the request that wanted
//! the room has two honest options: be told to come back, or wait.
//!
//! Before this module it was always told to come back, with a 503 and a note
//! saying a retry might work. That is correct and it puts the retry loop in
//! every caller, including the ones that do not have one. A wait here is the
//! same loop written once, in the place that knows when the room actually
//! frees up.
//!
//! Bounded, because the alternative is a request that never returns. What
//! expires is the wait, not the request: a caller that waited the whole window
//! and still found no room gets the refusal it would have got immediately.

use std::time::Duration;

use crate::launch::Failure;

/// Where the wait is configured.
const VARIABLE: &str = "MAESTRO_ADMISSION_WAIT_SECONDS";

/// How long to wait when the only room is held by something still answering.
///
/// Long enough to outlast an ordinary answer, which is what it is waiting for.
/// Short enough that a caller with no timeout of its own is not held for the
/// length of a long generation.
const DEFAULT: Duration = Duration::from_secs(60);

/// How long a request waits for room rather than being refused.
///
/// Zero means refuse immediately, which is the behaviour this router had
/// before the wait existed. Given a meaning on purpose rather than treated as
/// unset: an operator writing `0` into a variable is saying "do not wait", and
/// reading that as "wait the default" would be the opposite of what they said.
pub struct Wait(Duration);

impl Wait {
    /// A wait of exactly this long.
    ///
    /// Separate from reading the environment for the reason `Budget::new` is:
    /// a test states the wait it means directly rather than setting a
    /// process-global variable that every other test in its binary would race
    /// against.
    #[must_use]
    pub fn new(wait: Duration) -> Self {
        Self(wait)
    }

    /// The wait this machine is configured with, or the default.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the variable carries something that is not a
    /// whole number of seconds. A wait someone tried to set and mistyped must
    /// not silently become the default, because the difference is whether a
    /// caller is held or answered.
    pub fn configured() -> Result<Self, Failure> {
        let Some(value) = std::env::var_os(VARIABLE).filter(|value| !value.is_empty()) else {
            return Ok(Self(DEFAULT));
        };

        let text = value.to_string_lossy();
        let seconds = text.trim().parse().map_err(|_| {
            Failure::Unavailable(format!(
                "{VARIABLE} carries '{text}', which is not a whole number of \
                 seconds; unset it for the default of {}, or set 0 to refuse \
                 rather than wait",
                DEFAULT.as_secs()
            ))
        })?;
        Ok(Self(Duration::from_secs(seconds)))
    }

    /// How long to wait.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.0
    }

    /// Whether anything waits at all.
    #[must_use]
    pub fn waits(&self) -> bool {
        !self.0.is_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_means_refuse_rather_than_wait_the_default() {
        let wait = Wait::new(Duration::ZERO);
        assert!(
            !wait.waits(),
            "an operator writing 0 said 'do not wait', not 'wait the default'"
        );
    }

    #[test]
    fn a_stated_wait_is_carried_as_given() {
        assert_eq!(Wait::new(Duration::from_secs(5)).duration().as_secs(), 5);
    }
}
